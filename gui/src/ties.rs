//! Tied folders: each project can be tied to several local folders (a repo
//! checkout, a worktree, a deploy copy), one of which is active.

use std::collections::BTreeMap;

use rune::prefs::Prefs;

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Ties {
    /// Project name to its tied folders, in the order they were tied.
    folders: BTreeMap<String, Vec<String>>,
    /// Project name to the folder actions use.
    active: BTreeMap<String, String>,
}

impl Ties {
    /// Reads the stored ties, upgrading the one-folder-per-project format.
    pub fn load(prefs: &Prefs) -> Ties {
        let folders = prefs
            .get::<BTreeMap<String, Vec<String>>>("ties")
            .unwrap_or_else(|| {
                prefs
                    .get::<BTreeMap<String, String>>("folders")
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(project, folder)| (project, vec![folder]))
                    .collect()
            });
        let active = prefs.get("active").unwrap_or_default();
        Ties { folders, active }
    }

    pub fn save(&self, prefs: &mut Prefs) -> Result<(), String> {
        prefs
            .set("ties", &self.folders)
            .and_then(|()| prefs.set("active", &self.active))
            .and_then(|()| prefs.remove("folders"))
            .map_err(|error| format!("cannot remember tied folders: {error:?}"))
    }

    pub fn forget(prefs: &mut Prefs) -> Result<(), String> {
        ["ties", "active", "folders"]
            .iter()
            .try_for_each(|key| prefs.remove(key))
            .map_err(|error| format!("cannot forget tied folders: {error:?}"))
    }

    pub fn folders(&self, project: &str) -> &[String] {
        self.folders.get(project).map_or(&[], Vec::as_slice)
    }

    /// The folder actions use: the chosen one while it is still tied, else the first.
    pub fn active(&self, project: &str) -> Option<&str> {
        let folders = self.folders(project);
        self.active
            .get(project)
            .filter(|folder| folders.contains(folder))
            .or_else(|| folders.first())
            .map(String::as_str)
    }

    /// Ties a folder (once) and makes it active.
    pub fn tie(&mut self, project: &str, folder: &str) {
        let folders = self.folders.entry(project.to_owned()).or_default();
        if !folders.iter().any(|f| f == folder) {
            folders.push(folder.to_owned());
        }
        self.active.insert(project.to_owned(), folder.to_owned());
    }

    pub fn untie(&mut self, project: &str, folder: &str) {
        if let Some(folders) = self.folders.get_mut(project) {
            folders.retain(|f| f != folder);
            if folders.is_empty() {
                self.folders.remove(project);
            }
        }
        if self.active.get(project).is_some_and(|f| f == folder) {
            self.active.remove(project);
        }
    }

    pub fn set_active(&mut self, project: &str, folder: &str) {
        if self.folders(project).iter().any(|f| f == folder) {
            self.active.insert(project.to_owned(), folder.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tie_untie_and_active_folder() {
        let mut ties = Ties::default();
        assert_eq!(ties.active("app"), None);
        ties.tie("app", "/a");
        ties.tie("app", "/b");
        ties.tie("app", "/a");
        assert_eq!(ties.folders("app"), ["/a", "/b"]);
        assert_eq!(ties.active("app"), Some("/a"));
        ties.set_active("app", "/b");
        assert_eq!(ties.active("app"), Some("/b"));
        ties.set_active("app", "/not-tied");
        assert_eq!(ties.active("app"), Some("/b"));
        ties.untie("app", "/b");
        assert_eq!(ties.active("app"), Some("/a"));
        ties.untie("app", "/a");
        assert!(ties.folders("app").is_empty());
    }

    #[test]
    fn old_single_folders_are_upgraded_and_saved_as_ties() {
        let mut prefs = Prefs::memory();
        let old: BTreeMap<String, String> = [("app".to_owned(), "/code/app".to_owned())].into();
        prefs.set("folders", &old).unwrap();
        let mut ties = Ties::load(&prefs);
        assert_eq!(ties.folders("app"), ["/code/app"]);
        ties.tie("app", "/code/app-worktree");
        ties.save(&mut prefs).unwrap();
        assert!(!prefs.contains("folders"));
        assert_eq!(Ties::load(&prefs), ties);
    }
}
