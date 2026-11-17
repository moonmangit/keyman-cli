//! Link, push, pull, status and delete for cloud projects.
//!
//! The pure parts (merge, write, status) are separate functions so they can be
//! tested without a network. Order matters on pull: compute everything first,
//! write the master file, and only then change the link and the state file.

use super::account::Unlocked;
use super::api::Membership;
use super::crypto::{self, Key, KeyPair};
use super::groups;
use super::merge::{self, Conflict};
use super::state::{self, Link};
use crate::master::{self, Variables};
use crate::save;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The vault keys of one user: vault ID to (vault key, key generation).
pub struct Vaults {
    by_id: HashMap<String, (Key, i64)>,
    memberships: Vec<Membership>,
}

impl Vaults {
    /// Unwraps the key of every vault the user belongs to. A vault whose key
    /// does not unwrap is left out, and `key` reports it when it is needed.
    pub fn load(u: &Unlocked) -> Result<Vaults, String> {
        Ok(Vaults::from_memberships(u, &u.api.my_memberships()?))
    }

    fn from_memberships(u: &Unlocked, memberships: &[Membership]) -> Vaults {
        Vaults::from_pair(&u.pair, memberships)
    }

    /// The same, from a key pair. The key is stored with the generation of
    /// the membership it was unwrapped from.
    pub(crate) fn from_pair(pair: &KeyPair, memberships: &[Membership]) -> Vaults {
        let by_id = memberships
            .iter()
            .filter_map(|member| {
                let key = crypto::unwrap(pair, &member.wrapped_key).ok()?;
                Some((member.vault_id.clone(), (key, member.key_generation)))
            })
            .collect();
        Vaults {
            by_id,
            memberships: memberships.to_vec(),
        }
    }

    /// The memberships that were read when the keys were loaded.
    pub fn memberships(&self) -> &[Membership] {
        &self.memberships
    }

    pub fn key(&self, vault_id: &str) -> Result<&(Key, i64), String> {
        self.by_id
            .get(vault_id)
            .ok_or_else(|| "Keyman has no key for this vault.".to_string())
    }
}

fn personal_in(memberships: &[Membership]) -> Result<String, String> {
    memberships
        .iter()
        .find(|member| member.vaults.kind == "personal")
        .map(|member| member.vault_id.clone())
        .ok_or_else(|| "You have no personal vault. Run 'keyman login' first.".to_string())
}

pub fn personal_vault(u: &Unlocked) -> Result<String, String> {
    personal_in(&u.api.my_memberships()?)
}

/// The vault of the group with this name. Two groups with one name are
/// refused: an outsider can invite a person to a group with the same name.
pub(crate) fn group_vault(vaults: &Vaults, name: &str) -> Result<String, String> {
    groups::find_in(vaults, name).map_err(|error| {
        if error.starts_with("There is no group") {
            format!("You are not in a group named '{name}'.")
        } else {
            error
        }
    })
}

const KEY_CHANGED: &str = "The group key changed. Run 'keyman pull' again.";

/// After a removal the projects are at a new key generation. A different
/// generation is a stale key, not a damaged project.
fn open_failure(key_generation: i64, row_generation: i64, reason: String) -> String {
    if key_generation != row_generation {
        KEY_CHANGED.to_string()
    } else {
        cannot_open(&reason)
    }
}

fn cannot_open(reason: &str) -> String {
    format!("Keyman cannot open this project. {reason}")
}

pub fn skip_reason_missing(project: &str) -> String {
    format!(
        "Project '{project}' is not in the master file. Run 'keyman unlink {project}' to remove the link."
    )
}

fn not_linked(project: &str) -> String {
    format!("Project '{project}' is not linked. Run 'keyman link {project}' first.")
}

/// Same names and values, in any order.
fn same_vars(a: &[(String, String)], b: &[(String, String)]) -> bool {
    a.len() == b.len() && a.iter().all(|pair| b.contains(pair))
}

/// How many variables differ: added, removed or changed.
fn count_changes(before: &[(String, String)], after: &[(String, String)]) -> usize {
    let get = |list: &[(String, String)], name: &str| {
        list.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    };
    let changed = after
        .iter()
        .filter(|(name, value)| get(before, name).as_ref() != Some(value))
        .count();
    let removed = before
        .iter()
        .filter(|(name, _)| get(after, name).is_none())
        .count();
    changed + removed
}

