use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use yaml_rust2::{Yaml, YamlLoader};

fn main() {
    const USAGE: &str =
        "Usage:\n  keyman load <file_path>\n  keyman inject <project_name> <target_directory>";
    let args: Vec<_> = env::args_os().collect();
    let result = match args.get(1).and_then(|arg| arg.to_str()) {
        Some("--help" | "-h" | "help") if args.len() == 2 => Ok(USAGE.to_owned()),
        Some("--version" | "-V") if args.len() == 2 => {
            Ok(format!("keyman {}", env!("CARGO_PKG_VERSION")))
        }
        Some("load") if args.len() == 3 => {
            let home = home_dir();
            home.and_then(|home| load(Path::new(&args[2]), &home))
                .map(|path| format!("Loaded master YAML file: {}", path.display()))
        }
        Some("inject") if args.len() == 4 => {
            let home = home_dir();
            home.and_then(|home| inject(&args[2].to_string_lossy(), Path::new(&args[3]), &home))
                .map(|path| format!("Created {}", path.display()))
        }
        _ => Err(USAGE.to_owned()),
    };

    match result {
        Ok(message) => println!("{message}"),
        Err(message) => {
            eprintln!("Error: {message}");
            process::exit(1);
        }
    }
}

#[cfg(not(windows))]
fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            "Cannot find the home directory (HOME or USERPROFILE is not set).".to_owned()
        })
}

#[cfg(windows)]
fn home_dir() -> Result<PathBuf, String> {
    // Git Bash may set HOME to an MSYS path such as /c/Users/name, which a
    // native Windows executable cannot use directly.
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| "Cannot find the home directory (USERPROFILE is not set).".to_owned())
}

fn load(file_path: &Path, home: &Path) -> Result<PathBuf, String> {
    if file_path.extension().and_then(|ext| ext.to_str()) != Some("yml") {
        return Err("The master configuration file must have a .yml extension.".to_owned());
    }

    let master = fs::canonicalize(file_path)
        .map_err(|error| format!("Cannot find YAML file '{}': {error}", file_path.display()))?;
    if !master.is_file() {
        return Err(format!("YAML path '{}' is not a file.", master.display()));
    }

    // The master file contains secrets. Keep it private as required by the spec.
    set_private_permissions(&master)?;
    let config = home.join(".keyman_config");
    atomic_write(&config, format!("{}\n", master.display()).as_bytes())?;
    Ok(master)
}

fn inject(project_name: &str, target_directory: &Path, home: &Path) -> Result<PathBuf, String> {
    let config = home.join(".keyman_config");
    let master_path = fs::read_to_string(&config).map_err(|error| {
        format!(
            "Cannot read {}. Run 'keyman load <file_path>' first: {error}",
            config.display()
        )
    })?;
    let master_path = master_path.trim_end_matches(['\r', '\n']);
    if master_path.is_empty() {
        return Err(format!(
            "{} is empty. Run 'keyman load <file_path>' again.",
            config.display()
        ));
    }

    let yaml = fs::read_to_string(master_path)
        .map_err(|error| format!("Cannot read master YAML file '{master_path}': {error}"))?;
    let output = render_project(&yaml, project_name)?;

    let target = fs::canonicalize(target_directory).map_err(|error| {
        format!(
            "Target directory '{}' does not exist: {error}",
            target_directory.display()
        )
    })?;
    if !target.is_dir() {
        return Err(format!("Target '{}' is not a directory.", target.display()));
    }

    let env_file = target.join(".env");
    atomic_write(&env_file, output.as_bytes())?;
    Ok(env_file)
}

