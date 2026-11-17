//! What the app does, without any UI: each function mirrors a `keyman` command
//! and calls the same library code the CLI uses.

use std::path::{Path, PathBuf};

use keyman_cli::envfile::{self, Change};
use keyman_cli::master::{self, Variables};
use keyman_cli::save::{self, Edit, Plan};

pub struct Project {
    pub name: String,
    /// The project's variables, or the rule it breaks.
    pub variables: Result<Variables, String>,
}

pub struct Master {
    pub path: PathBuf,
    pub projects: Vec<Project>,
}

impl Master {
    pub fn project(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|project| project.name == name)
    }

    pub fn problems(&self) -> usize {
        self.projects
            .iter()
            .filter(|project| project.variables.is_err())
            .count()
    }
}

/// `keyman status`: the loaded master file and every project in it.
pub fn read(home: &Path) -> Result<Master, String> {
    let (path, yaml) = master::read_master(home)?;
    let projects = master::project_names(&yaml)?
        .into_iter()
        .map(|name| {
            let variables = master::project_variables(&yaml, &name);
            Project { name, variables }
        })
        .collect();
    Ok(Master { path, projects })
}

/// `keyman init`: creates ~/.keyman.yml with an example project.
pub fn init(home: &Path) -> Result<(), String> {
    master::init(None, home).map(drop)
}

/// `keyman load <file>`.
pub fn load(file: &Path, home: &Path) -> Result<(), String> {
    master::load(file, home).map(drop)
}

pub struct Injected {
    pub path: PathBuf,
    pub change: Change,
    /// Git would commit this .env.
    pub unignored: bool,
}

/// `keyman inject <project> <folder>`.
pub fn inject(variables: &[(String, String)], folder: &Path) -> Result<Injected, String> {
    let (path, change) = envfile::write(variables, folder)?;
    let unignored = envfile::unignored(&path);
    Ok(Injected {
        path,
        change,
        unignored,
    })
}

/// One line of `keyman diff`: a variable, what happened to it, and its value
/// in the master file and in .env (empty where it is missing).
pub struct DiffLine {
    pub name: String,
    pub edit: Edit,
    pub master: String,
    pub env: String,
}