/// Fails when the project is already linked on this computer. Offline.
pub fn ensure_unlinked(home: &Path, project: &str) -> Result<(), String> {
    if state::load(home)?.link(project).is_some() {
        return Err(format!("Project '{project}' is already linked."));
    }
    Ok(())
}

/// Refuses a name that is already in the target vault, so a second computer
/// or a second `link` cannot make a duplicate.
fn cloud_name_is_free(cloud_names: &[String], project: &str) -> Result<(), String> {
    if cloud_names.iter().any(|name| name == project) {
        return Err(format!(
            "A project named '{project}' is in the cloud. Run 'keyman pull {project}' to get it."
        ));
    }
    Ok(())
}

pub fn link(home: &Path, u: &Unlocked, project: &str, group: Option<&str>) -> Result<Link, String> {
    let (_, yaml) = master::read_master(home)?;
    let variables = master::project_variables(&yaml, project)?;
    ensure_unlinked(home, project)?;
    let mut st = state::load(home)?;
    let memberships = u.api.my_memberships()?;
    let vaults = Vaults::from_memberships(u, &memberships);
    let vault_id = match group {
        Some(name) => group_vault(&vaults, name)?,
        None => personal_in(&memberships)?,
    };
    let (key, generation) = vaults.key(&vault_id)?;
    let existing = u.api.projects(std::slice::from_ref(&vault_id))?;
    let names: Vec<String> = existing
        .iter()
        .filter_map(|row| {
            crypto::open_project(key, &row.id, row.version, &row.sealed)
                .ok()
                .map(|(name, _)| name)
        })
        .collect();
    cloud_name_is_free(&names, project)?;
    let row = u.api.insert_project(&vault_id, *generation, |id| {
        crypto::seal_project(key, id, 1, project, &variables)
    })?;
    let link = Link {
        project: project.to_string(),
        cloud_id: row.id,
        vault_id,
        version: row.version,
        base: variables,
        cloud_name: project.to_string(),
    };
    st.set_link(link.clone());
    state::save(home, &st)?;
    Ok(link)
}

pub fn unlink(home: &Path, project: &str) -> Result<(), String> {
    let mut st = state::load(home)?;
    if !st.remove_link(project) {
        return Err(format!("Project '{project}' is not linked."));
    }
    state::save(home, &st)
}

#[derive(Debug, PartialEq)]
pub enum PushResult {
    Pushed { project: String, version: i64 },
    UpToDate(String),
    CloudIsNewer(String),
    Skipped { project: String, reason: String },
}

pub fn push(home: &Path, u: &Unlocked, project: Option<&str>) -> Result<Vec<PushResult>, String> {
    let mut st = state::load(home)?;
    let names: Vec<String> = match project {
        Some(name) => {
            if st.link(name).is_none() {
                return Err(not_linked(name));
            }
            vec![name.to_string()]
        }
        None => st.links.iter().map(|link| link.project.clone()).collect(),
    };
    let (_, yaml) = master::read_master(home)?;
    let local_names = master::project_names(&yaml)?;
    let mut vaults: Option<Vaults> = None;
    let mut results = Vec::new();
    for name in names {
        let Some(link) = st.link(&name).cloned() else {
            continue;
        };
        if !local_names.contains(&name) {
            results.push(PushResult::Skipped {
                project: name.clone(),
                reason: skip_reason_missing(&name),
            });
            continue;
        }
        let local = match master::project_variables(&yaml, &name) {
            Ok(local) => local,
            Err(reason) => {
                results.push(PushResult::Skipped {
                    project: name,
                    reason,
                });
                continue;
            }
        };
        if same_vars(&local, &link.base) {
            results.push(PushResult::UpToDate(name));
            continue;
        }
        if vaults.is_none() {
            vaults = Some(Vaults::load(u)?);
        }
        let found = vaults
            .as_ref()
            .expect("the vaults are loaded above")
            .key(&link.vault_id);
        let (key, generation) = match found {
            Ok(found) => found,
            Err(reason) => {
                results.push(PushResult::Skipped {
                    project: name,
                    reason,
                });
                continue;
            }
        };
        let next = link.version + 1;
        let sealed_name = if link.cloud_name.is_empty() {
            &name
        } else {
            &link.cloud_name
        };
        let sealed = crypto::seal_project(key, &link.cloud_id, next, sealed_name, &local);
        match u
            .api
            .update_project(&link.cloud_id, link.version, *generation, &sealed)?
        {
            None => results.push(PushResult::CloudIsNewer(name)),
            Some(row) => {
                if let Some(stored) = st.link_mut(&name) {
                    stored.version = row.version;
                    stored.base = local;
                }
                state::save(home, &st)?;
                results.push(PushResult::Pushed {
                    project: name,
                    version: row.version,
                });
            }
        }
    }
    Ok(results)
}

