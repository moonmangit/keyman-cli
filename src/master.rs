//! The master YAML file: where it is, how it is read and validated, and how
//! variables are written out.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use yaml_rust2::{Yaml, YamlLoader};

use crate::ui::display_path;

pub type Variables = Vec<(String, String)>;

const TEMPLATE: &str = "\
# Keyman master file. It holds secrets, so keep it private and out of Git.
#
# Each top-level key is a project. When you omit the project name, Keyman
# uses the current folder name, so name projects after their folders.
# Quote every value, including numbers and true/false.
#
# A project can have stages. Variables directly under it are shared; a stage
# lists only what it adds or changes. Use one as 'keyman inject my-app:staging'.
#
#   my-app:
#     DEBUG: \"false\"
#     production:
#       DATABASE_URL: \"postgres://prod.internal/app\"

my-app:
  DATABASE_URL: \"postgres://user:password@localhost:5432/my_app\"
  API_KEY: \"replace-me\"
  DEBUG: \"true\"
";

#[cfg(not(windows))]
pub fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "cannot find the home directory (HOME or USERPROFILE is not set)".to_owned())
}

#[cfg(windows)]
pub fn home_dir() -> Result<PathBuf, String> {
    // Git Bash may set HOME to an MSYS path such as /c/Users/name, which a
    // native Windows executable cannot use directly.
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| "cannot find the home directory (USERPROFILE is not set)".to_owned())
}

#[derive(Debug)]
pub struct Loaded {
    pub path: PathBuf,
    pub projects: usize,
    pub problems: Vec<(String, String)>,
}

/// Validates the master file and records its absolute path in ~/.keyman_config.
pub fn load(file_path: &Path, home: &Path) -> Result<Loaded, String> {
    check_extension(file_path)?;
    let master = fs::canonicalize(file_path).map_err(|error| {
        format!(
            "cannot find YAML file '{}': {error}",
            display_path(file_path)
        )
    })?;
    if !master.is_file() {
        return Err(format!("'{}' is not a file", display_path(&master)));
    }
    let yaml = fs::read_to_string(&master)
        .map_err(|error| format!("cannot read '{}': {error}", display_path(&master)))?;
    let (projects, problems) = validate(&yaml)?;

    // The master file contains secrets. Keep it private as required by the spec.
    set_private_permissions(&master)?;
    atomic_write(
        &home.join(".keyman_config"),
        format!("{}\n", master.display()).as_bytes(),
    )?;
    Ok(Loaded {
        path: master,
        projects,
        problems,
    })
}

/// Creates a starter master file and loads it. Never overwrites a file.
pub fn init(file_path: Option<&Path>, home: &Path) -> Result<Loaded, String> {
    let path = file_path.map_or_else(|| home.join(".keyman.yml"), Path::to_path_buf);
    check_extension(&path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            format!(
                "'{shown}' already exists, and Keyman does not overwrite it\n\nRun 'keyman load {shown}' to use it.",
                shown = display_path(&path)
            )
        } else {
            format!("cannot create '{}': {error}", display_path(&path))
        }
    })?;
    file.write_all(TEMPLATE.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write '{}': {error}", display_path(&path)))?;
    drop(file);
    load(&path, home)
}

fn check_extension(path: &Path) -> Result<(), String> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("yml" | "yaml") => Ok(()),
        _ => Err("the master file must have a .yml or .yaml extension".to_owned()),
    }
}

/// Returns the loaded master file path and its contents.
pub fn read_master(home: &Path) -> Result<(PathBuf, String), String> {
    let config = home.join(".keyman_config");
    let master_path = match fs::read_to_string(&config) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err("no master file is loaded\n\nRun 'keyman init' to create one, or 'keyman load <file>' to use an existing file.".to_owned());
        }
        Err(error) => {
            return Err(format!("cannot read '{}': {error}", display_path(&config)));
        }
    };
    let master_path = master_path.trim_end_matches(['\r', '\n']);
    if master_path.is_empty() {
        return Err(format!(
            "'{}' is empty\n\nRun 'keyman load <file>' again.",
            display_path(&config)
        ));
    }

    let master = PathBuf::from(master_path);
    let yaml = fs::read_to_string(&master).map_err(|error| {
        format!(
            "cannot read the master file '{}': {error}\n\nRun 'keyman load <file>' if it moved.",
            display_path(&master)
        )
    })?;
    Ok((master, yaml))
}