fn render_project(yaml: &str, project_name: &str) -> Result<String, String> {
    let documents =
        YamlLoader::load_from_str(yaml).map_err(|error| format!("Invalid master YAML: {error}"))?;
    if documents.len() != 1 {
        return Err("Master YAML must contain exactly one document.".to_owned());
    }
    let projects = documents[0]
        .as_hash()
        .ok_or_else(|| "Master YAML must contain a mapping of project names.".to_owned())?;
    let project = projects
        .get(&Yaml::String(project_name.to_owned()))
        .ok_or_else(|| {
            format!("Project '{project_name}' was not found in the master YAML file.")
        })?;
    let variables = project
        .as_hash()
        .ok_or_else(|| format!("Project '{project_name}' must contain a mapping of variables."))?;

    let mut output = String::new();
    for (key, value) in variables {
        let name = key
            .as_str()
            .ok_or_else(|| format!("Project '{project_name}' has a non-string variable name."))?;
        if !valid_variable_name(name) {
            return Err(format!(
                "'{name}' is not a valid environment variable name."
            ));
        }
        let value = value
            .as_str()
            .ok_or_else(|| format!("Variable '{name}' must have a quoted YAML string value."))?;
        if value.contains(['\n', '\r', '\0']) {
            return Err(format!(
                "Variable '{name}' contains a line break or NUL character."
            ));
        }
        output.push_str(name);
        output.push_str("=\"");
        for character in value.chars() {
            match character {
                '\\' | '"' | '$' | '`' => output.push('\\'),
                _ => {}
            }
            output.push(character);
        }
        output.push_str("\"\n");
    }
    Ok(output)
}

fn valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid path: {}", path.display()))?;
    let mut temporary = None;
    for attempt in 0..10 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("Cannot get current time: {error}"))?
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
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Cannot create file in '{}': {error}",
                    parent.display()
                ))
            }
        }
    }

    let (temporary_path, mut file) = temporary
        .ok_or_else(|| format!("Cannot create a temporary file in '{}'.", parent.display()))?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.map_err(|error| format!("Cannot write '{}': {error}", path.display()))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!(
            "Cannot set 600 permissions on '{}': {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "keyman-test-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn load_then_inject() {
        let root = fixture();
        let home = root.join("home");
        let target = root.join("target");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&target).unwrap();
        let master = root.join("master.yml");
        fs::write(&master, "project-a:\n  DATABASE_URL: \"postgres://localhost/db\"\n  API_KEY: \"secret #1\"\nproject-b:\n  DEBUG: \"true\"\n").unwrap();

        load(&master, &home).unwrap();
        inject("project-a", &target, &home).unwrap();
        assert_eq!(
            fs::read_to_string(target.join(".env")).unwrap(),
            "DATABASE_URL=\"postgres://localhost/db\"\nAPI_KEY=\"secret #1\"\n"
        );
        inject("project-b", &target, &home).unwrap();
        assert_eq!(
            fs::read_to_string(target.join(".env")).unwrap(),
            "DEBUG=\"true\"\n"
        );
        assert_eq!(
            fs::read_to_string(home.join(".keyman_config")).unwrap(),
            format!("{}\n", fs::canonicalize(&master).unwrap().display())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&master).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(target.join(".env"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(home.join(".keyman_config"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn required_errors() {
        let root = fixture();
        let home = root.join("home");
        let target = root.join("target");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(load(&root.join("missing.yml"), &home).is_err());
        assert!(inject("project-a", &target, &home)
            .unwrap_err()
            .contains("Run 'keyman load"));
        let master = root.join("master.yml");
        fs::write(&master, "project-a:\n  TOKEN: \"ok\"\n").unwrap();
        load(&master, &home).unwrap();
        assert!(inject("missing", &target, &home)
            .unwrap_err()
            .contains("was not found"));
        assert!(inject("project-a", &root.join("missing"), &home)
            .unwrap_err()
            .contains("does not exist"));
        assert!(!target.join(".env").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_unsafe_or_invalid_values() {
        assert!(render_project("p:\n  BAD-NAME: ok\n", "p").is_err());
        assert!(render_project("p:\n  KEY: true\n", "p").is_err());
        assert!(render_project("p:\n  KEY: [one, two]\n", "p").is_err());
        assert!(render_project("p:\n  KEY: \"one\\ntwo\"\n", "p").is_err());
        assert!(render_project("p: []\n", "p").is_err());
        assert_eq!(
            render_project("p:\n  KEY: 'a\"b\\c$d`e'\n", "p").unwrap(),
            "KEY=\"a\\\"b\\\\c\\$d\\`e\"\n"
        );
    }
}
