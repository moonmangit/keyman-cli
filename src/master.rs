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
            project_variables(yaml, name)
                .err()
                .map(|error| (name.clone(), error))
        })
        .collect();
    Ok((names.len(), problems))
}

pub fn project_variables(yaml: &str, project_name: &str) -> Result<Variables, String> {
    let document = master_document(yaml)?;
    let project = document
        .as_hash()
        .and_then(|projects| projects.get(&Yaml::String(project_name.to_owned())))
        .ok_or_else(|| format!("project '{project_name}' was not found in the master file"))?;
    if project.is_null() {
        return Ok(Vec::new());
    }
    let variables = project
        .as_hash()
        .ok_or_else(|| format!("project '{project_name}' must be a mapping of variables"))?;

    let mut output = Vec::with_capacity(variables.len());
    for (key, value) in variables {
        let name = key
            .as_str()
            .ok_or_else(|| "a variable name is not a string".to_owned())?;
        if !valid_variable_name(name) {
            return Err(format!(
                "'{name}' is not a valid variable name (use letters, digits, and underscores, and do not start with a digit)"
            ));
        }
        let value = value.as_str().ok_or_else(|| {
            format!("variable '{name}' must be a quoted string, for example {name}: \"value\"")
        })?;
        if value.contains(['\n', '\r', '\0']) {
            return Err(format!(
                "variable '{name}' contains a line break or NUL character"
            ));
        }
        output.push((name.to_owned(), value.to_owned()));
    }
    Ok(output)
}

#[cfg(test)]
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