fn master_document(yaml: &str) -> Result<Yaml, String> {
    let mut documents =
        YamlLoader::load_from_str(yaml).map_err(|error| format!("invalid master YAML: {error}"))?;
    if documents.len() != 1 {
        return Err("the master YAML must contain exactly one document".to_owned());
    }
    Ok(documents.remove(0))
}

pub fn project_names(yaml: &str) -> Result<Vec<String>, String> {
    let document = master_document(yaml)?;
    if document.is_null() {
        return Ok(Vec::new());
    }
    let projects = document
        .as_hash()
        .ok_or_else(|| "the master YAML must be a mapping of project names".to_owned())?;
    projects
        .keys()
        .map(|key| {
            key.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "the master YAML has a project name that is not a string".to_owned())
        })
        .collect()
}

/// Checks every project. Returns the project count and each project's problem.
/// Fails only when the file as a whole cannot be read as projects.
pub fn validate(yaml: &str) -> Result<(usize, Vec<(String, String)>), String> {
    let names = project_names(yaml)?;
    let problems = names
        .iter()
        .filter_map(|name| {
            read_project(yaml, name)
                .err()
                .map(|error| (name.clone(), error))
        })
        .collect();
    Ok((names.len(), problems))
}

/// A project: variables shared by every stage, and optional stages that each
/// list only the variables they add or override.
///
/// ```yaml
/// my-app:
///   DEBUG: "false"        # shared
///   production:           # a stage
///     DATABASE_URL: "..."
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Project {
    pub shared: Variables,
    pub stages: Vec<(String, Variables)>,
}

/// Separates a project from its stage: `my-app:staging`.
pub const STAGE_SEPARATOR: char = ':';

impl Project {
    pub fn has_stages(&self) -> bool {
        !self.stages.is_empty()
    }

    pub fn stage_names(&self) -> Vec<String> {
        self.stages.iter().map(|(name, _)| name.clone()).collect()
    }

    pub fn stage(&self, name: &str) -> Option<&Variables> {
        self.stages
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, variables)| variables)
    }

    /// The variables a stage writes: the shared ones, with the stage's own
    /// values over them, then the stage's extra variables.
    pub fn effective(&self, stage: &str) -> Option<Variables> {
        Some(overlay(&self.shared, self.stage(stage)?))
    }

    /// One list for the whole project, as cloud sync stores and merges it:
    /// shared variables by name, then for each stage a `stage:` marker (so an
    /// empty stage survives) and its variables as `stage:NAME`.
    pub fn flat(&self) -> Variables {
        let mut flat = self.shared.clone();
        for (stage, variables) in &self.stages {
            flat.push((format!("{stage}{STAGE_SEPARATOR}"), String::new()));
            for (name, value) in variables {
                flat.push((format!("{stage}{STAGE_SEPARATOR}{name}"), value.clone()));
            }
        }
        flat
    }

    /// The reverse of `flat`. A stage exists when its marker or any of its
    /// variables is in the list.
    pub fn from_flat(flat: &[(String, String)]) -> Result<Project, String> {
        let mut project = Project::default();
        for (key, value) in flat {
            match key.split_once(STAGE_SEPARATOR) {
                None => {
                    check_variable(key, value)?;
                    project.shared.push((key.clone(), value.clone()));
                }
                Some((stage, name)) => {
                    if !valid_stage_name(stage) {
                        return Err(format!("'{stage}' is not a valid stage name"));
                    }
                    let index = match project.stages.iter().position(|(s, _)| s == stage) {
                        Some(index) => index,
                        None => {
                            project.stages.push((stage.to_owned(), Vec::new()));
                            project.stages.len() - 1
                        }
                    };
                    if !name.is_empty() {
                        check_variable(name, value)?;
                        project.stages[index]
                            .1
                            .push((name.to_owned(), value.clone()));
                    }
                }
            }
        }
        if let Some((stage, _)) = project
            .stages
            .iter()
            .find(|(stage, _)| project.shared.iter().any(|(name, _)| name == stage))
        {
            return Err(format!(
                "'{stage}' cannot be both a variable and a stage of the same project"
            ));
        }
        Ok(project)
    }
}

/// `base` with `over` on top: changed values in place, new names at the end.
pub fn overlay(base: &[(String, String)], over: &[(String, String)]) -> Variables {
    let mut result = base.to_vec();
    for (name, value) in over {
        match result.iter_mut().find(|(candidate, _)| candidate == name) {
            Some(entry) => entry.1 = value.clone(),
            None => result.push((name.clone(), value.clone())),
        }
    }
    result
}

