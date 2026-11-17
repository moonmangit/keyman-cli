//! Share groups. A group is a vault whose key is wrapped for each member.
//!
//! Any member's computer can finish an invite, because it only has to wrap
//! the vault key for the public key of the invitee. Removing a member makes
//! a new vault key and encrypts every project again, in one server call.

use super::account::Unlocked;
use super::api::{self, CloudProject, Invite, Membership, PublicProfile};
use super::crypto::{self, Key};
use super::state;
use super::sync::Vaults;
use crate::ui::{is_hidden, safe_text};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::HashMap;
use std::path::Path;

/// One group, as `group list` shows it. `members` are user IDs, `invited`
/// are email addresses.
#[derive(Debug, PartialEq)]
pub struct Group {
    pub vault_id: String,
    pub name: String,
    pub owner: bool,
    pub members: Vec<String>,
    pub invited: Vec<String>,
}

impl Group {
    /// For example "owner · 3 members · 1 invited".
    pub fn detail(&self) -> String {
        let count = |n: usize, word: &str| {
            if n == 1 {
                format!("1 {word}")
            } else {
                format!("{n} {word}s")
            }
        };
        let mut parts = vec![
            (if self.owner { "owner" } else { "member" }).to_string(),
            count(self.members.len(), "member"),
        ];
        if !self.invited.is_empty() {
            parts.push(format!("{} invited", self.invited.len()));
        }
        parts.join(" · ")
    }
}

const NOT_OWNER_INVITE: &str = "Only the owner of the group can invite persons.";
const NOT_OWNER_REMOVE: &str = "Only the owner of the group can remove persons.";
const NOT_OWNER_DELETE: &str = "Only the owner of the group can delete the group.";
const KEY_CHANGED: &str = "The group key changed. Run the command again.";

/// Re-seals one project for a new vault key. The server makes the version
/// one higher, so the new text is sealed for `version + 1`.
pub fn reseal(
    old: &Key,
    new: &Key,
    id: &str,
    version: i64,
    sealed: &str,
) -> Result<String, String> {
    let (name, variables) = crypto::open_project(old, id, version, sealed)?;
    Ok(crypto::seal_project(
        new,
        id,
        version + 1,
        &name,
        &variables,
    ))
}

/// `(id, base version, sealed)` for every project, as `rotate_vault` needs.
/// If one project does not open, nothing is returned: a removal must not
/// leave a project that nobody can read.
fn reseal_all(
    old: &Key,
    new: &Key,
    rows: &[CloudProject],
) -> Result<Vec<(String, i64, String)>, String> {
    rows.iter()
        .map(|row| {
            let sealed = reseal(old, new, &row.id, row.version, &row.sealed).map_err(|_| {
                "Keyman cannot open a project of the group. Keyman did not remove anyone."
                    .to_string()
            })?;
            Ok((row.id.clone(), row.version, sealed))
        })
        .collect()
}

/// Lowercase and trim an email address, and refuse one that cannot be valid.
fn clean_email(email: &str) -> Result<String, String> {
    let email = email.trim().to_lowercase();
    let valid = match email.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        }
        None => false,
    };
    if valid {
        Ok(email)
    } else {
        Err(format!("The email address '{email}' is not valid."))
    }
}

/// Checks an invite before it is sent. Returns the clean email address.
fn check_invite(
    email: &str,
    my_email: &str,
    members: &[String],
    invited: &[String],
) -> Result<String, String> {
    let email = clean_email(email)?;
    if email == my_email.to_lowercase() {
        return Err("You cannot invite your own email address.".to_string());
    }
    if members.iter().any(|member| member.to_lowercase() == email) {
        return Err(format!("{email} is already in the group."));
    }
    if invited.iter().any(|item| item.to_lowercase() == email) {
        return Err(format!("{email} has an invite already."));
    }
    Ok(email)
}

