//! Link, push, pull, status and delete for cloud projects.
//!
//! The pure parts (merge, write, status) are separate functions so they can be
//! tested without a network. Order matters on pull: compute everything first,
//! write the master file, and only then change the link and the state file.

use super::account::{self, Unlocked};
use super::api::{CloudProject, Membership};
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
    /// The vaults whose key opened as a personal key. Such a key never goes
    /// to another person (see `groups::accept_pending`).
    personal: HashSet<String>,
    memberships: Vec<Membership>,
}

impl Vaults {
    /// Unwraps the key of every vault the user belongs to. A vault whose key
    /// does not unwrap is left out, and `key` reports it when it is needed.
    pub fn load(u: &Unlocked) -> Result<Vaults, String> {
        Ok(Vaults::from_memberships(u, &u.api.my_memberships()?))
    }

    fn from_memberships(u: &Unlocked, memberships: &[Membership]) -> Vaults {
        Vaults::from_pair(&u.pair, memberships, u.personal_vault.as_deref())
    }

    /// The same, from a key pair. The key is stored with the generation of
    /// the membership it was unwrapped from. A wrapped key must be bound to
    /// its own vault and generation, so the server cannot move a key from
    /// one vault to another.
    ///
    /// A personal vault (the server says "personal", or it is `personal`, the
    /// one this computer saved) must have a personal key, which only this key
    /// pair can make. So the server cannot give the personal vault a key it
    /// knows, also when it labels the personal vault as a group.
    pub(crate) fn from_pair(
        pair: &KeyPair,
        memberships: &[Membership],
        personal: Option<&str>,
    ) -> Vaults {
        let mut personal_ids = HashSet::new();
        // A vault ID in more than one row comes from the server: such a vault
        // gets no key, so a second row cannot replace the key of the first.
        let mut seen = HashSet::new();
        let doubled: HashSet<&str> = memberships
            .iter()
            .filter(|member| !seen.insert(member.vault_id.as_str()))
            .map(|member| member.vault_id.as_str())
            .collect();
        let by_id = memberships
            .iter()
            .filter(|member| !doubled.contains(member.vault_id.as_str()))
            .filter_map(|member| {
                let is_personal =
                    member.vaults.kind == "personal" || personal == Some(member.vault_id.as_str());
                let unwrap = if is_personal {
                    crypto::unwrap_personal
                } else {
                    crypto::unwrap
                };
                let key = unwrap(
                    pair,
                    &member.wrapped_key,
                    &member.vault_id,
                    member.key_generation,
                )
                .ok()?;
                if is_personal {
                    personal_ids.insert(member.vault_id.clone());
                }
                Some((member.vault_id.clone(), (key, member.key_generation)))
            })
            .collect();
        Vaults {
            by_id,
            personal: personal_ids,
            memberships: memberships.to_vec(),
        }
    }