/// Stage names: letters, digits, '-', '_' and '.', starting with a letter or digit.
pub fn valid_stage_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

fn check_variable(name: &str, value: &str) -> Result<(), String> {
    if !valid_variable_name(name) {
        return Err(format!(
            "'{name}' is not a valid variable name (use letters, digits, and underscores, and do not start with a digit)"
        ));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err(format!(
            "variable '{name}' contains a line break or NUL character"
        ));
    }
    Ok(())
}

/// Reads one variable mapping entry. `stage` names the stage, for messages.
fn read_variable(key: &Yaml, value: &Yaml) -> Result<(String, String), String> {
    let name = key
        .as_str()
        .ok_or_else(|| "a variable name is not a string".to_owned())?;
    let value = value.as_str().ok_or_else(|| {
        format!("variable '{name}' must be a quoted string, for example {name}: \"value\"")
    })?;
    check_variable(name, value)?;
    Ok((name.to_owned(), value.to_owned()))
}

/// Reads a whole project, with its stages.
pub fn read_project(yaml: &str, project_name: &str) -> Result<Project, String> {
    let document = master_document(yaml)?;
    let project = document
        .as_hash()
        .and_then(|projects| projects.get(&Yaml::String(project_name.to_owned())))
        .ok_or_else(|| format!("project '{project_name}' was not found in the master file"))?;
    if project.is_null() {
        return Ok(Project::default());
    }
    let entries = project
        .as_hash()
        .ok_or_else(|| format!("project '{project_name}' must be a mapping of variables"))?;

    let mut output = Project::default();
    for (key, value) in entries {
        if value.as_hash().is_some() || value.is_null() {
            let stage = key
                .as_str()
                .ok_or_else(|| "a stage name is not a string".to_owned())?;
            if !valid_stage_name(stage) {
                return Err(format!(
                    "'{stage}' is not a valid stage name (use letters, digits, '-', '_' and '.')"
                ));
            }
            let mut variables = Vec::new();
            if let Some(stage_entries) = value.as_hash() {
                for (key, value) in stage_entries {
                    variables.push(
                        read_variable(key, value)
                            .map_err(|error| format!("stage '{stage}': {error}"))?,
                    );
                }
            }
            output.stages.push((stage.to_owned(), variables));
        } else {
            output.shared.push(read_variable(key, value)?);
        }
    }
    Ok(output)
}

/// Splits `my-app:staging` into the project and the stage. A project whose own
/// name contains ':' is matched whole first.
pub fn split_target<'a>(target: &'a str, names: &[String]) -> (&'a str, Option<&'a str>) {
    if names.iter().any(|name| name == target) {
        return (target, None);
    }
    match target.rsplit_once(STAGE_SEPARATOR) {
        Some((project, stage)) => (project, Some(stage)),
        None => (target, None),
    }
}

/// The variables to write for a project, or for one of its stages. A project
/// with stages needs a stage.
pub fn stage_variables(
    project: &Project,
    project_name: &str,
    stage: Option<&str>,
) -> Result<Variables, String> {
    match stage {
        Some(stage) => project.effective(stage).ok_or_else(|| {
            if project.has_stages() {
                format!(
                    "project '{project_name}' has no stage '{stage}'{}\n\nIts stages: {}.",
                    crate::ui::did_you_mean(stage, project.stages.iter().map(|(s, _)| s.as_str())),
                    project.stage_names().join(", ")
                )
            } else {
                format!("project '{project_name}' has no stages, so use '{project_name}' without ':{stage}'")
            }
        }),
        None if project.has_stages() => {
            let names = project.stage_names();
            Err(format!(
                "'{project_name}' has stages: {}\n\nName one, for example '{project_name}{STAGE_SEPARATOR}{}'.",
                names.join(", "),
                names[0]
            ))
        }
        None => Ok(project.shared.clone()),
    }
}

/// A whole project as one list, stages included (see `Project::flat`). Cloud
/// sync stores, merges and compares projects in this form.
pub fn project_flat(yaml: &str, project_name: &str) -> Result<Variables, String> {
    Ok(read_project(yaml, project_name)?.flat())
}

/// The variables for `target`: a project name, or `project:stage`.
pub fn project_variables(yaml: &str, target: &str) -> Result<Variables, String> {
    let names = project_names(yaml)?;
    let (project_name, stage) = split_target(target, &names);
    let project = read_project(yaml, project_name)?;
    stage_variables(&project, project_name, stage)
}