/// The name of a group as sealed, with no change.
fn raw_name(vaults: &Vaults, member: &Membership) -> Option<String> {
    if member.vaults.kind != "group" {
        return None;
    }
    let sealed = member.vaults.name_sealed.as_ref()?;
    let (key, _) = vaults.key(&member.vault_id).ok()?;
    let plain = crypto::open(key, &crypto::vault_name_aad(&member.vault_id), sealed).ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

/// The name of a group, with hidden characters replaced (the owner makes
/// the name, and it goes to the terminal of every member). It is also the
/// form that names are compared in.
fn group_name(vaults: &Vaults, member: &Membership) -> Option<String> {
    Some(safe_text(&raw_name(vaults, member)?))
}

/// `(vault ID, name)` of every group the user is in.
fn names(vaults: &Vaults) -> Vec<(String, String)> {
    vaults
        .memberships()
        .iter()
        .filter_map(|member| Some((member.vault_id.clone(), group_name(vaults, member)?)))
        .collect()
}

pub(crate) fn find_in(vaults: &Vaults, name: &str) -> Result<String, String> {
    let mut found = names(vaults)
        .into_iter()
        .filter(|(_, group)| group == name)
        .map(|(id, _)| id);
    match (found.next(), found.next()) {
        (Some(id), None) => Ok(id),
        (Some(_), Some(_)) => Err(format!("You are in more than one group named '{name}'.")),
        _ => Err(format!("There is no group named '{name}'.")),
    }
}

/// The groups of the user, sorted by name, with the counts from the
/// `vault_members` and `invites` rows.
fn summarize(vaults: &Vaults, me: &str, members: &[Membership], invites: &[Invite]) -> Vec<Group> {
    let mut groups: Vec<Group> = vaults
        .memberships()
        .iter()
        .filter_map(|member| {
            let name = group_name(vaults, member)?;
            let id = &member.vault_id;
            Some(Group {
                vault_id: id.clone(),
                name,
                owner: member.vaults.owner_id == me,
                members: members
                    .iter()
                    .filter(|row| &row.vault_id == id)
                    .map(|row| row.user_id.clone())
                    .collect(),
                invited: invites
                    .iter()
                    .filter(|row| &row.vault_id == id)
                    .map(|row| row.email.clone())
                    .collect(),
            })
        })
        .collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}

fn public_key(profile: &PublicProfile) -> Result<[u8; 32], String> {
    STANDARD
        .decode(&profile.public_key)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| {
            format!(
                "The public key of {} is not valid.",
                safe_text(&profile.email)
            )
        })
}

/// The membership of the user in this vault, only when the user owns it.
fn owned<'a>(
    vaults: &'a Vaults,
    u: &Unlocked,
    vault_id: &str,
    refusal: &str,
) -> Result<&'a Membership, String> {
    vaults
        .memberships()
        .iter()
        .find(|member| member.vault_id == vault_id && member.vaults.owner_id == u.api.user_id())
        .ok_or_else(|| refusal.to_string())
}

/// Makes a group and returns its vault ID. Keyman makes the ID, so the
/// wrapped key (generation 1) and the sealed name are bound to it.
pub fn create(u: &Unlocked, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("The group needs a name.".to_string());
    }
    if name.chars().any(is_hidden) {
        return Err("The name of the group has a character that Keyman cannot use.".to_string());
    }
    let vaults = Vaults::load(u)?;
    if names(&vaults).iter().any(|(_, existing)| existing == name) {
        return Err(format!("You are in a group named '{name}' already."));
    }
    let id = api::new_uuid();
    let key = Key::random();
    let wrapped = crypto::wrap_for(&u.pair.public, &key, &id, 1);
    let sealed = crypto::seal(&key, &crypto::vault_name_aad(&id), name.as_bytes());
    u.api.create_vault(&id, "group", Some(&sealed), &wrapped)?;
    Ok(id)
}

/// The vault ID of the group with this name.
pub fn find(_u: &Unlocked, vaults: &Vaults, name: &str) -> Result<String, String> {
    find_in(vaults, name)
}

/// Only the owner can invite. The invite is complete when a member runs
/// `keyman pull` (see `accept_pending`).
pub fn invite(u: &Unlocked, group: &str, email: &str) -> Result<(), String> {
    let vaults = Vaults::load(u)?;
    let id = find_in(&vaults, group)?;
    owned(&vaults, u, &id, NOT_OWNER_INVITE)?;
    let rows = u.api.vault_members(&id)?;
    let ids: Vec<String> = rows.iter().map(|row| row.user_id.clone()).collect();
    let members: Vec<String> = u
        .api
        .public_profiles(&ids)?
        .into_iter()
        .map(|profile| profile.email)
        .collect();
    let invited: Vec<String> = u
        .api
        .invites_for_vaults(std::slice::from_ref(&id))?
        .into_iter()
        .map(|row| row.email)
        .collect();
    let email = check_invite(email, u.api.email(), &members, &invited)?;
    u.api.create_invite(&id, &email)
}