    /// True when the key of this vault opened as a personal key.
    pub fn is_personal(&self, vault_id: &str) -> bool {
        self.personal.contains(vault_id)
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

pub(crate) const NO_PERSONAL: &str = "You have no personal vault. Run 'keyman login' first.";
pub(crate) const PERSONAL_CHANGED: &str = "The server changed your personal vault. Keyman stops.";
pub(crate) const PERSONAL_KEY_BAD: &str =
    "The key of your personal vault is not valid. Keyman stops.";
const TWO_PERSONAL: &str = "The server shows more than one personal vault. Keyman stops.";

/// The personal vault of `user_id` in the memberships. `pinned` is the
/// personal vault that this computer saved before: a different one, or a
/// second one, is refused.
pub(crate) fn personal_in(
    memberships: &[Membership],
    user_id: &str,
    pinned: Option<&str>,
) -> Result<String, String> {
    let found: Vec<&str> = memberships
        .iter()
        .filter(|member| {
            member.vaults.kind == "personal"
                && member.vaults.owner_id == user_id
                && member.user_id == user_id
        })
        .map(|member| member.vault_id.as_str())
        .collect();
    match (pinned, found.as_slice()) {
        (Some(pinned), [one]) if *one == pinned => Ok(pinned.to_string()),
        (Some(_), _) => Err(PERSONAL_CHANGED.to_string()),
        (None, [one]) => Ok(one.to_string()),
        (None, []) => Err(NO_PERSONAL.to_string()),
        (None, _) => Err(TWO_PERSONAL.to_string()),
    }
}

/// The personal vault, checked against the one this computer saved. Its key
/// must be a personal key. The first time, Keyman saves it.
fn personal_vault(
    home: &Path,
    u: &Unlocked,
    memberships: &[Membership],
    vaults: &Vaults,
) -> Result<String, String> {
    let pinned = account::pinned_vault(&state::load(home)?, &u.user_id);
    let id = personal_in(memberships, &u.user_id, pinned.as_deref())?;
    if vaults.key(&id).is_err() {
        return Err(PERSONAL_KEY_BAD.to_string());
    }
    account::pin_personal_vault(home, &u.user_id, &id)?;
    Ok(id)
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
    let memberships = u.api.my_memberships()?;
    let vaults = Vaults::from_memberships(u, &memberships);
    let vault_id = match group {
        Some(name) => group_vault(&vaults, name)?,
        None => personal_vault(home, u, &memberships, &vaults)?,
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
    // Read the state again: `personal_vault` can save it above.
    let mut st = state::load(home)?;
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

/// `pull <project>` joins one unlinked cloud project. When two have the
/// name, Keyman cannot know which one is right.
fn one_cloud_project_named(unlinked_names: &[String], wanted: &str) -> Result<(), String> {
    if unlinked_names.iter().filter(|name| *name == wanted).count() > 1 {
        return Err(format!(
            "More than one project in the cloud has the name {wanted}."
        ));
    }
    Ok(())
}

/// The server must never give a version that is older than the last sync:
/// the old sealed copy still opens, so it is a silent roll back.
fn older_in_cloud(project: &str, link_version: i64, cloud_version: i64) -> Option<String> {
    (cloud_version < link_version).then(|| {
        format!(
            "The cloud has an older version of {project} than this computer. Keyman did not change it."
        )
    })
}

/// A linked project stays in its vault. Keyman never moves a project, so a
/// row in another vault comes from the server: if Keyman followed it, the
/// next push would seal the project with the key of that vault, which the
/// server can know.
fn moved_to_other_vault(project: &str, link_vault: &str, cloud_vault: &str) -> Option<String> {
    (link_vault != cloud_vault)
        .then(|| format!("The server moved {project} to another vault. Keyman did not change it."))
}

/// A push changes the version from `link_version` to exactly one more.
fn check_push_reply(project: &str, link_version: i64, reply_version: i64) -> Result<(), String> {
    if reply_version == link_version + 1 {
        Ok(())
    } else {
        Err(format!(
            "The server gave a wrong version for {project}. Keyman did not change the link. Run 'keyman pull {project}'."
        ))
    }
}

/// The result of a push that changed no row. `row` is the cloud project
/// read again: when it is gone, the project was deleted or the user left.
fn not_pushed(project: &str, row: Option<&CloudProject>) -> PushResult {
    match row {
        Some(_) => PushResult::CloudIsNewer(project.to_string()),
        None => PushResult::Skipped {
            project: project.to_string(),
            reason: format!(
                "The project {project} is not in the cloud now. Run 'keyman unlink {project}'."
            ),
        },
    }
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
            None => {
                let row = u.api.project(&link.cloud_id)?;
                results.push(not_pushed(&name, row.as_ref()));
            }
            Some(row) => {
                if let Err(reason) = check_push_reply(&name, link.version, row.version) {
                    results.push(PushResult::Skipped {
                        project: name,
                        reason,
                    });
                    continue;
                }
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

    /// Called once, right after `pull` gave access to invited people and
    /// before any other work: `(group, email)` and warnings about invites.
    fn invites(&mut self, _given: &[(String, String)], _warnings: &[String]) {}

    /// Whether the person stopped the pull. `pull` asks before each cloud
    /// project and then stops with the results so far; the projects after it
    /// stay as they are.
    fn stopped(&self) -> bool {
        false
    }
}

#[derive(Debug, PartialEq)]
pub enum PullResult {
    Updated {
        project: String,
        changes: usize,
    },
    Added(String),
    UpToDate(String),
    Stopped {
        project: String,
        conflicts: usize,
    },
    Skipped {
        project: String,
        reason: String,
    },
    /// A cloud project has the name of a local project that is not linked.
    NameTaken {
        project: String,
    },
}

/// What a pull did: one result for each project, the people who got access
/// to a group (`(group, email)`), and warnings about invites.
#[derive(Debug, PartialEq)]
pub struct PullReport {
    pub results: Vec<PullResult>,
    pub given: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

fn name_taken(project: String) -> PullResult {
    PullResult::NameTaken { project }
}

/// The reason text for a `NameTaken` result.
pub fn name_taken_reason(project: &str) -> String {
    format!(
        "A local project named '{project}' exists. Run 'keyman pull {project}' to merge it with the local project, or use --as <name>."
    )
}

const CHANGED_WHILE_WORKING: &str =
    "The master file changed while Keyman worked. Keyman did not write anything. Run 'keyman pull' again.";

pub fn pull(
    home: &Path,
    u: &Unlocked,
    project: Option<&str>,
    local_name: Option<&str>,
    resolver: &mut dyn Resolver,
) -> Result<PullReport, String> {
    let vaults = Vaults::load(u)?;
    let accepted = groups::accept_pending(u, &vaults)?;
    resolver.invites(&accepted.given, &accepted.warnings);
    let mut ids: Vec<String> = vaults.by_id.keys().cloned().collect();
    ids.sort();
    let rows = u.api.projects(&ids)?;
    let results = pull_rows(home, &vaults, &rows, project, local_name, resolver)?;
    Ok(PullReport {
        results,
        given: accepted.given,
        warnings: accepted.warnings,
    })
}

/// The part of `pull` after the server calls: one result per cloud row, then
/// the links whose cloud project is gone.
fn pull_rows(
    home: &Path,
    vaults: &Vaults,
    rows: &[CloudProject],
    project: Option<&str>,
    local_name: Option<&str>,
    resolver: &mut dyn Resolver,
) -> Result<Vec<PullResult>, String> {
    let mut st = state::load(home)?;
    if let Some(wanted) = project {
        let unlinked: Vec<String> = rows
            .iter()
            .filter(|row| st.link_by_cloud_id(&row.id).is_none())
            .filter_map(|row| {
                let (key, _) = vaults.key(&row.vault_id).ok()?;
                let (name, _) =
                    crypto::open_project(key, &row.id, row.version, &row.sealed).ok()?;
                Some(name)
            })
            .collect();
        one_cloud_project_named(&unlinked, wanted)?;
    }
    let mut results = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut matched = false;

    for row in rows {
        // Stopped: the results so far. The rows after this one are not read,
        // so no link is called gone below.
        if resolver.stopped() {
            return Ok(results);
        }
        seen.insert(row.id.clone());
        let linked = st.link_by_cloud_id(&row.id).cloned();
        if let (Some(existing), Some(wanted)) = (&linked, project) {
            if existing.project != wanted {
                continue;
            }
        }
        if let Some(link) = &linked {
            let refused = moved_to_other_vault(&link.project, &link.vault_id, &row.vault_id)
                .or_else(|| older_in_cloud(&link.project, link.version, row.version));
            if let Some(reason) = refused {
                matched = true;
                results.push(PullResult::Skipped {
                    project: link.project.clone(),
                    reason,
                });
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
                    // `linked_step` never gives this: only `added_step` does.
                    Step::NameTaken => results.push(PullResult::NameTaken { project: name }),
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
                        // The vault never changes here: a row in another vault
                        // is refused above (`moved_to_other_vault`).
                        let moved = link.version != row.version
                            || link.cloud_name != cloud_name
                            || !same_vars(&link.base, &cloud_vars);
                        if moved {
                            if let Some(stored) = st.link_mut(&name) {
                                stored.version = row.version;
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
                    Step::NameTaken => results.push(name_taken(target)),
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
            "Keyman cannot delete '{project}' from the cloud. Only a member of its vault can do this."
        ));
    }
    st.remove_link(project);
    state::save(home, &st)
}

/// What to do with one project after the merge is computed.
#[derive(Debug, PartialEq)]
enum Step {
    Skipped(String),
    /// A local project has this name already (see `name_taken_reason`).
    NameTaken,
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
        return Ok(Step::NameTaken);
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

    /// A membership as the server returns it. A "personal" row carries a
    /// personal key (as Keyman makes it); other rows carry a sealed box.
    fn member(pair: &KeyPair, id: &str, kind: &str, wrapped_for: &str, key: &Key) -> Membership {
        let wrapped_key = if kind == "personal" {
            crypto::wrap_personal(pair, key, wrapped_for, 1)
        } else {
            crypto::wrap_for(&pair.public, key, wrapped_for, 1)
        };
        Membership {
            vault_id: id.to_string(),
            user_id: "me".to_string(),
            role: "owner".to_string(),
            wrapped_key,
            key_generation: 1,
            vaults: crate::cloud::api::VaultRow {
                kind: kind.to_string(),
                owner_id: "me".to_string(),
                name_sealed: None,
                key_generation: 1,
            },
        }
    }

    #[test]
    fn a_key_moved_to_another_vault_is_not_used() {
        let pair = KeyPair::generate();
        let group_key = Key::random();
        // The server copies the wrapped key of the group (a sealed box) onto
        // the personal vault row.
        let group_row = member(&pair, "group", "group", "group", &group_key);
        let mut moved = group_row.clone();
        moved.vault_id = "personal".to_string();
        moved.vaults.kind = "personal".to_string();
        let memberships = vec![group_row, moved];
        let vaults = Vaults::from_pair(&pair, &memberships, None);
        assert!(vaults.key("group").is_ok());
        assert_eq!(
            vaults.key("personal").err().unwrap(),
            "Keyman has no key for this vault."
        );
    }

    /// A sealed box that the server made itself, with the right binding.
    fn server_made(pair: &KeyPair, id: &str, kind: &str, key: &Key) -> Membership {
        let mut row = member(pair, id, "group", id, key);
        row.vaults.kind = kind.to_string();
        row
    }

    #[test]
    fn a_vault_in_two_rows_gets_no_key() {
        let pair = KeyPair::generate();
        let real = member(&pair, "p1", "personal", "p1", &Key::random());
        // A second row for the same vault, with a key the server made.
        let mut second = member(&pair, "p1", "group", "p1", &Key::random());
        second.vaults.kind = "group".to_string();
        let vaults = Vaults::from_pair(&pair, &[real, second], None);
        assert!(vaults.key("p1").is_err());
        assert!(!vaults.is_personal("p1"));
    }

    #[test]
    fn a_personal_vault_needs_a_personal_key() {
        let pair = KeyPair::generate();
        let server_key = Key::random();
        // The server makes its own key for the personal vault.
        let rows = vec![server_made(&pair, "p1", "personal", &server_key)];
        let vaults = Vaults::from_pair(&pair, &rows, None);
        assert!(vaults.key("p1").is_err());
        // The server labels the saved personal vault as a group.
        let rows = vec![server_made(&pair, "p1", "group", &server_key)];
        let vaults = Vaults::from_pair(&pair, &rows, Some("p1"));
        assert!(vaults.key("p1").is_err());
        // Without the saved vault, a real group with a sealed key still opens.
        let vaults = Vaults::from_pair(&pair, &rows, None);
        assert!(vaults.key("p1").is_ok());
        // The personal key that Keyman makes opens.
        let rows = vec![member(&pair, "p1", "personal", "p1", &Key::random())];
        let vaults = Vaults::from_pair(&pair, &rows, Some("p1"));
        assert!(vaults.key("p1").is_ok());
    }

    #[test]
    fn the_personal_vault_is_checked_against_the_saved_one() {
        let pair = KeyPair::generate();
        let key = Key::random();
        let one = vec![member(&pair, "p1", "personal", "p1", &key)];
        assert_eq!(personal_in(&one, "me", None).unwrap(), "p1");
        assert_eq!(personal_in(&one, "me", Some("p1")).unwrap(), "p1");
        assert_eq!(
            personal_in(&one, "me", Some("p0")).unwrap_err(),
            "The server changed your personal vault. Keyman stops."
        );
        // A group that the server labels "personal" is a second personal vault.
        let two = vec![
            member(&pair, "p1", "personal", "p1", &key),
            member(&pair, "g1", "personal", "g1", &key),
        ];
        assert!(personal_in(&two, "me", None).is_err());
        assert_eq!(
            personal_in(&two, "me", Some("p1")).unwrap_err(),
            PERSONAL_CHANGED
        );
        // The personal vault of another user is not mine.
        assert_eq!(personal_in(&one, "other", None).unwrap_err(), NO_PERSONAL);
        assert_eq!(personal_in(&[], "me", None).unwrap_err(), NO_PERSONAL);
    }

    #[test]
    fn two_unlinked_cloud_projects_with_one_name_are_refused() {
        let names = vec!["app".to_string(), "other".to_string(), "app".to_string()];
        assert_eq!(
            one_cloud_project_named(&names, "app").unwrap_err(),
            "More than one project in the cloud has the name app."
        );
        assert!(one_cloud_project_named(&names, "other").is_ok());
        assert!(one_cloud_project_named(&names, "new").is_ok());
    }

    #[test]
    fn an_older_cloud_version_is_skipped() {
        assert_eq!(
            older_in_cloud("app", 5, 4),
            Some(
                "The cloud has an older version of app than this computer. Keyman did not change it."
                    .to_string()
            )
        );
        assert_eq!(
            moved_to_other_vault("app", "personal", "fake-group").unwrap(),
            "The server moved app to another vault. Keyman did not change it."
        );
        assert_eq!(moved_to_other_vault("app", "v1", "v1"), None);
        assert_eq!(older_in_cloud("app", 5, 5), None);
        assert_eq!(older_in_cloud("app", 5, 6), None);
    }

    #[test]
    fn a_push_reply_must_be_the_next_version() {
        assert!(check_push_reply("app", 3, 4).is_ok());
        for wrong in [1, 3, 5, 99] {
            assert_eq!(
                check_push_reply("app", 3, wrong).unwrap_err(),
                "The server gave a wrong version for app. Keyman did not change the link. Run 'keyman pull app'."
            );
        }
    }

    fn cloud_row(version: i64) -> CloudProject {
        CloudProject {
            id: "c1".to_string(),
            vault_id: "v".to_string(),
            version,
            key_generation: 1,
            sealed: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_push_that_changes_nothing_says_why() {
        assert_eq!(
            not_pushed("app", None),
            PushResult::Skipped {
                project: "app".to_string(),
                reason: "The project app is not in the cloud now. Run 'keyman unlink app'."
                    .to_string()
            }
        );
        assert_eq!(
            not_pushed("app", Some(&cloud_row(7))),
            PushResult::CloudIsNewer("app".to_string())
        );
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
    fn added_step_skips_a_name_that_exists_or_is_linked() {
        let step = added_step("other:\n  X: \"1\"\n", "app", true, &v(&[])).unwrap();
        assert_eq!(step, Step::NameTaken);
    }

    #[test]
    fn a_name_collision_is_name_taken() {
        let step = added_step("app:\n  A: \"1\"\n", "app", false, &v(&[("A", "2")])).unwrap();
        assert_eq!(step, Step::NameTaken);
        assert_eq!(
            name_taken("app".to_string()),
            PullResult::NameTaken {
                project: "app".into()
            }
        );
        assert!(name_taken_reason("app").contains("keyman pull app"));
    }

    #[derive(Default)]
    struct Recording(Vec<String>);

    impl Resolver for Recording {
        fn resolve(
            &mut self,
            _project: &str,
            _conflicts: &[Conflict],
        ) -> Result<Option<Vec<Option<String>>>, String> {
            self.0.push("resolve".to_string());
            Ok(None)
        }
        fn invites(&mut self, given: &[(String, String)], _warnings: &[String]) {
            self.0.push(format!("invites {}", given.len()));
        }
    }

    /// A resolver whose `stopped` turns true at call `after` (0 = at once).
    struct StopAt {
        calls: std::cell::Cell<usize>,
        after: usize,
    }

    impl Resolver for StopAt {
        fn resolve(
            &mut self,
            _project: &str,
            _conflicts: &[Conflict],
        ) -> Result<Option<Vec<Option<String>>>, String> {
            Ok(None)
        }
        fn stopped(&self) -> bool {
            let n = self.calls.get();
            self.calls.set(n + 1);
            n >= self.after
        }
    }

    /// Two cloud projects, "a" and "b", in the personal vault "vault".
    fn two_cloud_projects() -> (Vaults, Vec<CloudProject>) {
        let pair = KeyPair::generate();
        let key = Key::random();
        let rows = ["a", "b"]
            .iter()
            .map(|name| {
                let id = format!("cloud-{name}");
                CloudProject {
                    sealed: crypto::seal_project(&key, &id, 1, name, &v(&[("A", "1")])),
                    id,
                    vault_id: "vault".to_string(),
                    version: 1,
                    key_generation: 1,
                    updated_at: String::new(),
                }
            })
            .collect();
        let memberships = [member(&pair, "vault", "personal", "vault", &key)];
        (Vaults::from_pair(&pair, &memberships, Some("vault")), rows)
    }

    #[test]
    fn a_stopped_pull_leaves_the_projects_after_it() {
        let (vaults, rows) = two_cloud_projects();
        let home = home_with(
            "other:
  X: \"1\"
",
            Vec::new(),
        );
        let mut stop = StopAt {
            calls: std::cell::Cell::new(0),
            after: 1,
        };
        let results = pull_rows(&home, &vaults, &rows, None, None, &mut stop).unwrap();
        // Only the first row is done; "b" is not written and not linked.
        assert_eq!(results, vec![PullResult::Added("a".into())]);
        let names = master::project_names(&master::read_master(&home).unwrap().1).unwrap();
        assert!(names.contains(&"a".to_string()));
        assert!(!names.contains(&"b".to_string()));
        let st = state::load(&home).unwrap();
        assert!(st.link("a").is_some() && st.link("b").is_none());

        // Not stopped: both rows are done (the default resolver never stops).
        let home = home_with(
            "other:
  X: \"1\"
",
            Vec::new(),
        );
        let mut plain = Fixed(None, 0);
        assert!(!plain.stopped());
        let results = pull_rows(&home, &vaults, &rows, None, None, &mut plain).unwrap();
        assert_eq!(
            results,
            vec![PullResult::Added("a".into()), PullResult::Added("b".into())]
        );

        // Stopped before the first row: nothing is done, and a link whose row
        // was not read is not called gone.
        let home = home_with(
            "other:
  X: \"1\"
",
            vec![link_of("x", v(&[]))],
        );
        let mut at_once = StopAt {
            calls: std::cell::Cell::new(0),
            after: 0,
        };
        let results = pull_rows(&home, &vaults, &rows, None, None, &mut at_once).unwrap();
        assert!(results.is_empty(), "{results:?}");
    }

    #[test]
    fn the_invites_hook_has_a_default_and_can_be_recorded() {
        // `pull` calls `invites` itself, right after `accept_pending`. The
        // end-to-end tests run the real order; here the default is a no-op.
        let mut plain = Fixed(None, 0);
        plain.invites(&[("g".into(), "e@x.com".into())], &[]);
        assert_eq!(plain.1, 0);
        let mut r = Recording::default();
        r.invites(&[], &[]);
        let _ = r.resolve("app", &[]);
        assert_eq!(r.0, vec!["invites 0", "resolve"]);
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
            open_failure(1, 2, "The data is not valid.".to_string()),
            "The group key changed. Run 'keyman pull' again."
        );
        assert_eq!(
            open_failure(2, 2, "The data is not valid.".to_string()),
            "Keyman cannot open this project. The data is not valid."
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
        assert_eq!(step, Step::NameTaken);
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