pub trait Resolver {
    /// One choice per conflict (`None` removes the variable). `Ok(None)`
    /// means stop without any change.
    fn resolve(
        &mut self,
        project: &str,
        conflicts: &[Conflict],
    ) -> Result<Option<Vec<Option<String>>>, String>;
}

#[derive(Debug, PartialEq)]
pub enum PullResult {
    Updated { project: String, changes: usize },
    Added(String),
    UpToDate(String),
    Stopped { project: String, conflicts: usize },
    Skipped { project: String, reason: String },
}

const CHANGED_WHILE_WORKING: &str =
    "The master file changed while Keyman worked. Keyman did not write anything. Run 'keyman pull' again.";

pub fn pull(
    home: &Path,
    u: &Unlocked,
    project: Option<&str>,
    local_name: Option<&str>,
    resolver: &mut dyn Resolver,
) -> Result<Vec<PullResult>, String> {
    let vaults = Vaults::load(u)?;
    let accepted = groups::accept_pending(u, &vaults)?;
    for (group, email) in &accepted.given {
        println!("Gave {email} access to {group}.");
    }
    for warning in &accepted.warnings {
        eprintln!("{warning}");
    }
    let mut ids: Vec<String> = vaults.by_id.keys().cloned().collect();
    ids.sort();
    let rows = u.api.projects(&ids)?;
    let mut st = state::load(home)?;
    let mut results = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut matched = false;

    for row in &rows {
        seen.insert(row.id.clone());
        let linked = st.link_by_cloud_id(&row.id).cloned();
        if let (Some(existing), Some(wanted)) = (&linked, project) {
            if existing.project != wanted {
                continue;
            }
        }
        let opened = vaults
            .key(&row.vault_id)
            .map_err(|reason| cannot_open(&reason))
            .and_then(|(key, generation)| {
                crypto::open_project(key, &row.id, row.version, &row.sealed)
                    .map_err(|reason| open_failure(*generation, row.key_generation, reason))
            });
        let (cloud_name, cloud_vars) = match opened {
            Ok(opened) => opened,
            Err(reason) => {
                matched = true;
                results.push(PullResult::Skipped {
                    project: linked.map_or_else(|| row.id.clone(), |l| l.project),
                    reason,
                });
                continue;
            }
        };
        let (path, yaml) = master::read_master(home)?;

        match linked {
            Some(link) => {
                matched = true;
                let name = link.project.clone();
                let step = linked_step(&yaml, &link, row.version, &cloud_vars, resolver)?;
                match step {
                    Step::Skipped(reason) => results.push(PullResult::Skipped {
                        project: name,
                        reason,
                    }),
                    Step::Stopped(conflicts) => results.push(PullResult::Stopped {
                        project: name,
                        conflicts,
                    }),
                    Step::Done { yaml: new, changes } => {
                        if let Some(text) = new {
                            if master::read_master(home)?.1 != yaml {
                                results.push(PullResult::Skipped {
                                    project: name,
                                    reason: CHANGED_WHILE_WORKING.to_string(),
                                });
                                continue;
                            }
                            // (a) the master file first. If this fails, nothing else changes.
                            master::atomic_write(&path, text.as_bytes())?;
                        }
                        // (b) then the link: the base is the cloud copy, not the merge.
                        let moved = link.version != row.version
                            || link.vault_id != row.vault_id
                            || link.cloud_name != cloud_name
                            || !same_vars(&link.base, &cloud_vars);
                        if moved {
                            if let Some(stored) = st.link_mut(&name) {
                                stored.version = row.version;
                                stored.vault_id = row.vault_id.clone();
                                stored.cloud_name = cloud_name.clone();
                                stored.base = cloud_vars;
                            }
                            state::save(home, &st)?;
                        }
                        results.push(if changes > 0 {
                            PullResult::Updated {
                                project: name,
                                changes,
                            }
                        } else {
                            PullResult::UpToDate(name)
                        });
                    }
                }
            }
            None => {
                if project.is_some_and(|wanted| wanted != cloud_name) {
                    continue;
                }
                matched = true;
                let target = local_name.map_or_else(|| cloud_name.clone(), str::to_string);
                let taken = st.link(&target).is_some();
                // Only a project named on the command line, with no --as, joins a local one.
                let adopting = project.is_some()
                    && local_name.is_none()
                    && !taken
                    && master::project_names(&yaml)?.contains(&target);
                let step = if adopting {
                    adopt_step(&yaml, &target, &cloud_vars, resolver)?
                } else {
                    added_step(&yaml, &target, taken, &cloud_vars)?
                };
                match step {
                    Step::Skipped(reason) => results.push(PullResult::Skipped {
                        project: target,
                        reason,
                    }),
                    Step::Stopped(conflicts) => results.push(PullResult::Stopped {
                        project: target,
                        conflicts,
                    }),
                    Step::Done { yaml: new, changes } => {
                        if let Some(text) = new {
                            if master::read_master(home)?.1 != yaml {
                                results.push(PullResult::Skipped {
                                    project: target,
                                    reason: CHANGED_WHILE_WORKING.to_string(),
                                });
                                continue;
                            }
                            master::atomic_write(&path, text.as_bytes())?;
                        }
                        // The base is the cloud copy, so the next push sends local-only changes.
                        st.set_link(Link {
                            project: target.clone(),
                            cloud_id: row.id.clone(),
                            vault_id: row.vault_id.clone(),
                            version: row.version,
                            base: cloud_vars,
                            cloud_name,
                        });
                        state::save(home, &st)?;
                        results.push(if !adopting {
                            PullResult::Added(target)
                        } else if changes > 0 {
                            PullResult::Updated {
                                project: target,
                                changes,
                            }
                        } else {
                            PullResult::UpToDate(target)
                        });
                    }
                }
            }
        }
    }

    // Links whose cloud project is gone, or whose vault this user left.
    for link in st.links.clone() {
        if seen.contains(&link.cloud_id) || project.is_some_and(|wanted| wanted != link.project) {
            continue;
        }
        matched = true;
        results.push(PullResult::Skipped {
            reason: format!(
                "Project '{0}' is not in the cloud now. Run 'keyman unlink {0}' to remove the link.",
                link.project
            ),
            project: link.project,
        });
    }
    if !matched {
        if let Some(wanted) = project {
            return Err(format!(
                "No project named '{wanted}' was found in the cloud."
            ));
        }
    }
    Ok(results)
}