/// What to do with the answer of the server for one invite.
#[derive(Debug, PartialEq)]
enum InviteOutcome {
    Done,
    /// Another member was first, or the key changed. The next pull is right.
    Quiet,
    Warn,
}

fn invite_outcome(answer: &Result<(), String>) -> InviteOutcome {
    match answer {
        Ok(()) => InviteOutcome::Done,
        Err(message) if message == api::INVITE_GONE || message == api::GROUP_KEY_CHANGED => {
            InviteOutcome::Quiet
        }
        Err(_) => InviteOutcome::Warn,
    }
}

/// What `accept_pending` did: `(group, email)` for each person who got
/// access, and a warning for each invite that Keyman could not finish.
#[derive(Debug, Default, PartialEq)]
pub struct Accepted {
    pub given: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// For each invite in my groups whose email has a profile: wrap the vault
/// key for that profile and accept. An invite with no profile is skipped:
/// the person has not set up Keyman Cloud yet. A problem with one invite
/// (the invitee controls their own public key) gives a warning and never
/// stops the others. Only a failure to read the invites is an error.
pub fn accept_pending(u: &Unlocked, vaults: &Vaults) -> Result<Accepted, String> {
    let group_names: HashMap<String, String> = names(vaults).into_iter().collect();
    let mut ids: Vec<String> = group_names.keys().cloned().collect();
    ids.sort();
    let mut result = Accepted::default();
    for invite in u.api.invites_for_vaults(&ids)? {
        let Ok((key, generation)) = vaults.key(&invite.vault_id) else {
            continue;
        };
        let group = group_names
            .get(&invite.vault_id)
            .cloned()
            .unwrap_or_default();
        let warn = format!(
            "Keyman cannot give {} access to {group} now.",
            safe_text(&invite.email)
        );
        let profile = match u.api.find_profile_by_email(&invite.email) {
            Ok(Some(profile)) => profile,
            Ok(None) => continue,
            Err(_) => {
                result.warnings.push(warn);
                continue;
            }
        };
        let Ok(public) = public_key(&profile) else {
            result.warnings.push(warn);
            continue;
        };
        // Bound to this vault and the generation that the server checks.
        let wrapped = crypto::wrap_for(&public, key, &invite.vault_id, *generation);
        let answer = u
            .api
            .accept_invite(&invite.id, &profile.user_id, &wrapped, *generation);
        match invite_outcome(&answer) {
            InviteOutcome::Done => result.given.push((group, safe_text(&invite.email))),
            InviteOutcome::Quiet => {}
            InviteOutcome::Warn => result.warnings.push(warn),
        }
    }
    Ok(result)
}

/// How many invites for my own email address wait for a member.
pub fn waiting(u: &Unlocked) -> Result<usize, String> {
    Ok(u.api.my_invites()?.len())
}

/// What a removal does to an email address.
#[derive(Debug, PartialEq)]
pub enum Removal {
    /// A member: new group key, `projects` encrypted again.
    Member { projects: usize },
    /// A pending invite: only the invite is deleted (no key was given).
    Invite,
}

#[derive(Debug, PartialEq)]
enum Target {
    Member,
    Invite(String),
}

/// A member comes first. Then a pending invite.
fn classify(email: &str, member_emails: &[String], invites: &[Invite]) -> Option<Target> {
    if member_emails.iter().any(|m| m.to_lowercase() == email) {
        return Some(Target::Member);
    }
    invites
        .iter()
        .find(|invite| invite.email.to_lowercase() == email)
        .map(|invite| Target::Invite(invite.id.clone()))
}

/// Before the confirmation: the user must own the group, and the email must
/// be a member or have an invite. Only a member needs a confirmation.
pub fn check_remove(
    u: &Unlocked,
    vaults: &Vaults,
    group: &str,
    email: &str,
) -> Result<Removal, String> {
    let email = clean_email(email)?;
    let id = find_in(vaults, group)?;
    owned(vaults, u, &id, NOT_OWNER_REMOVE)?;
    let rows = u.api.vault_members(&id)?;
    let ids: Vec<String> = rows.iter().map(|row| row.user_id.clone()).collect();
    let profiles = u.api.public_profiles(&ids)?;
    let member_emails: Vec<String> = profiles.iter().map(|p| p.email.clone()).collect();
    let invites = u.api.invites_for_vaults(std::slice::from_ref(&id))?;
    match classify(&email, &member_emails, &invites) {
        Some(Target::Member) => {
            if profiles
                .iter()
                .any(|p| p.user_id == u.api.user_id() && p.email.to_lowercase() == email)
            {
                return Err("The owner cannot be removed. Delete the group instead.".to_string());
            }
            Ok(Removal::Member {
                projects: u.api.projects(std::slice::from_ref(&id))?.len(),
            })
        }
        Some(Target::Invite(_)) => Ok(Removal::Invite),
        None => Err(format!("{email} is not in the group '{group}'.")),
    }
}

/// Removes a member, or deletes the invite of a person who is not a member.
/// For a member it makes a new vault key, encrypts all projects and the
/// name again, and wraps the key for each remaining member, in one call.
pub fn remove(u: &Unlocked, vaults: &Vaults, group: &str, email: &str) -> Result<Removal, String> {
    let email = clean_email(email)?;
    let id = find_in(vaults, group)?;
    let membership = owned(vaults, u, &id, NOT_OWNER_REMOVE)?;
    let (old, generation) = vaults.key(&id)?;
    let rows = u.api.vault_members(&id)?;
    let ids: Vec<String> = rows.iter().map(|row| row.user_id.clone()).collect();
    let profiles = u.api.public_profiles(&ids)?;
    let member_emails: Vec<String> = profiles.iter().map(|p| p.email.clone()).collect();
    let invites = u.api.invites_for_vaults(std::slice::from_ref(&id))?;
    let target = match classify(&email, &member_emails, &invites) {
        Some(Target::Invite(invite_id)) => {
            u.api.delete_invite(&invite_id)?;
            let left = u.api.invites_for_vaults(std::slice::from_ref(&id))?;
            if left.iter().any(|invite| invite.id == invite_id) {
                return Err("Keyman could not remove the invite.".to_string());
            }
            return Ok(Removal::Invite);
        }
        Some(Target::Member) => profiles
            .iter()
            .find(|profile| profile.email.to_lowercase() == email)
            .ok_or_else(|| format!("{email} is not in the group '{group}'."))?,
        None => return Err(format!("{email} is not in the group '{group}'.")),
    };
    if target.user_id == u.api.user_id() {
        return Err("The owner cannot be removed. Delete the group instead.".to_string());
    }
    if membership.vaults.key_generation != *generation {
        return Err(KEY_CHANGED.to_string());
    }
    // The server makes the generation one higher; the new keys are bound to it.
    let next = generation + 1;
    let new = Key::random();
    let mut keys = Vec::new();
    for row in rows.iter().filter(|row| row.user_id != target.user_id) {
        let profile = profiles
            .iter()
            .find(|profile| profile.user_id == row.user_id)
            .ok_or_else(|| {
                "Keyman cannot read the profile of a member. Keyman did not remove anyone."
                    .to_string()
            })?;
        keys.push((
            row.user_id.clone(),
            crypto::wrap_for(&public_key(profile)?, &new, &id, next),
        ));
    }
    let projects = u.api.projects(std::slice::from_ref(&id))?;
    if projects.iter().any(|row| row.key_generation != *generation) {
        return Err(KEY_CHANGED.to_string());
    }
    let sealed = reseal_all(old, &new, &projects)?;
    // The name that was sealed, not the text that was typed.
    let name = raw_name(vaults, membership)
        .ok_or_else(|| "Keyman cannot read the name of the group.".to_string())?;
    let sealed_name = crypto::seal(&new, &crypto::vault_name_aad(&id), name.as_bytes());
    let made = u
        .api
        .rotate_vault(&id, &target.user_id, &keys, &sealed, Some(&sealed_name))?;
    if made != next {
        return Err(
            "The group key changed during the removal. Run 'keyman group remove' again."
                .to_string(),
        );
    }
    Ok(Removal::Member {
        projects: projects.len(),
    })
}

/// Before a delete is confirmed: the user must own the group. Returns the
/// vault ID, which `forget_links` needs after the delete.
pub fn check_delete(u: &Unlocked, vaults: &Vaults, group: &str) -> Result<String, String> {
    let id = find_in(vaults, group)?;
    owned(vaults, u, &id, NOT_OWNER_DELETE)?;
    Ok(id)
}

/// The groups of the user. It needs no decryption except the names.
pub fn list(u: &Unlocked, vaults: &Vaults) -> Result<Vec<Group>, String> {
    let ids: Vec<String> = names(vaults).into_iter().map(|(id, _)| id).collect();
    let members = u.api.members_for_vaults(&ids)?;
    let invites = u.api.invites_for_vaults(&ids)?;
    Ok(summarize(vaults, u.api.user_id(), &members, &invites))
}

/// Only the owner can delete. The server ignores a delete that it does not
/// allow, so the group is read again after the call.
pub fn delete(u: &Unlocked, vaults: &Vaults, group: &str) -> Result<(), String> {
    let id = find_in(vaults, group)?;
    owned(vaults, u, &id, NOT_OWNER_DELETE)?;
    u.api.delete_vault(&id)?;
    if !u.api.vault_members(&id)?.is_empty() {
        return Err("Keyman could not delete the group.".to_string());
    }
    Ok(())
}

/// Removes the links that point to a deleted group. Local projects stay.
/// Returns how many links it removed.
pub fn forget_links(home: &Path, vault_id: &str) -> Result<usize, String> {
    let mut st = state::load(home)?;
    let before = st.links.len();
    st.links.retain(|link| link.vault_id != vault_id);
    let removed = before - st.links.len();
    if removed > 0 {
        state::save(home, &st)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::api::VaultRow;
    use crate::cloud::crypto::KeyPair;
    use crate::cloud::state::{Link, State};
    use std::fs;
    use std::path::PathBuf;

    fn vars() -> Vec<(String, String)> {
        vec![("A".to_string(), "1".to_string())]
    }

    fn row(id: &str, version: i64, sealed: String) -> CloudProject {
        CloudProject {
            id: id.to_string(),
            vault_id: "v".to_string(),
            version,
            key_generation: 1,
            sealed,
            updated_at: String::new(),
        }
    }

    #[test]
    fn one_bad_invite_is_a_warning_not_an_error() {
        assert_eq!(invite_outcome(&Ok(())), InviteOutcome::Done);
        for quiet in [
            "the invite does not exist",
            "the group key changed; run the command again",
        ] {
            assert_eq!(
                invite_outcome(&Err(api::server_message("P0001", quiet))),
                InviteOutcome::Quiet
            );
        }
        for bad in [
            "duplicate key value violates unique constraint",
            "the user does not match the invite email",
        ] {
            assert_eq!(invite_outcome(&Err(bad.to_string())), InviteOutcome::Warn);
        }
        let bad_key = PublicProfile {
            user_id: "u".to_string(),
            email: "x@x.com".to_string(),
            public_key: "not a key".to_string(),
        };
        assert!(public_key(&bad_key).is_err());
    }

    #[test]
    fn an_invite_is_removed_when_the_person_is_not_a_member() {
        let invites = vec![Invite {
            id: "i1".to_string(),
            vault_id: "v".to_string(),
            email: "Pending@x.com".to_string(),
        }];
        let members = vec!["m@x.com".to_string()];
        assert_eq!(
            classify("m@x.com", &members, &invites),
            Some(Target::Member)
        );
        assert_eq!(
            classify("pending@x.com", &members, &invites),
            Some(Target::Invite("i1".to_string()))
        );
        assert_eq!(classify("none@x.com", &members, &invites), None);
        // a member who also has an invite is a member
        assert_eq!(
            classify("pending@x.com", &["pending@x.com".to_string()], &invites),
            Some(Target::Member)
        );
    }

    #[test]
    fn format_characters_are_hidden_in_names() {
        let pair = KeyPair::generate();
        let m = membership(
            &pair,
            "v9",
            "group",
            Some("a\u{202E}b\u{200B}c\u{2066}d"),
            "me",
        );
        let vaults = Vaults::from_pair(&pair, std::slice::from_ref(&m));
        assert_eq!(group_name(&vaults, &m).unwrap(), "a?b?c?d");
        // the sealed text keeps the original, for re-sealing
        assert_eq!(
            raw_name(&vaults, &m).unwrap(),
            "a\u{202E}b\u{200B}c\u{2066}d"
        );
    }

    #[test]
    fn link_refuses_two_groups_with_one_name() {
        let f = fixture();
        let mut memberships = f.memberships.clone();
        memberships.push(membership(&f.pair, "v3", "group", Some("billing"), "other"));
        let vaults = Vaults::from_pair(&f.pair, &memberships);
        assert_eq!(
            crate::cloud::sync::group_vault(&vaults, "billing").unwrap_err(),
            "You are in more than one group named 'billing'."
        );
        assert_eq!(
            crate::cloud::sync::group_vault(&vaults, "team-api").unwrap(),
            "v1"
        );
        assert_eq!(
            crate::cloud::sync::group_vault(&vaults, "zzz").unwrap_err(),
            "You are not in a group named 'zzz'."
        );
    }

    #[test]
    fn reseal_for_rotation_uses_next_version() {
        let old = Key::random();
        let new = Key::random();
        let sealed = crypto::seal_project(&old, "p1", 3, "app", &vars());
        let out = reseal(&old, &new, "p1", 3, &sealed).unwrap();
        let (name, variables) = crypto::open_project(&new, "p1", 4, &out).unwrap();
        assert_eq!(name, "app");
        assert_eq!(variables, vars());
        assert!(crypto::open_project(&new, "p1", 3, &out).is_err());
        assert!(crypto::open_project(&old, "p1", 4, &out).is_err());
    }

    #[test]
    fn reseal_with_wrong_old_key_fails() {
        let sealed = crypto::seal_project(&Key::random(), "p1", 1, "app", &vars());
        assert!(reseal(&Key::random(), &Key::random(), "p1", 1, &sealed).is_err());
    }

    #[test]
    fn reseal_all_stops_when_one_project_does_not_open() {
        let old = Key::random();
        let new = Key::random();
        let good = row("p1", 2, crypto::seal_project(&old, "p1", 2, "a", &vars()));
        let bad = row(
            "p2",
            1,
            crypto::seal_project(&Key::random(), "p2", 1, "b", &vars()),
        );
        let done = reseal_all(&old, &new, std::slice::from_ref(&good)).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].0.as_str(), done[0].1), ("p1", 2));
        let error = reseal_all(&old, &new, &[good, bad]).unwrap_err();
        assert!(error.contains("Keyman did not remove anyone."), "{error}");
    }

    #[test]
    fn emails_are_cleaned_and_checked() {
        assert_eq!(
            clean_email("  Bob@Example.COM ").unwrap(),
            "bob@example.com"
        );
        for bad in ["", "bob", "@x.com", "bob@", "a b@x.com", "a@b@c"] {
            assert!(clean_email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn invite_checks() {
        let mine = "owner@x.com";
        let members = vec!["owner@x.com".to_string(), "m@x.com".to_string()];
        let invited = vec!["i@x.com".to_string()];
        assert_eq!(
            check_invite("New@X.com", mine, &members, &invited).unwrap(),
            "new@x.com"
        );
        let own = check_invite("OWNER@x.com", mine, &members, &invited).unwrap_err();
        assert_eq!(own, "You cannot invite your own email address.");
        let member = check_invite("m@x.com", mine, &members, &invited).unwrap_err();
        assert_eq!(member, "m@x.com is already in the group.");
        let again = check_invite("I@x.com", mine, &members, &invited).unwrap_err();
        assert_eq!(again, "i@x.com has an invite already.");
    }

    struct Fixture {
        pair: KeyPair,
        memberships: Vec<Membership>,
    }

    fn membership(
        pair: &KeyPair,
        id: &str,
        kind: &str,
        name: Option<&str>,
        owner: &str,
    ) -> Membership {
        let key = Key::random();
        let sealed = name.map(|n| crypto::seal(&key, &crypto::vault_name_aad(id), n.as_bytes()));
        Membership {
            vault_id: id.to_string(),
            user_id: "me".to_string(),
            role: if owner == "me" { "owner" } else { "member" }.to_string(),
            wrapped_key: crypto::wrap_for(&pair.public, &key, id, 1),
            key_generation: 1,
            vaults: VaultRow {
                kind: kind.to_string(),
                owner_id: owner.to_string(),
                name_sealed: sealed,
                key_generation: 1,
            },
        }
    }

    fn fixture() -> Fixture {
        let pair = KeyPair::generate();
        let memberships = vec![
            membership(&pair, "v0", "personal", None, "me"),
            membership(&pair, "v1", "group", Some("team-api"), "me"),
            membership(&pair, "v2", "group", Some("billing"), "other"),
        ];
        Fixture { pair, memberships }
    }

    fn other_member(vault: &str, user: &str) -> Membership {
        Membership {
            vault_id: vault.to_string(),
            user_id: user.to_string(),
            role: "member".to_string(),
            wrapped_key: String::new(),
            key_generation: 1,
            vaults: VaultRow {
                kind: "group".to_string(),
                owner_id: "me".to_string(),
                name_sealed: None,
                key_generation: 1,
            },
        }
    }

    #[test]
    fn find_gives_the_vault_of_a_group_name() {
        let f = fixture();
        let vaults = Vaults::from_pair(&f.pair, &f.memberships);
        assert_eq!(find_in(&vaults, "team-api").unwrap(), "v1");
        assert_eq!(find_in(&vaults, "billing").unwrap(), "v2");
        assert_eq!(
            find_in(&vaults, "nope").unwrap_err(),
            "There is no group named 'nope'."
        );
    }

    #[test]
    fn a_personal_vault_is_never_a_group() {
        let f = fixture();
        let vaults = Vaults::from_pair(&f.pair, &f.memberships);
        assert!(group_name(&vaults, &f.memberships[0]).is_none());
        assert_eq!(
            find_in(&vaults, "").unwrap_err(),
            "There is no group named ''."
        );
    }

    #[test]
    fn list_counts_members_and_invites() {
        let f = fixture();
        let vaults = Vaults::from_pair(&f.pair, &f.memberships);
        let members = vec![
            other_member("v1", "me"),
            other_member("v1", "u2"),
            other_member("v1", "u3"),
            other_member("v2", "me"),
            other_member("v2", "u4"),
        ];
        let invites = vec![Invite {
            id: "i1".to_string(),
            vault_id: "v1".to_string(),
            email: "x@x.com".to_string(),
        }];
        let groups = summarize(&vaults, "me", &members, &invites);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "billing");
        assert!(!groups[0].owner);
        assert_eq!(groups[0].members.len(), 2);
        assert!(groups[0].invited.is_empty());
        assert_eq!(groups[1].name, "team-api");
        assert!(groups[1].owner);
        assert_eq!(groups[1].members.len(), 3);
        assert_eq!(groups[1].invited, vec!["x@x.com".to_string()]);
    }

    #[test]
    fn two_groups_with_one_name_are_refused() {
        let f = fixture();
        let mut memberships = f.memberships.clone();
        memberships.push(membership(&f.pair, "v3", "group", Some("billing"), "other"));
        let vaults = Vaults::from_pair(&f.pair, &memberships);
        assert_eq!(
            find_in(&vaults, "billing").unwrap_err(),
            "You are in more than one group named 'billing'."
        );
        assert_eq!(find_in(&vaults, "team-api").unwrap(), "v1");
    }

    #[test]
    fn detail_line_matches_the_list_format() {
        let group = |owner, members: usize, invited: usize| Group {
            vault_id: String::new(),
            name: String::new(),
            owner,
            members: vec![String::new(); members],
            invited: vec![String::new(); invited],
        };
        assert_eq!(group(true, 3, 1).detail(), "owner · 3 members · 1 invited");
        assert_eq!(group(false, 2, 0).detail(), "member · 2 members");
        assert_eq!(group(false, 1, 0).detail(), "member · 1 member");
    }

    #[test]
    fn control_characters_in_a_name_are_replaced() {
        let pair = KeyPair::generate();
        let m = membership(&pair, "v9", "group", Some("a\u{1b}[31mb"), "me");
        let vaults = Vaults::from_pair(&pair, std::slice::from_ref(&m));
        assert_eq!(group_name(&vaults, &m).unwrap(), "a?[31mb");
    }

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keyman-groups-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn link(project: &str, vault: &str) -> Link {
        Link {
            project: project.to_string(),
            cloud_id: format!("id-{project}"),
            vault_id: vault.to_string(),
            version: 1,
            base: vars(),
            cloud_name: project.to_string(),
        }
    }

    #[test]
    fn deleting_a_group_removes_only_its_links() {
        let home = temp_home("links");
        let st = State {
            links: vec![link("a", "v1"), link("b", "v2"), link("c", "v1")],
            ..State::default()
        };
        state::save(&home, &st).unwrap();
        assert_eq!(forget_links(&home, "v1").unwrap(), 2);
        let after = state::load(&home).unwrap();
        assert_eq!(after.links.len(), 1);
        assert_eq!(after.links[0].project, "b");
        assert_eq!(forget_links(&home, "v1").unwrap(), 0);
        let _ = fs::remove_dir_all(&home);
    }
}
