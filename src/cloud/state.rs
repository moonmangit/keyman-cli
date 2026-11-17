//! The private sync state file, `~/.keyman_sync`.
//!
//! It links local projects to cloud projects. It also keeps the last-synced
//! variables (secrets), so it gets the same protection as the master file.

use crate::master::{atomic_write, set_private_permissions, Variables};
use crate::ui::display_path;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Link {
    pub project: String,
    pub cloud_id: String,
    pub vault_id: String,
    pub version: i64,
    pub base: Variables,
    /// The name inside the cloud copy. It can differ from `project` after `pull --as`.
    #[serde(default)]
    pub cloud_name: String,
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
pub struct State {
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// The personal vault of `user_id`, saved at setup or at the first
    /// unlock. A server that reports a different one is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_vault: Option<String>,
}

pub fn path(home: &Path) -> PathBuf {
    home.join(".keyman_sync")
}

pub fn load(home: &Path) -> Result<State, String> {
    let file = path(home);
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(State::default()),
        Err(error) => return Err(format!("cannot read '{}': {error}", display_path(&file))),
    };
    serde_json::from_str(&text).map_err(|error| {
        format!(
            "cannot read {}: {error}

The file is damaged. Remove it, then run 'keyman login' and 'keyman pull' again.",
            display_path(&file)
        )
    })
}

pub fn save(home: &Path, state: &State) -> Result<(), String> {
    let file = path(home);
    let json = serde_json::to_string_pretty(state)
        .map_err(|error| format!("cannot write '{}': {error}", display_path(&file)))?;
    atomic_write(&file, json.as_bytes())?;
    set_private_permissions(&file)
}

impl State {
    pub fn link(&self, project: &str) -> Option<&Link> {
        self.links.iter().find(|link| link.project == project)
    }

    pub fn link_mut(&mut self, project: &str) -> Option<&mut Link> {
        self.links.iter_mut().find(|link| link.project == project)
    }

    pub fn set_link(&mut self, link: Link) {
        match self.link_mut(&link.project) {
            Some(existing) => *existing = link,
            None => self.links.push(link),
        }
    }

    pub fn remove_link(&mut self, project: &str) -> bool {
        let before = self.links.len();
        self.links.retain(|link| link.project != project);
        self.links.len() != before
    }

    pub fn link_by_cloud_id(&self, id: &str) -> Option<&Link> {
        self.links.iter().find(|link| link.cloud_id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::process;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_HOME: AtomicUsize = AtomicUsize::new(0);

    fn temp_home() -> PathBuf {
        let home = env::temp_dir().join(format!(
            "keyman-state-test-{}-{}",
            process::id(),
            NEXT_HOME.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        home
    }

    fn link(project: &str, version: i64) -> Link {
        Link {
            project: project.to_string(),
            cloud_id: format!("cloud-{project}"),
            vault_id: "vault-1".to_string(),
            version,
            base: vec![("KEY".to_string(), "value".to_string())],
            cloud_name: project.to_string(),
        }
    }

    #[test]
    fn missing_file_is_empty_state() {
        let home = temp_home();
        assert_eq!(load(&home).unwrap(), State::default());
    }

    #[test]
    fn save_then_load_round_trip() {
        let home = temp_home();
        let state = State {
            user_id: Some("user-1".to_string()),
            email: Some("a@example.com".to_string()),
            links: vec![link("app", 3)],
            refresh_token: Some("token".to_string()),
            personal_vault: Some("vault-0".to_string()),
        };
        save(&home, &state).unwrap();
        assert_eq!(load(&home).unwrap(), state);
    }

    #[test]
    fn set_link_replaces_same_project() {
        let mut state = State::default();
        state.set_link(link("app", 1));
        state.set_link(link("other", 1));
        state.set_link(link("app", 2));
        assert_eq!(state.links.len(), 2);
        assert_eq!(state.link("app").unwrap().version, 2);
        assert_eq!(
            state.link_by_cloud_id("cloud-other").unwrap().project,
            "other"
        );
        state.link_mut("app").unwrap().version = 5;
        assert_eq!(state.link("app").unwrap().version, 5);
    }

    #[test]
    fn remove_link_reports_absence() {
        let mut state = State::default();
        state.set_link(link("app", 1));
        assert!(state.remove_link("app"));
        assert!(!state.remove_link("app"));
        assert!(state.link("app").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home();
        save(&home, &State::default()).unwrap();
        let mode = fs::metadata(path(&home)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_old_file_with_no_personal_vault_loads() {
        let home = temp_home();
        fs::write(path(&home), r#"{"user_id":"u1","email":null,"links":[]}"#).unwrap();
        let st = load(&home).unwrap();
        assert_eq!(st.user_id.as_deref(), Some("u1"));
        assert_eq!(st.personal_vault, None);
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let home = temp_home();
        fs::write(path(&home), "not json").unwrap();
        let error = load(&home).unwrap_err();
        assert!(error.contains(".keyman_sync"), "{error}");
    }
}