/// Merges base, local and cloud. `Ok(None)` means the resolver stopped.
pub fn pull_merge(
    project: &str,
    base: &[(String, String)],
    local: &[(String, String)],
    cloud: &[(String, String)],
    resolver: &mut dyn Resolver,
) -> Result<Option<Variables>, String> {
    let merged = merge::merge(base, local, cloud);
    if merged.conflicts.is_empty() {
        return Ok(Some(merged.result));
    }
    let Some(choices) = resolver.resolve(project, &merged.conflicts)? else {
        return Ok(None);
    };
    if choices.len() != merged.conflicts.len() {
        return Err("Keyman got the wrong number of answers for the conflicts.".to_string());
    }
    Ok(Some(merge::resolve(merged, &choices)))
}

/// The master file text with `project` set to exactly `variables`.
pub fn write_project(
    yaml: &str,
    project: &str,
    exists: bool,
    variables: &[(String, String)],
) -> Result<String, String> {
    let existing = if exists {
        master::project_variables(yaml, project)?
    } else {
        Vec::new()
    };
    let plan = save::plan(&existing, variables, true);
    save::update_project(yaml, project, &plan, exists)
}

#[derive(Debug, PartialEq)]
pub enum LinkStatus {
    Synced,
    LocalChanges,
    Missing,
}

/// Compares each linked project with its last-synced copy. Offline.
pub fn local_status(home: &Path) -> Result<Vec<(String, LinkStatus)>, String> {
    let st = state::load(home)?;
    if st.links.is_empty() {
        return Ok(Vec::new());
    }
    let (_, yaml) = master::read_master(home)?;
    let names = master::project_names(&yaml)?;
    Ok(st
        .links
        .iter()
        .map(|link| {
            let status = if !names.contains(&link.project) {
                LinkStatus::Missing
            } else {
                match master::project_variables(&yaml, &link.project) {
                    Ok(local) if same_vars(&local, &link.base) => LinkStatus::Synced,
                    _ => LinkStatus::LocalChanges,
                }
            };
            (link.project.clone(), status)
        })
        .collect())
}