pub fn render_project(yaml: &str, project_name: &str) -> Result<String, String> {
    Ok(format_variables(
        &project_variables(yaml, project_name)?,
        "",
    ))
}

/// Formats variables as double-quoted lines that both dotenv parsers and
/// POSIX shells read the same way.
pub fn format_variables(variables: &[(String, String)], prefix: &str) -> String {
    let mut output = String::new();
    for (name, value) in variables {
        output.push_str(prefix);
        output.push_str(name);
        output.push_str("=\"");
        for character in value.chars() {
            if matches!(character, '\\' | '"' | '$' | '`') {
                output.push('\\');
            }
            output.push(character);
        }
        output.push_str("\"\n");
    }
    output
}

pub fn valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid path: {}", display_path(path)))?;
    let mut temporary = None;
    for attempt in 0..10 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("cannot get current time: {error}"))?
            .as_nanos();
        let candidate = parent.join(format!(".keyman-{}-{nanos}-{attempt}.tmp", process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create a file in '{}': {error}",
                    display_path(parent)
                ))
            }
        }
    }

    let (temporary_path, mut file) = temporary.ok_or_else(|| {
        format!(
            "cannot create a temporary file in '{}'",
            display_path(parent)
        )
    })?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.map_err(|error| format!("cannot write '{}': {error}", display_path(path)))
}

#[cfg(unix)]
pub fn set_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!(
            "cannot set 600 permissions on '{}': {error}",
            display_path(path)
        )
    })
}

#[cfg(not(unix))]
pub fn set_private_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "app:\n  DEBUG: \"false\"\n  LOG: \"info\"\n  production:\n    DB: \"prod\"\n  staging:\n    DB: \"stage\"\n    DEBUG: \"true\"\n  pre-production:\nplain:\n  A: \"1\"\n\"odd:name\":\n  B: \"2\"\n";

    fn v(list: &[(&str, &str)]) -> Variables {
        list.iter()
            .map(|(n, v)| ((*n).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn reads_stages_with_shared_values() {
        let project = read_project(YAML, "app").unwrap();
        assert_eq!(
            project.stage_names(),
            ["production", "staging", "pre-production"]
        );
        assert_eq!(
            project_variables(YAML, "app:staging").unwrap(),
            v(&[("DEBUG", "true"), ("LOG", "info"), ("DB", "stage")])
        );
        assert_eq!(
            project_variables(YAML, "app:pre-production").unwrap(),
            v(&[("DEBUG", "false"), ("LOG", "info")])
        );
        let error = project_variables(YAML, "app").unwrap_err();
        assert!(
            error.contains("has stages: production, staging, pre-production"),
            "{error}"
        );
        assert!(project_variables(YAML, "app:stagin")
            .unwrap_err()
            .contains("Did you mean 'staging'"));
        assert!(project_variables(YAML, "app:prod")
            .unwrap_err()
            .contains("Its stages: production, staging, pre-production"));
        assert!(project_variables(YAML, "plain:prod")
            .unwrap_err()
            .contains("has no stages"));
        assert_eq!(project_variables(YAML, "plain").unwrap(), v(&[("A", "1")]));
        // A project with ':' in its own name still works.
        assert_eq!(
            project_variables(YAML, "odd:name").unwrap(),
            v(&[("B", "2")])
        );
        assert_eq!(validate(YAML).unwrap(), (3, vec![]));
    }

    #[test]
    fn rejects_bad_stages() {
        assert!(read_project("p:\n  -bad:\n    A: \"1\"\n", "p").is_err());
        assert!(read_project("p:\n  s:\n    A: true\n", "p")
            .unwrap_err()
            .contains("stage 's'"));
        assert!(read_project("p:\n  s:\n    t:\n      A: \"1\"\n", "p").is_err());
    }

    #[test]
    fn flat_round_trip() {
        let project = read_project(YAML, "app").unwrap();
        let flat = project.flat();
        assert!(flat.contains(&("staging:DB".to_owned(), "stage".to_owned())));
        assert!(flat.contains(&("pre-production:".to_owned(), String::new())));
        assert_eq!(Project::from_flat(&flat).unwrap(), project);
        // A stage var without its marker still makes the stage.
        assert_eq!(
            Project::from_flat(&v(&[("s:A", "1")])).unwrap().stages,
            vec![("s".to_owned(), v(&[("A", "1")]))]
        );
        assert!(Project::from_flat(&v(&[("s", "1"), ("s:", "")])).is_err());
        assert!(Project::from_flat(&v(&[("bad name", "1")])).is_err());
    }
}
