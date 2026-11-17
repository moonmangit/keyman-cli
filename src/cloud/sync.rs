//! Link, push, pull, status and delete for cloud projects.
//!
//! The pure parts (merge, write, status) are separate functions so they can be
//! tested without a network. Order matters on pull: compute everything first,
//! write the master file, and only then change the link and the state file.

use super::account::Unlocked;
use super::api::Membership;
use super::crypto::{self, Key};
use super::merge::{self, Conflict};
use super::state::{self, Link};
use crate::master::{self, Variables};
use crate::save;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The vault keys of one user: vault ID to (vault key, key generation).
pub struct Vaults {
    by_id: HashMap<String, (Key, i64)>,
}

impl Vaults {
    /// Unwraps the key of every vault the user belongs to. A vault whose key
    /// does not unwrap is left out, and `key` reports it when it is needed.
    pub fn load(u: &Unlocked) -> Result<Vaults, String> {
        Ok(Vaults::from_memberships(u, &u.api.my_memberships()?))
    }

    fn from_memberships(u: &Unlocked, memberships: &[Membership]) -> Vaults {
        let by_id = memberships
            .iter()
            .filter_map(|member| {
                let key = crypto::unwrap(&u.pair, &member.wrapped_key).ok()?;
                Some((member.vault_id.clone(), (key, member.key_generation)))
            })
            .collect();
        Vaults { by_id }
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

/// The vault of the group with this name, among the vaults the user is in.
fn group_vault(memberships: &[Membership], vaults: &Vaults, name: &str) -> Result<String, String> {
    for member in memberships.iter().filter(|m| m.vaults.kind != "personal") {
        let Some(sealed) = &member.vaults.name_sealed else {
            continue;
        };
        let Ok((key, _)) = vaults.key(&member.vault_id) else {
            continue;
        };
        let aad = crypto::vault_name_aad(&member.vault_id);
        if let Ok(plain) = crypto::open(key, &aad, sealed) {
            if plain.as_slice() == name.as_bytes() {
                return Ok(member.vault_id.clone());
            }
        }
    }
    Err(format!("You are not in a group named '{name}'."))
}

/// Task 8 moves this to `groups.rs`. It returns `(group, email)` for each
/// person who got access.
pub fn accept_pending(_u: &Unlocked, _vaults: &Vaults) -> Result<Vec<(String, String)>, String> {
    Ok(Vec::new())
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

pub fn link(home: &Path, u: &Unlocked, project: &str, group: Option<&str>) -> Result<Link, String> {
    let (_, yaml) = master::read_master(home)?;
    let variables = master::project_variables(&yaml, project)?;
    let mut st = state::load(home)?;
    if st.link(project).is_some() {
        return Err(format!("Project '{project}' is already linked."));
    }
    let memberships = u.api.my_memberships()?;
    let vaults = Vaults::from_memberships(u, &memberships);
    let vault_id = match group {
        Some(name) => group_vault(&memberships, &vaults, name)?,
        None => personal_in(&memberships)?,
    };
    let (key, generation) = vaults.key(&vault_id)?;
    let row = u.api.insert_project(&vault_id, *generation, |id| {
        crypto::seal_project(key, id, 1, project, &variables)
    })?;
    let link = Link {
        project: project.to_string(),
        cloud_id: row.id,
        vault_id,
        version: row.version,
        base: variables,
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
        let sealed = crypto::seal_project(key, &link.cloud_id, next, &name, &local);
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
    "The master file changed while Keyman worked. Nothing was written. Run 'keyman pull' again.";

pub fn pull(
    home: &Path,
    u: &Unlocked,
    project: Option<&str>,
    local_name: Option<&str>,
    resolver: &mut dyn Resolver,
) -> Result<Vec<PullResult>, String> {
    let vaults = Vaults::load(u)?;
    for (group, email) in accept_pending(u, &vaults)? {
        println!("Gave {email} access to {group}.");
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
            .and_then(|(key, _)| crypto::open_project(key, &row.id, row.version, &row.sealed));
        let (cloud_name, cloud_vars) = match opened {
            Ok(opened) => opened,
            Err(reason) => {
                matched = true;
                results.push(PullResult::Skipped {
                    project: linked.map_or_else(|| row.id.clone(), |l| l.project),
                    reason: format!("Keyman cannot open this project. {reason}"),
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
                            || !same_vars(&link.base, &cloud_vars);
                        if moved {
                            if let Some(stored) = st.link_mut(&name) {
                                stored.version = row.version;
                                stored.vault_id = row.vault_id.clone();
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
                let target = local_name.map_or(cloud_name, str::to_string);
                let taken = st.link(&target).is_some();
                match added_step(&yaml, &target, taken, &cloud_vars)? {
                    Step::Skipped(reason) => results.push(PullResult::Skipped {
                        project: target,
                        reason,
                    }),
                    Step::Done {
                        yaml: Some(text), ..
                    } => {
                        if master::read_master(home)?.1 != yaml {
                            results.push(PullResult::Skipped {
                                project: target,
                                reason: CHANGED_WHILE_WORKING.to_string(),
                            });
                            continue;
                        }
                        master::atomic_write(&path, text.as_bytes())?;
                        st.set_link(Link {
                            project: target.clone(),
                            cloud_id: row.id.clone(),
                            vault_id: row.vault_id.clone(),
                            version: row.version,
                            base: cloud_vars,
                        });
                        state::save(home, &st)?;
                        results.push(PullResult::Added(target));
                    }
                    Step::Done { yaml: None, .. } | Step::Stopped(_) => {}
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
    let Some(merged) = pull_merge(&link.project, &link.base, &local, cloud, resolver)? else {
        let conflicts = merge::merge(&link.base, &local, cloud).conflicts.len();
        return Ok(Step::Stopped(conflicts));
    };
    let changes = count_changes(&local, &merged);
    if changes == 0 {
        return Ok(Step::Done {
            yaml: None,
            changes: 0,
        });
    }
    Ok(match write_project(yaml, &link.project, true, &merged) {
        Ok(text) => Step::Done {
            yaml: Some(text),
            changes,
        },
        Err(reason) => Step::Skipped(reason),
    })
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
            "A local project named '{name}' exists. Use --as <name>."
        )));
    }
    Ok(match write_project(yaml, name, false, cloud) {
        Ok(text) => Step::Done {
            yaml: Some(text),
            changes: cloud.len(),
        },
        Err(reason) => Step::Skipped(reason),
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
            Step::Skipped("A local project named 'app' exists. Use --as <name>.".to_string())
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
}