pub fn delete_cloud(home: &Path, u: &Unlocked, project: &str) -> Result<(), String> {
    let mut st = state::load(home)?;
    let link = st
        .link(project)
        .cloned()
        .ok_or_else(|| not_linked(project))?;
    u.api.delete_project(&link.cloud_id)?;
    // The server ignores a delete that the user may not do, so check.
    if u.api.project(&link.cloud_id)?.is_some() {
        return Err(format!(
            "Keyman cannot delete '{project}' from the cloud. Only the owner of the vault can do this."
        ));
    }
    st.remove_link(project);
    state::save(home, &st)
}

/// What to do with one project after the merge is computed.
#[derive(Debug, PartialEq)]
enum Step {
    Skipped(String),
    Stopped(usize),
    /// `yaml` is the new master file text, or `None` when nothing changes.
    Done {
        yaml: Option<String>,
        changes: usize,
    },
}

/// A refusal from `write_project` as one short sentence, then the detail.
fn refused(project: &str, detail: &str) -> String {
    format!(
        "Keyman cannot update '{project}' safely.\n{}",
        detail.replace("\n\n", "\n")
    )
}

/// Merge for a project that has a base. Asks the resolver only when the
/// master file layout allows the write.
fn merge_step(
    yaml: &str,
    name: &str,
    base: &[(String, String)],
    local: &[(String, String)],
    cloud: &[(String, String)],
    resolver: &mut dyn Resolver,
) -> Result<Step, String> {
    let conflicts = merge::merge(base, local, cloud).conflicts.len();
    if conflicts > 0 {
        if let Err(detail) = write_project(yaml, name, true, local) {
            return Ok(Step::Skipped(refused(name, &detail)));
        }
    }
    let Some(merged) = pull_merge(name, base, local, cloud, resolver)? else {
        return Ok(Step::Stopped(conflicts));
    };
    let changes = count_changes(local, &merged);
    if changes == 0 {
        return Ok(Step::Done {
            yaml: None,
            changes: 0,
        });
    }
    Ok(match write_project(yaml, name, true, &merged) {
        Ok(text) => Step::Done {
            yaml: Some(text),
            changes,
        },
        Err(detail) => Step::Skipped(refused(name, &detail)),
    })
}

fn linked_step(
    yaml: &str,
    link: &Link,
    cloud_version: i64,
    cloud: &[(String, String)],
    resolver: &mut dyn Resolver,
) -> Result<Step, String> {
    let names = master::project_names(yaml)?;
    if !names.contains(&link.project) {
        return Ok(Step::Skipped(skip_reason_missing(&link.project)));
    }
    let local = match master::project_variables(yaml, &link.project) {
        Ok(local) => local,
        Err(reason) => return Ok(Step::Skipped(reason)),
    };
    if cloud_version == link.version && same_vars(&local, &link.base) {
        return Ok(Step::Done {
            yaml: None,
            changes: 0,
        });
    }
    merge_step(yaml, &link.project, &link.base, &local, cloud, resolver)
}

/// Joins an existing local project to a cloud project of the same name. The
/// base is empty: equal values match, different values are conflicts, and
/// variables on one side only are kept.
fn adopt_step(
    yaml: &str,
    name: &str,
    cloud: &[(String, String)],
    resolver: &mut dyn Resolver,
) -> Result<Step, String> {
    let local = match master::project_variables(yaml, name) {
        Ok(local) => local,
        Err(reason) => return Ok(Step::Skipped(reason)),
    };
    merge_step(yaml, name, &[], &local, cloud, resolver)
}