/// `keyman diff <project> <folder>`: master file (old) against .env (new).
pub fn diff(master_vars: &[(String, String)], folder: &Path) -> Result<Vec<DiffLine>, String> {
    let (_, env) = envfile::read(folder)?;
    let value_in = |list: &[(String, String)], name: &str| {
        list.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    Ok(save::plan(master_vars, &env, true)
        .edits
        .into_iter()
        .map(|(name, edit)| DiffLine {
            master: value_in(master_vars, &name),
            env: value_in(&env, &name),
            name,
            edit,
        })
        .collect())
}

pub struct SavePreview {
    pub plan: Plan,
    /// False when saving creates a new project.
    pub exists: bool,
}

/// `keyman save --dry-run <project> <folder>`. Reads the master file fresh so
/// the preview matches what `save_apply` will write.
pub fn save_preview(
    home: &Path,
    project: &str,
    folder: &Path,
    replace: bool,
) -> Result<SavePreview, String> {
    let (_, yaml) = master::read_master(home)?;
    let (env_path, incoming) = envfile::read(folder)?;
    if incoming.is_empty() {
        return Err(format!(
            "'{}' has no variables, so there is nothing to save",
            env_path.display()
        ));
    }
    let exists = master::project_names(&yaml)?
        .iter()
        .any(|name| name == project);
    let existing = if exists {
        master::project_variables(&yaml, project)?
    } else {
        Vec::new()
    };
    Ok(SavePreview {
        plan: save::plan(&existing, &incoming, replace),
        exists,
    })
}

/// `keyman save <project> <folder>`. Returns how many variables changed.
pub fn save_apply(
    home: &Path,
    project: &str,
    folder: &Path,
    replace: bool,
) -> Result<usize, String> {
    let preview = save_preview(home, project, folder, replace)?;
    if preview.exists && preview.plan.changes() == 0 {
        return Ok(0);
    }
    let (path, yaml) = master::read_master(home)?;
    let updated = save::update_project(&yaml, project, &preview.plan, preview.exists)?;
    master::atomic_write(&path, updated.as_bytes())?;
    Ok(preview.plan.changes())
}

/// `keyman export`: `export NAME="value"` lines for a shell.
pub fn export_lines(variables: &[(String, String)]) -> String {
    master::format_variables(variables, "export ")
        .trim_end()
        .to_owned()
}

/// The folder's own name, which `keyman` uses as the default project name.
pub fn folder_name(folder: &Path) -> Option<String> {
    folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// One variable whose master-file value and .env value disagree. `None` means
/// the variable is missing on that side.
#[derive(Clone, PartialEq, Debug)]
pub struct Conflict {
    pub name: String,
    pub master: Option<String>,
    pub env: Option<String>,
}

/// The variables that differ between the master file and FOLDER/.env.
pub fn conflicts(master_vars: &[(String, String)], folder: &Path) -> Result<Vec<Conflict>, String> {
    Ok(diff(master_vars, folder)?
        .into_iter()
        .filter(|line| line.edit != Edit::Unchanged)
        .map(|line| Conflict {
            master: (line.edit != Edit::Added).then_some(line.master),
            env: (line.edit != Edit::Removed).then_some(line.env),
            name: line.name,
        })
        .collect())
}

/// Applies resolved conflicts: each name maps to the value both sides should
/// hold, or `None` to drop the variable. The master file's project and the
/// folder's .env are both rewritten to the result, so they end up in sync.
/// Returns the project's variables after the sync.
pub fn resolve(
    home: &Path,
    project: &str,
    folder: &Path,
    resolved: &[(String, Option<String>)],
) -> Result<Variables, String> {
    for (name, value) in resolved {
        if !master::valid_variable_name(name) {
            return Err(format!("'{name}' is not a valid variable name"));
        }
        if value
            .as_ref()
            .is_some_and(|v| v.contains(['\n', '\r', '\0']))
        {
            return Err(format!(
                "the value for '{name}' cannot contain a line break"
            ));
        }
    }
    let (path, yaml) = master::read_master(home)?;
    let existing = master::project_variables(&yaml, project)?;
    let choice = |name: &str| {
        resolved
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    let mut result: Variables = existing
        .iter()
        .filter_map(|(name, value)| match choice(name) {
            Some(Some(v)) => Some((name.clone(), v)),
            Some(None) => None,
            None => Some((name.clone(), value.clone())),
        })
        .collect();
    for (name, value) in resolved {
        if let Some(value) = value
            && !existing.iter().any(|(n, _)| n == name)
        {
            result.push((name.clone(), value.clone()));
        }
    }
    let plan = save::plan(&existing, &result, true);
    if plan.changes() > 0 {
        let updated = save::update_project(&yaml, project, &plan, true)?;
        master::atomic_write(&path, updated.as_bytes())?;
    }
    envfile::write(&result, folder)?;
    Ok(result)
}

/// How a tied folder's .env stands against the master file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderStatus {
    /// The folder no longer exists.
    Missing,
    /// The folder has no .env yet.
    NoEnv,
    InSync,
    /// This many variables differ (changed, only in .env, or only in master).
    Differs(usize),
    /// .env exists but cannot be read as variables.
    Unreadable,
}

pub fn folder_status(master_vars: &[(String, String)], folder: &Path) -> FolderStatus {
    if !folder.is_dir() {
        return FolderStatus::Missing;
    }
    if !folder.join(".env").is_file() {
        return FolderStatus::NoEnv;
    }
    match diff(master_vars, folder) {
        Ok(lines) => match lines
            .iter()
            .filter(|line| line.edit != Edit::Unchanged)
            .count()
        {
            0 => FolderStatus::InSync,
            n => FolderStatus::Differs(n),
        },
        Err(_) => FolderStatus::Unreadable,
    }
}

/// True when FOLDER/.env exists and writing these variables would change it.
pub fn env_would_change(variables: &[(String, String)], folder: &Path) -> bool {
    std::fs::read(folder.join(".env"))
        .is_ok_and(|existing| existing != master::format_variables(variables, "").as_bytes())
}

/// Shows the file selected in Finder or Explorer.
pub fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(path);
        command
    } else if cfg!(windows) {
        let mut command = std::process::Command::new("explorer");
        command.arg(format!("/select,{}", path.display()));
        command
    } else {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
        command
    };
    command
        .spawn()
        .map(drop)
        .map_err(|error| format!("cannot open the file manager: {error}"))
}

