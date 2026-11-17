//! A project folder's `.env` file: reading it, writing it, and checking that
//! Git ignores it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::master::{self, Variables};
use crate::save;
use crate::ui::display_path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Change {
    Created,
    Replaced,
    Unchanged,
}

/// Reads DIRECTORY/.env.
pub fn read(directory: &Path) -> Result<(PathBuf, Variables), String> {
    let env_path = directory.join(".env");
    let text = fs::read_to_string(&env_path)
        .map_err(|error| format!("cannot read '{}': {error}", display_path(&env_path)))?;
    let variables = save::parse_dotenv(&text)?;
    Ok((env_path, variables))
}

/// Writes the variables to DIRECTORY/.env with private permissions.
pub fn write(
    variables: &[(String, String)],
    target_directory: &Path,
) -> Result<(PathBuf, Change), String> {
    let output = master::format_variables(variables, "");
    let target = fs::canonicalize(target_directory).map_err(|error| {
        format!(
            "target directory '{}' does not exist: {error}",
            display_path(target_directory)
        )
    })?;
    if !target.is_dir() {
        return Err(format!("'{}' is not a directory", display_path(&target)));
    }

    let path = target.join(".env");
    let change = match fs::read(&path) {
        Ok(existing) if existing == output.as_bytes() => Change::Unchanged,
        Ok(_) => Change::Replaced,
        Err(_) => Change::Created,
    };
    if change == Change::Unchanged {
        master::set_private_permissions(&path)?;
    } else {
        master::atomic_write(&path, output.as_bytes())?;
    }
    Ok((path, change))
}

/// True only when Git reports that the .env file is inside a repository and
/// not ignored. Missing Git or a non-repository directory gives false.
pub fn unignored(env_file: &Path) -> bool {
    let Some(directory) = env_file.parent() else {
        return false;
    };
    Command::new("git")
        .args(["check-ignore", "-q", ".env"])
        .current_dir(directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.code() == Some(1))
}