fn added_step(
    yaml: &str,
    name: &str,
    linked: bool,
    cloud: &[(String, String)],
) -> Result<Step, String> {
    let exists = master::project_names(yaml)?.iter().any(|n| n == name);
    if exists || linked {
        return Ok(Step::Skipped(format!(
            "A local project named '{name}' exists. Run 'keyman pull {name}' to merge it with the local project, or use --as <name>."
        )));
    }
    Ok(match write_project(yaml, name, false, cloud) {
        Ok(text) => Step::Done {
            yaml: Some(text),
            changes: cloud.len(),
        },
        Err(detail) => Step::Skipped(refused(name, &detail)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::master;
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn v(pairs: &[(&str, &str)]) -> Variables {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    struct Fixed(Option<Vec<Option<String>>>, usize);

    impl Resolver for Fixed {
        fn resolve(
            &mut self,
            _project: &str,
            _conflicts: &[Conflict],
        ) -> Result<Option<Vec<Option<String>>>, String> {
            self.1 += 1;
            Ok(self.0.clone())
        }
    }

    fn temp_home() -> PathBuf {
        let home = env::temp_dir().join(format!(
            "keyman-sync-test-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        home
    }

    fn home_with(master_text: &str, links: Vec<Link>) -> PathBuf {
        let home = temp_home();
        let file = home.join("master.yml");
        fs::write(&file, master_text).unwrap();
        fs::write(home.join(".keyman_config"), file.to_string_lossy().as_ref()).unwrap();
        let state = state::State {
            links,
            ..Default::default()
        };
        state::save(&home, &state).unwrap();
        home
    }

    fn link_of(project: &str, base: Variables) -> Link {
        Link {
            project: project.to_string(),
            cloud_id: format!("cloud-{project}"),
            vault_id: "vault".to_string(),
            version: 1,
            base,
            cloud_name: project.to_string(),
        }
    }

    #[test]
    fn pull_merge_without_conflicts_returns_result() {
        let mut r = Fixed(None, 0);
        let out = pull_merge(
            "app",
            &v(&[("A", "1")]),
            &v(&[("A", "1"), ("B", "2")]),
            &v(&[("A", "3")]),
            &mut r,
        )
        .unwrap();
        assert_eq!(out, Some(v(&[("A", "3"), ("B", "2")])));
        assert_eq!(r.1, 0);
    }

    #[test]
    fn pull_merge_calls_resolver_on_conflict() {
        let mut r = Fixed(Some(vec![Some("x".into())]), 0);
        let out = pull_merge(
            "app",
            &v(&[("A", "1")]),
            &v(&[("A", "2")]),
            &v(&[("A", "3")]),
            &mut r,
        )
        .unwrap();
        assert_eq!(out, Some(v(&[("A", "x")])));
        assert_eq!(r.1, 1);
    }

    #[test]
    fn pull_merge_stops_when_resolver_declines() {
        let mut r = Fixed(None, 0);
        let out = pull_merge(
            "app",
            &v(&[("A", "1")]),
            &v(&[("A", "2")]),
            &v(&[("A", "3")]),
            &mut r,
        )
        .unwrap();
        assert_eq!(out, None);
    }

    #[test]
    fn pull_merge_rejects_wrong_number_of_choices() {
        let mut r = Fixed(Some(vec![]), 0);
        let out = pull_merge(
            "app",
            &v(&[("A", "1")]),
            &v(&[("A", "2")]),
            &v(&[("A", "3")]),
            &mut r,
        );
        assert!(out.is_err());
    }

    #[test]
    fn write_project_adds_new_project() {
        let yaml = "# top\nother:\n  X: \"1\"\n";
        let out = write_project(yaml, "app", false, &v(&[("A", "1")])).unwrap();
        assert!(out.starts_with("# top\nother:\n  X: \"1\"\n"));
        assert_eq!(
            master::project_variables(&out, "app").unwrap(),
            v(&[("A", "1")])
        );
    }

    #[test]
    fn write_project_replaces_variables() {
        let yaml = "# keep me\napp:\n  A: \"1\"\n  OLD: \"x\"\n# other note\nother:\n  X: \"1\"\n";
        let out = write_project(yaml, "app", true, &v(&[("A", "2"), ("B", "3")])).unwrap();
        assert_eq!(
            master::project_variables(&out, "app").unwrap(),
            v(&[("A", "2"), ("B", "3")])
        );
        assert!(!out.contains("OLD"));
        assert!(out.contains("# keep me"));
        assert!(out.contains("# other note"));
        assert_eq!(
            master::project_variables(&out, "other").unwrap(),
            v(&[("X", "1")])
        );
    }

    #[test]
    fn refused_layout_writes_nothing() {
        let yaml = "app: {A: \"1\"}\n";
        let copy = yaml.to_string();
        assert!(write_project(yaml, "app", true, &v(&[("A", "2")])).is_err());
        assert_eq!(yaml, copy);

        let link = link_of("app", v(&[("A", "1")]));
        let mut r = Fixed(None, 0);
        let step = linked_step(yaml, &link, 2, &v(&[("A", "2")]), &mut r).unwrap();
        assert!(matches!(step, Step::Skipped(_)), "{step:?}");
    }

    #[test]
    fn missing_local_project_is_skipped_with_message() {
        let home = home_with("other:\n  X: \"1\"\n", vec![link_of("app", v(&[]))]);
        assert_eq!(
            local_status(&home).unwrap(),
            vec![("app".to_string(), LinkStatus::Missing)]
        );
        assert!(skip_reason_missing("app").contains("keyman unlink app"));
        let link = link_of("app", v(&[]));
        let mut r = Fixed(None, 0);
        let step = linked_step("other:\n  X: \"1\"\n", &link, 2, &v(&[]), &mut r).unwrap();
        assert_eq!(step, Step::Skipped(skip_reason_missing("app")));
    }

    #[test]
    fn local_status_detects_local_changes() {
        let home = home_with(
            "app:\n  A: \"1\"\nsame:\n  B: \"2\"\n",
            vec![
                link_of("app", v(&[("A", "0")])),
                link_of("same", v(&[("B", "2")])),
            ],
        );
        assert_eq!(
            local_status(&home).unwrap(),
            vec![
                ("app".to_string(), LinkStatus::LocalChanges),
                ("same".to_string(), LinkStatus::Synced)
            ]
        );
    }

    #[test]
    fn local_status_ignores_variable_order() {
        let home = home_with(
            "app:\n  B: \"2\"\n  A: \"1\"\n",
            vec![link_of("app", v(&[("A", "1"), ("B", "2")]))],
        );
        assert_eq!(local_status(&home).unwrap()[0].1, LinkStatus::Synced);
    }

    #[test]
    fn linked_step_up_to_date_when_nothing_changed() {
        let yaml = "app:\n  A: \"1\"\n";
        let link = link_of("app", v(&[("A", "1")]));
        let mut r = Fixed(None, 0);
        let step = linked_step(yaml, &link, 1, &v(&[("A", "1")]), &mut r).unwrap();
        assert_eq!(
            step,
            Step::Done {
                yaml: None,
                changes: 0
            }
        );
    }

    #[test]
    fn linked_step_keeps_local_only_changes() {
        // The cloud did not change, so there is nothing to write.
        let yaml = "app:\n  A: \"2\"\n";
        let link = link_of("app", v(&[("A", "1")]));
        let mut r = Fixed(None, 0);
        let step = linked_step(yaml, &link, 1, &v(&[("A", "1")]), &mut r).unwrap();
        assert_eq!(
            step,
            Step::Done {
                yaml: None,
                changes: 0
            }
        );
    }

    #[test]
    fn linked_step_writes_cloud_changes() {
        let yaml = "app:\n  A: \"1\"\n";
        let link = link_of("app", v(&[("A", "1")]));
        let mut r = Fixed(None, 0);
        let step = linked_step(yaml, &link, 2, &v(&[("A", "1"), ("B", "2")]), &mut r).unwrap();
        let Step::Done {
            yaml: Some(out),
            changes,
        } = step
        else {
            panic!("expected a write");
        };
        assert_eq!(changes, 1);
        assert_eq!(
            master::project_variables(&out, "app").unwrap(),
            v(&[("A", "1"), ("B", "2")])
        );
    }

    #[test]
    fn linked_step_stops_and_counts_conflicts() {
        let yaml = "app:\n  A: \"2\"\n";
        let link = link_of("app", v(&[("A", "1")]));
        let mut r = Fixed(None, 0);
        let step = linked_step(yaml, &link, 2, &v(&[("A", "3")]), &mut r).unwrap();
        assert_eq!(step, Step::Stopped(1));
    }

    #[test]
    fn added_step_skips_a_name_that_exists() {
        let yaml = "app:\n  A: \"1\"\n";
        let step = added_step(yaml, "app", false, &v(&[("A", "9")])).unwrap();
        assert_eq!(
            step,
            Step::Skipped(
                "A local project named 'app' exists. Run 'keyman pull app' to merge it with the local project, or use --as <name>."
                    .to_string()
            )
        );
        let step = added_step("other:\n  X: \"1\"\n", "app", true, &v(&[])).unwrap();
        assert!(matches!(step, Step::Skipped(_)));
    }

    #[test]
    fn added_step_writes_a_new_project() {
        let step = added_step("other:\n  X: \"1\"\n", "app", false, &v(&[("A", "9")])).unwrap();
        let Step::Done {
            yaml: Some(out), ..
        } = step
        else {
            panic!("expected a write");
        };
        assert_eq!(
            master::project_variables(&out, "app").unwrap(),
            v(&[("A", "9")])
        );
    }

    #[test]
    fn a_new_key_generation_is_not_a_decrypt_error() {
        assert_eq!(
            open_failure(1, 2, "the data is not valid".to_string()),
            "The group key changed. Run 'keyman pull' again."
        );
        assert_eq!(
            open_failure(2, 2, "the data is not valid".to_string()),
            "Keyman cannot open this project. the data is not valid"
        );
    }

    #[test]
    fn link_refuses_a_name_that_is_in_the_cloud() {
        let names = vec!["other".to_string(), "app".to_string()];
        let error = cloud_name_is_free(&names, "app").unwrap_err();
        assert_eq!(
            error,
            "A project named 'app' is in the cloud. Run 'keyman pull app' to get it."
        );
        assert!(cloud_name_is_free(&names, "new").is_ok());
        assert!(cloud_name_is_free(&[], "app").is_ok());
    }

    #[test]
    fn ensure_unlinked_checks_state_offline() {
        let home = home_with("app:\n  A: \"1\"\n", vec![link_of("app", v(&[]))]);
        assert_eq!(
            ensure_unlinked(&home, "app").unwrap_err(),
            "Project 'app' is already linked."
        );
        assert!(ensure_unlinked(&home, "other").is_ok());
    }

    #[test]
    fn adopt_merges_with_no_conflict_when_values_match() {
        let yaml = "app:\n  A: \"1\"\n  L: \"local\"\n";
        let mut r = Fixed(None, 0);
        let step = adopt_step(yaml, "app", &v(&[("A", "1"), ("C", "cloud")]), &mut r).unwrap();
        let Step::Done {
            yaml: Some(out),
            changes,
        } = step
        else {
            panic!("expected a write");
        };
        assert_eq!(r.1, 0);
        assert_eq!(changes, 1);
        assert_eq!(
            master::project_variables(&out, "app").unwrap(),
            v(&[("A", "1"), ("L", "local"), ("C", "cloud")])
        );
    }

    #[test]
    fn adopt_sends_a_differing_value_to_the_resolver() {
        let yaml = "app:\n  A: \"1\"\n";
        let mut r = Fixed(Some(vec![Some("x".into())]), 0);
        let step = adopt_step(yaml, "app", &v(&[("A", "2")]), &mut r).unwrap();
        assert_eq!(r.1, 1);
        let Step::Done {
            yaml: Some(out), ..
        } = step
        else {
            panic!("expected a write");
        };
        assert_eq!(
            master::project_variables(&out, "app").unwrap(),
            v(&[("A", "x")])
        );
        let mut stop = Fixed(None, 0);
        assert_eq!(
            adopt_step(yaml, "app", &v(&[("A", "2")]), &mut stop).unwrap(),
            Step::Stopped(1)
        );
    }

    #[test]
    fn pull_all_still_skips_a_name_collision() {
        let step = added_step("app:\n  A: \"1\"\n", "app", false, &v(&[("A", "2")])).unwrap();
        let Step::Skipped(reason) = step else {
            panic!("expected skip");
        };
        assert!(reason.contains("keyman pull app"), "{reason}");
    }

    #[test]
    fn layout_is_checked_before_the_resolver_is_asked() {
        let yaml = "app: {A: \"1\"}\n";
        let link = link_of("app", v(&[("A", "0")]));
        let mut r = Fixed(Some(vec![Some("x".into())]), 0);
        let step = linked_step(yaml, &link, 2, &v(&[("A", "2")]), &mut r).unwrap();
        let Step::Skipped(reason) = step else {
            panic!("expected skip");
        };
        assert_eq!(r.1, 0);
        assert!(
            reason.starts_with("Keyman cannot update 'app' safely.\n"),
            "{reason}"
        );
        assert!(!reason.contains("\n\n"));
    }
}