/// `keyman edit`, for a desktop: opens the master file in the system's text editor.
pub fn open_in_editor(path: &Path) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg("-t");
        command
    } else if cfg!(windows) {
        std::process::Command::new("notepad")
    } else {
        std::process::Command::new("xdg-open")
    };
    command
        .arg(path)
        .spawn()
        .map(drop)
        .map_err(|error| format!("cannot open an editor: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir =
                std::env::temp_dir().join(format!("keyman-app-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("my-app")).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn init_inject_diff_save_round_trip() {
        let scratch = Scratch::new("round-trip");
        let home = &scratch.0;
        let folder = home.join("my-app");

        init(home).unwrap();
        let loaded = read(home).unwrap();
        let vars = loaded.project("my-app").unwrap().variables.clone().unwrap();
        assert_eq!(vars.len(), 3);

        let injected = inject(&vars, &folder).unwrap();
        assert_eq!(injected.change, Change::Created);
        assert_eq!(inject(&vars, &folder).unwrap().change, Change::Unchanged);
        assert!(
            diff(&vars, &folder)
                .unwrap()
                .iter()
                .all(|line| line.edit == Edit::Unchanged)
        );

        fs::write(folder.join(".env"), "API_KEY=\"rotated\"\nNEW_FLAG=\"1\"\n").unwrap();
        let lines = diff(&vars, &folder).unwrap();
        let edit_of = |name: &str| lines.iter().find(|line| line.name == name).unwrap().edit;
        assert_eq!(edit_of("API_KEY"), Edit::Changed);
        assert_eq!(edit_of("NEW_FLAG"), Edit::Added);
        assert_eq!(edit_of("DEBUG"), Edit::Removed);

        let preview = save_preview(home, "my-app", &folder, false).unwrap();
        assert!(preview.exists);
        assert_eq!(preview.plan.count(Edit::Kept), 2);
        assert_eq!(save_apply(home, "my-app", &folder, false).unwrap(), 2);

        let saved = read(home)
            .unwrap()
            .project("my-app")
            .unwrap()
            .variables
            .clone()
            .unwrap();
        assert!(saved.contains(&("API_KEY".into(), "rotated".into())));
        assert!(saved.contains(&("NEW_FLAG".into(), "1".into())));
        assert!(saved.contains(&("DEBUG".into(), "true".into())));
        assert_eq!(save_apply(home, "my-app", &folder, false).unwrap(), 0);
    }

    #[test]
    fn resolving_conflicts_syncs_master_and_env() {
        let scratch = Scratch::new("resolve");
        let home = &scratch.0;
        let folder = home.join("my-app");
        init(home).unwrap();
        // The template has DATABASE_URL, API_KEY and DEBUG.
        fs::write(
            folder.join(".env"),
            "DATABASE_URL=\"postgres://user:password@localhost:5432/my_app\"\nAPI_KEY=\"from-env\"\nNEW_FLAG=\"1\"\nOTHER=\"x\"\n",
        )
        .unwrap();
        let vars = read(home)
            .unwrap()
            .project("my-app")
            .unwrap()
            .variables
            .clone()
            .unwrap();
        let found = conflicts(&vars, &folder).unwrap();
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["API_KEY", "DEBUG", "NEW_FLAG", "OTHER"]);
        assert_eq!(found[0].env.as_deref(), Some("from-env"));
        assert_eq!(found[1].env, None);
        assert_eq!(found[2].master, None);

        // Keep the .env key, keep master's DEBUG, edit NEW_FLAG, drop OTHER.
        let resolved = vec![
            ("API_KEY".to_owned(), Some("from-env".to_owned())),
            ("DEBUG".to_owned(), Some("true".to_owned())),
            ("NEW_FLAG".to_owned(), Some("edited".to_owned())),
            ("OTHER".to_owned(), None),
        ];
        let result = resolve(home, "my-app", &folder, &resolved).unwrap();
        let saved = read(home)
            .unwrap()
            .project("my-app")
            .unwrap()
            .variables
            .clone()
            .unwrap();
        assert_eq!(saved, result);
        assert!(saved.contains(&("API_KEY".into(), "from-env".into())));
        assert!(saved.contains(&("NEW_FLAG".into(), "edited".into())));
        assert!(!saved.iter().any(|(n, _)| n == "OTHER"));
        assert_eq!(folder_status(&saved, &folder), FolderStatus::InSync);

        let bad = vec![("NEW_FLAG".to_owned(), Some("two\nlines".to_owned()))];
        assert!(resolve(home, "my-app", &folder, &bad).is_err());
    }

    #[test]
    fn folder_status_covers_each_state() {
        let scratch = Scratch::new("status");
        let folder = scratch.0.join("my-app");
        let vars = vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "2".to_owned()),
        ];
        assert_eq!(
            folder_status(&vars, &scratch.0.join("gone")),
            FolderStatus::Missing
        );
        assert_eq!(folder_status(&vars, &folder), FolderStatus::NoEnv);
        inject(&vars, &folder).unwrap();
        assert_eq!(folder_status(&vars, &folder), FolderStatus::InSync);
        fs::write(folder.join(".env"), "A=\"changed\"\nC=\"3\"\n").unwrap();
        assert_eq!(folder_status(&vars, &folder), FolderStatus::Differs(3));
        fs::write(folder.join(".env"), "not a dotenv line\n").unwrap();
        assert_eq!(folder_status(&vars, &folder), FolderStatus::Unreadable);
    }

    #[test]
    fn save_creates_a_new_project() {
        let scratch = Scratch::new("new-project");
        let home = &scratch.0;
        let folder = home.join("other");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(".env"), "TOKEN=\"abc\"\n").unwrap();
        init(home).unwrap();

        assert!(!save_preview(home, "other", &folder, false).unwrap().exists);
        save_apply(home, "other", &folder, false).unwrap();
        let master = read(home).unwrap();
        assert_eq!(
            master.project("other").unwrap().variables.clone().unwrap(),
            vec![("TOKEN".into(), "abc".into())]
        );
        assert!(master.project("my-app").is_some());
    }
}
