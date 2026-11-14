mod cli;
mod completions;
mod master;
mod save;
mod ui;

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};

use cli::Parsed;
use master::{home_dir, read_master, Variables};
use save::Edit;
use ui::{accent, bold, dim, display_path, green, plural, red, yellow};

enum Outcome {
    Print(String),
    Exit(i32),
}

fn main() {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    match execute(&args) {
        Ok(Outcome::Print(message)) => {
            if !message.is_empty() {
                println!("{message}");
            }
        }
        Ok(Outcome::Exit(code)) => process::exit(code),
        Err(message) => {
            ui::error(&message);
            process::exit(1);
        }
    }
}

fn execute(args: &[OsString]) -> Result<Outcome, String> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(Outcome::Print(cli::help()));
    };
    let name = first.to_string_lossy();
    match name.as_ref() {
        "-h" | "--help" => return Ok(Outcome::Print(cli::help())),
        "-V" | "--version" => {
            return Ok(Outcome::Print(format!(
                "keyman {}",
                env!("CARGO_PKG_VERSION")
            )))
        }
        _ => {}
    }
    let spec = cli::find(&name)?;

    if spec.name == "run" {
        let Some((project, command)) = cli::parse_run(rest)? else {
            return Ok(Outcome::Print(cli::command_help(spec)));
        };
        let (_, variables) = project_for(project)?;
        return run(&variables, command).map(Outcome::Exit);
    }

    let (positional, flags) = match cli::parse(spec, rest)? {
        Parsed::Help => return Ok(Outcome::Print(cli::command_help(spec))),
        Parsed::Args { positional, flags } => (positional, flags),
    };
    let arg = |index: usize| positional.get(index).copied();
    let output = match spec.name {
        "help" => match arg(0) {
            None => cli::help(),
            Some(command) => cli::command_help(cli::find(&command.to_string_lossy())?),
        },
        "init" => loaded_message(&master::init(arg(0).map(Path::new), &home_dir()?)?, true),
        "load" => loaded_message(
            &master::load(Path::new(positional[0]), &home_dir()?)?,
            false,
        ),
        "edit" => edit(&home_dir()?)?,
        "status" => status(&home_dir()?)?,
        "list" => list(&home_dir()?)?,
        "show" => {
            let (project, variables) = project_for(arg(0))?;
            show(&project, &variables, flags.contains(&"--reveal"))
        }
        "inject" => {
            let (project, variables) = project_for(arg(0))?;
            let directory = arg(1).map_or_else(|| PathBuf::from("."), PathBuf::from);
            let injected = inject(&variables, &directory)?;
            if env_file_unignored(&injected.path) {
                ui::warning(".env is not ignored by Git in this repository. Add '.env' to .gitignore so you do not commit secrets.");
            }
            injected.message(&project, variables.len())
        }
        "save" => save_command(arg(0), arg(1), &flags)?,
        "diff" => {
            let (project, variables) = project_for(arg(0))?;
            let (env_path, incoming) = read_env_file(arg(1))?;
            diff(
                &project,
                &variables,
                &env_path,
                &incoming,
                flags.contains(&"--reveal"),
            )
        }
        "export" => {
            let (_, variables) = project_for(arg(0))?;
            master::format_variables(&variables, "export ")
                .trim_end()
                .to_owned()
        }
        "completions" => completions::script(&positional[0].to_string_lossy())?,
        other => return Err(format!("command '{other}' is not implemented")),
    };
    Ok(Outcome::Print(output))
}

fn loaded_message(loaded: &master::Loaded, created: bool) -> String {
    for (project, problem) in &loaded.problems {
        ui::warning(&format!("project '{project}': {problem}"));
    }
    let verb = if created {
        "Created and loaded"
    } else {
        "Loaded"
    };
    let mut message = ui::success(&format!(
        "{verb} {}{}",
        bold(&display_path(&loaded.path)),
        dim(&format!(" · {}", plural(loaded.projects, "project")))
    ));
    if created {
        message.push_str(&format!(
            "\n\n{}\n  {}  Add your projects and secrets\n  {}  Write .env in a project folder\n  {}  Save a folder's .env back to the master file",
            ui::heading("Next steps"),
            accent("keyman edit            "),
            accent("keyman inject <project>"),
            accent("keyman save <project>  "),
        ));
    }
    message
}

/// Reads the master file and returns the variables of the named project, or
/// of the project named after the current folder when no name is given.
fn project_for(given: Option<&OsString>) -> Result<(String, Variables), String> {
    let (_, yaml) = read_master(&home_dir()?)?;
    let project = resolve_project(given, &yaml)?;
    let variables = master::project_variables(&yaml, &project)?;
    Ok((project, variables))
}

fn resolve_project(given: Option<&OsString>, yaml: &str) -> Result<String, String> {
    let names = master::project_names(yaml)?;
    let (project, from_folder) = match given {
        Some(name) => (name.to_string_lossy().into_owned(), false),
        None => (current_folder_name()?, true),
    };
    if names.contains(&project) {
        if from_folder {
            ui::note(&format!(
                "using project '{project}' (the current folder name)"
            ));
        }
        return Ok(project);
    }
    let hint = ui::did_you_mean(&project, names.iter().map(String::as_str));
    if from_folder {
        Err(format!(
            "no project given, and no project is named after the current folder '{project}'{hint}\n\nName a project, for example 'keyman inject my-app'. Run 'keyman list' to see projects."
        ))
    } else {
        Err(format!(
            "project '{project}' was not found in the master file{hint}\n\nRun 'keyman list' to see projects."
        ))
    }
}

fn current_folder_name() -> Result<String, String> {
    let directory = env::current_dir()
        .map_err(|error| format!("cannot read the current directory: {error}"))?;
    directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "no project given, and the current directory has no name".to_owned())
}

fn status(home: &Path) -> Result<String, String> {
    let (master, yaml) = read_master(home)?;
    let names = master::project_names(&yaml)?;
    let mut lines = vec![
        format!(
            "{}{}",
            bold(&display_path(&master)),
            dim(&format!(" · {}", plural(names.len(), "project")))
        ),
        String::new(),
    ];
    if names.is_empty() {
        lines.push(dim("No projects yet. Run 'keyman edit' to add one."));
        return Ok(lines.join("\n"));
    }
    let current = current_folder_name().ok();
    let width = names.iter().map(String::len).max().unwrap_or(0);
    let mut problems = 0;
    for name in &names {
        let (mark, detail) = match master::project_variables(&yaml, name) {
            Ok(variables) => (green("✓"), dim(&plural(variables.len(), "variable"))),
            Err(problem) => {
                problems += 1;
                (red("✗"), red(&problem))
            }
        };
        let marker = if current.as_ref() == Some(name) {
            format!("  {}", yellow("← current folder"))
        } else {
            String::new()
        };
        lines.push(format!(
            "  {mark} {}  {detail}{marker}",
            accent(&ui::pad(name, width))
        ));
    }
    if problems > 0 {
        lines.push(String::new());
        lines.push(red(&format!(
            "{} {} a problem. Run 'keyman edit' to fix it.",
            plural(problems, "project"),
            if problems == 1 { "has" } else { "have" }
        )));
    }
    Ok(lines.join("\n"))
}

fn list(home: &Path) -> Result<String, String> {
    let (_, yaml) = read_master(home)?;
    let names = master::project_names(&yaml)?;
    if names.is_empty() {
        ui::note("the master file has no projects yet. Run 'keyman edit' to add one.");
    }
    Ok(names.join("\n"))
}

fn show(project: &str, variables: &[(String, String)], reveal: bool) -> String {
    let mut lines = vec![
        format!(
            "{}{}",
            accent(&bold(project)),
            dim(&format!(" · {}", plural(variables.len(), "variable")))
        ),
        String::new(),
    ];
    if variables.is_empty() {
        lines.push(dim("  No variables yet."));
        return lines.join("\n");
    }
    let width = variables
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    for (name, value) in variables {
        let value = if reveal {
            value.clone()
        } else {
            dim("••••••••")
        };
        lines.push(format!("  {}  {value}", bold(&ui::pad(name, width))));
    }
    if !reveal {
        lines.push(String::new());
        lines.push(dim("Values are hidden. Add --reveal to show them."));
    }
    lines.join("\n")
}

/// Reads a folder's .env and merges it into a project in the master file.
fn save_command(
    given: Option<&OsString>,
    directory: Option<&OsString>,
    flags: &[&str],
) -> Result<String, String> {
    let (master_path, yaml) = read_master(&home_dir()?)?;
    let names = master::project_names(&yaml)?;
    let (project, from_folder) = match given {
        Some(name) => (name.to_string_lossy().into_owned(), false),
        None => (current_folder_name()?, true),
    };
    let exists = names.contains(&project);
    if !exists && !flags.contains(&"--new") {
        if let Some(close) = ui::suggest(&project, names.iter().map(String::as_str)) {
            return Err(format!(
                "project '{project}' does not exist, but '{close}' does\n\nTo save into '{close}', run 'keyman save {close}'.\nTo create '{project}' as a new project, run 'keyman save {project} --new'."
            ));
        }
    }
    if from_folder {
        ui::note(&format!(
            "using project '{project}' (the current folder name)"
        ));
    }

    let (env_path, incoming) = read_env_file(directory)?;
    if incoming.is_empty() {
        return Err(format!(
            "'{}' has no variables, so there is nothing to save",
            display_path(&env_path)
        ));
    }
    let existing = if exists {
        master::project_variables(&yaml, &project).map_err(|error| {
            format!("{error}\n\nFix the project with 'keyman edit' before saving into it.")
        })?
    } else {
        Vec::new()
    };
    let plan = save::plan(&existing, &incoming, flags.contains(&"--replace"));
    let env_shown = display_path(&fs::canonicalize(&env_path).unwrap_or(env_path));
    let master_shown = display_path(&master_path);

    if exists && plan.changes() == 0 {
        return Ok(ui::success(&format!(
            "{} is already up to date with {env_shown}",
            accent(&project)
        )));
    }

    let mut lines = vec![
        format!(
            "{} {} {} {}{}",
            bold("Saving"),
            env_shown,
            dim("→"),
            accent(&bold(&project)),
            dim(if exists { "" } else { " (new project)" })
        ),
        String::new(),
    ];
    let width = plan
        .edits
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    for (name, edit) in &plan.edits {
        let name = ui::pad(name, width);
        let row = match edit {
            Edit::Added => format!("{} {name}  {}", green("+"), dim("added")),
            Edit::Changed => format!("{} {name}  {}", yellow("~"), dim("changed")),
            Edit::Removed => format!("{} {name}  {}", red("-"), dim("removed")),
            Edit::Kept => dim(&format!(
                "= {name}  only in the master file, kept (--replace removes it)"
            )),
            Edit::Unchanged => continue,
        };
        lines.push(format!("  {row}"));
    }
    let unchanged = plan.count(Edit::Unchanged);
    if unchanged > 0 {
        lines.push(dim(&format!(
            "  {} unchanged",
            plural(unchanged, "variable")
        )));
    }
    lines.push(String::new());

    if flags.contains(&"--dry-run") {
        lines.push(dim(
            "Dry run: nothing was saved. Run again without --dry-run to save.",
        ));
        return Ok(lines.join("\n"));
    }
    let updated = save::update_project(&yaml, &project, &plan, exists)?;
    master::atomic_write(&master_path, updated.as_bytes())?;
    lines.push(ui::success(&format!(
        "Saved {} to {}",
        plural(plan.changes(), "change"),
        bold(&master_shown)
    )));
    Ok(lines.join("\n"))
}

/// Reads DIRECTORY/.env (default: the current directory).
fn read_env_file(directory: Option<&OsString>) -> Result<(PathBuf, Variables), String> {
    let env_path = directory
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".env");
    let text = fs::read_to_string(&env_path)
        .map_err(|error| format!("cannot read '{}': {error}", display_path(&env_path)))?;
    let variables = save::parse_dotenv(&text)?;
    Ok((env_path, variables))
}

/// A git-style diff from the master file (red -) to .env (green +).
fn diff(
    project: &str,
    master: &[(String, String)],
    env_path: &Path,
    env: &[(String, String)],
    reveal: bool,
) -> String {
    let env_shown = display_path(&fs::canonicalize(env_path).unwrap_or_else(|_| env_path.into()));
    let plan = save::plan(master, env, true);
    let line = |name: &str, value: &str| {
        if reveal {
            master::format_variables(&[(name.to_owned(), value.to_owned())], "")
                .trim_end()
                .to_owned()
        } else {
            format!("{name}=••••••••")
        }
    };
    let value_in = |list: &[(String, String)], name: &str| {
        list.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };

    let mut lines = vec![
        format!(
            "{} {} {} {}",
            bold("diff"),
            accent(&bold(project)),
            dim("→"),
            bold(&env_shown)
        ),
        String::new(),
    ];
    if plan.changes() == 0 {
        lines.push(dim(&format!(
            "  {} the same in both",
            plural(plan.count(Edit::Unchanged), "variable")
        )));
        lines.push(String::new());
        lines.push(ui::success(&format!(
            "{} matches the master file",
            bold(".env")
        )));
        return lines.join("\n");
    }
    for (name, edit) in &plan.edits {
        let old = value_in(master, name);
        let new = value_in(env, name);
        match edit {
            Edit::Unchanged | Edit::Kept => lines.push(dim(&format!("  {}", line(name, &old)))),
            Edit::Changed => {
                lines.push(red(&format!("- {}", line(name, &old))));
                let note = if reveal { "" } else { "  (value changed)" };
                lines.push(format!(
                    "{}{}",
                    green(&format!("+ {}", line(name, &new))),
                    dim(note)
                ));
            }
            Edit::Removed => lines.push(red(&format!("- {}", line(name, &old)))),
            Edit::Added => lines.push(green(&format!("+ {}", line(name, &new)))),
        }
    }

    let summary: Vec<String> = [
        (plan.count(Edit::Changed), "changed"),
        (plan.count(Edit::Added), "only in .env"),
        (plan.count(Edit::Removed), "only in the master file"),
    ]
    .iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect();
    lines.push(String::new());
    lines.push(dim(&summary.join(" · ")));
    lines.push(dim(&ui::highlight(
        ui::Stream::Out,
        "Run 'keyman save' to update the master file, or 'keyman inject' to overwrite .env.",
    )));
    lines.join("\n")
}

fn edit(home: &Path) -> Result<String, String> {
    let (master, _) = read_master(home)?;
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| if cfg!(windows) { "notepad" } else { "vi" }.to_owned());
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| "the editor command is empty".to_owned())?;
    let status = Command::new(program)
        .args(parts)
        .arg(&master)
        .status()
        .map_err(|error| {
            format!("cannot start the editor '{editor}': {error}\n\nSet EDITOR to your editor, for example EDITOR=\"code --wait\".")
        })?;
    if !status.success() {
        return Err(format!("the editor '{editor}' exited with {status}"));
    }

    let (_, yaml) = read_master(home)?;
    let fix = "\n\nRun 'keyman edit' to fix it.";
    let (projects, problems) = master::validate(&yaml).map_err(|error| format!("{error}{fix}"))?;
    if !problems.is_empty() {
        let list: Vec<String> = problems
            .iter()
            .map(|(project, problem)| format!("  {project}: {problem}"))
            .collect();
        return Err(format!(
            "the master file has problems:\n{}{fix}",
            list.join("\n")
        ));
    }
    Ok(ui::success(&format!(
        "{} is valid{}",
        bold(&display_path(&master)),
        dim(&format!(" · {}", plural(projects, "project")))
    )))
}

#[derive(Debug, PartialEq)]
enum Change {
    Created,
    Replaced,
    Unchanged,
}

#[derive(Debug)]
struct Injected {
    path: PathBuf,
    change: Change,
}

impl Injected {
    fn message(&self, project: &str, count: usize) -> String {
        let path = bold(&display_path(&self.path));
        let source = dim(&format!(" · {} from ", plural(count, "variable"))) + &accent(project);
        ui::success(&match self.change {
            Change::Created => format!("Created {path}{source}"),
            Change::Replaced => format!("Updated {path}{source}"),
            Change::Unchanged => format!("{path} is already up to date{source}"),
        })
    }
}

fn inject(variables: &[(String, String)], target_directory: &Path) -> Result<Injected, String> {
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
    Ok(Injected { path, change })
}

/// Runs a command with the project's variables added to its environment.
/// Returns the command's exit code.
fn run(variables: &[(String, String)], command: &[OsString]) -> Result<i32, String> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| "missing the command to run".to_owned())?;
    let status = Command::new(program)
        .args(args)
        .envs(variables.iter().map(|(name, value)| (name, value)))
        .status()
        .map_err(|error| {
            let mut message = format!("cannot run '{}': {error}", program.to_string_lossy());
            if cfg!(windows) {
                message.push_str("\n\nFor .cmd and .bat scripts such as npm, use: keyman run <project> -- cmd /c <command>");
            }
            message
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return Ok(128 + signal);
        }
    }
    Ok(status.code().unwrap_or(1))
}

/// True only when Git reports that the .env file is inside a repository and
/// not ignored. Missing Git or a non-repository directory gives false.
fn env_file_unignored(env_file: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture {
        root: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new(master: &str) -> Self {
            let root = env::temp_dir().join(format!(
                "keyman-test-{}-{}",
                process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let home = root.join("home");
            fs::create_dir_all(&home).unwrap();
            fs::write(root.join("master.yml"), master).unwrap();
            Fixture { root, home }
        }

        fn load(&self) {
            master::load(&self.root.join("master.yml"), &self.home).unwrap();
        }

        fn variables(&self, project: &str) -> Variables {
            let (_, yaml) = read_master(&self.home).unwrap();
            let project = resolve_project(Some(&OsString::from(project)), &yaml).unwrap();
            master::project_variables(&yaml, &project).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn load_then_inject() {
        let fixture = Fixture::new("project-a:\n  DATABASE_URL: \"postgres://localhost/db\"\n  API_KEY: \"secret #1\"\nproject-b:\n  DEBUG: \"true\"\n");
        let target = fixture.root.join("target");
        fs::create_dir(&target).unwrap();
        fixture.load();

        let injected = inject(&fixture.variables("project-a"), &target).unwrap();
        assert_eq!(injected.change, Change::Created);
        assert_eq!(
            fs::read_to_string(target.join(".env")).unwrap(),
            "DATABASE_URL=\"postgres://localhost/db\"\nAPI_KEY=\"secret #1\"\n"
        );
        let injected = inject(&fixture.variables("project-b"), &target).unwrap();
        assert_eq!(injected.change, Change::Replaced);
        assert_eq!(
            fs::read_to_string(target.join(".env")).unwrap(),
            "DEBUG=\"true\"\n"
        );
        let injected = inject(&fixture.variables("project-b"), &target).unwrap();
        assert_eq!(injected.change, Change::Unchanged);
        assert_eq!(
            fs::read_to_string(fixture.home.join(".keyman_config")).unwrap(),
            format!(
                "{}\n",
                fs::canonicalize(fixture.root.join("master.yml"))
                    .unwrap()
                    .display()
            )
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(fixture.root.join("master.yml")), 0o600);
            assert_eq!(mode(target.join(".env")), 0o600);
            assert_eq!(mode(fixture.home.join(".keyman_config")), 0o600);
        }
    }

    #[test]
    fn required_errors() {
        let fixture = Fixture::new("project-a:\n  TOKEN: \"ok\"\n");
        let target = fixture.root.join("target");
        fs::create_dir(&target).unwrap();
        assert!(master::load(&fixture.root.join("missing.yml"), &fixture.home).is_err());
        assert!(
            master::load(&fixture.root.join("master.txt"), &fixture.home)
                .unwrap_err()
                .contains(".yml or .yaml")
        );
        assert!(read_master(&fixture.home)
            .unwrap_err()
            .contains("keyman load"));

        fixture.load();
        let (_, yaml) = read_master(&fixture.home).unwrap();
        let missing = resolve_project(Some(&OsString::from("project-b")), &yaml).unwrap_err();
        assert!(missing.contains("was not found"));
        assert!(missing.contains("Did you mean 'project-a'"));
        assert!(inject(
            &fixture.variables("project-a"),
            &fixture.root.join("missing")
        )
        .unwrap_err()
        .contains("does not exist"));
        assert!(!target.join(".env").exists());
    }

    #[test]
    fn load_rejects_invalid_yaml_and_warns_about_bad_projects() {
        let fixture = Fixture::new("good:\n  A: \"1\"\nbad:\n  B: 2\n");
        let loaded = master::load(&fixture.root.join("master.yml"), &fixture.home).unwrap();
        assert_eq!(loaded.projects, 2);
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(loaded.problems[0].0, "bad");

        fs::write(fixture.root.join("broken.yaml"), "a: [unclosed\n").unwrap();
        assert!(
            master::load(&fixture.root.join("broken.yaml"), &fixture.home)
                .unwrap_err()
                .contains("invalid master YAML")
        );
    }

    #[test]
    fn rejects_unsafe_or_invalid_values() {
        use master::render_project;
        assert!(render_project("p:\n  BAD-NAME: ok\n", "p").is_err());
        assert!(render_project("p:\n  KEY: true\n", "p").is_err());
        assert!(render_project("p:\n  KEY: [one, two]\n", "p").is_err());
        assert!(render_project("p:\n  KEY: \"one\\ntwo\"\n", "p").is_err());
        assert!(render_project("p: []\n", "p").is_err());
        assert_eq!(render_project("p:\n", "p").unwrap(), "");
        assert_eq!(
            render_project("p:\n  KEY: 'a\"b\\c$d`e'\n", "p").unwrap(),
            "KEY=\"a\\\"b\\\\c\\$d\\`e\"\n"
        );
    }

    #[test]
    fn lists_shows_and_exports_projects() {
        let fixture = Fixture::new("api:\n  TOKEN: \"a$b\"\n  DEBUG: \"1\"\nweb: {}\n");
        fixture.load();

        assert_eq!(list(&fixture.home).unwrap(), "api\nweb");
        let status = status(&fixture.home).unwrap();
        assert!(status.contains(" · 2 projects"));
        assert!(status.contains("  ✓ api  2 variables"));
        assert!(status.contains("  ✓ web  0 variables"));
        let variables = fixture.variables("api");
        assert_eq!(
            show("api", &variables, false),
            "api · 2 variables\n\n  TOKEN  ••••••••\n  DEBUG  ••••••••\n\nValues are hidden. Add --reveal to show them."
        );
        assert_eq!(
            show("api", &variables, true),
            "api · 2 variables\n\n  TOKEN  a$b\n  DEBUG  1"
        );
        assert_eq!(
            master::format_variables(&variables, "export "),
            "export TOKEN=\"a\\$b\"\nexport DEBUG=\"1\"\n"
        );
    }

    #[test]
    fn diffs_like_git() {
        let master = vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "old".to_owned()),
            ("C".to_owned(), "gone".to_owned()),
        ];
        let env_file = vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "new".to_owned()),
            ("D".to_owned(), "added".to_owned()),
        ];
        let path = Path::new("project/.env");
        let revealed = diff("api", &master, path, &env_file, true);
        let body: Vec<&str> = revealed.lines().skip(2).collect();
        assert_eq!(
            body,
            [
                "  A=\"1\"",
                "- B=\"old\"",
                "+ B=\"new\"",
                "- C=\"gone\"",
                "+ D=\"added\"",
                "",
                "1 changed · 1 only in .env · 1 only in the master file",
                "Run 'keyman save' to update the master file, or 'keyman inject' to overwrite .env.",
            ]
        );
        let masked = diff("api", &master, path, &env_file, false);
        assert!(masked.contains("+ B=••••••••  (value changed)"));
        assert!(!masked.contains("new"));
        assert!(
            diff("api", &master, path, &master, false).ends_with("✓ .env matches the master file")
        );
    }

    #[test]
    fn init_creates_a_valid_file_once() {
        let fixture = Fixture::new("");
        let loaded = master::init(None, &fixture.home).unwrap();
        assert_eq!(
            loaded.path,
            fs::canonicalize(fixture.home.join(".keyman.yml")).unwrap()
        );
        assert_eq!(loaded.projects, 1);
        assert!(loaded.problems.is_empty());
        assert_eq!(fixture.variables("my-app").len(), 3);
        assert!(master::init(None, &fixture.home)
            .unwrap_err()
            .contains("already exists"));
    }

    #[test]
    fn completion_scripts_cover_every_shell() {
        for shell in cli::SHELLS {
            let script = completions::script(shell).unwrap();
            assert!(script.contains("inject"), "{shell}");
            assert!(script.contains("keyman list"), "{shell}");
        }
        assert!(completions::script("bsh")
            .unwrap_err()
            .contains("Did you mean 'bash'"));
    }

    #[cfg(unix)]
    #[test]
    fn run_passes_variables_and_exit_code() {
        let fixture = Fixture::new("api:\n  TOKEN: \"a$b\"\n");
        fixture.load();
        let variables = fixture.variables("api");
        let shell = |script: &str| ["sh", "-c", script].map(OsString::from);
        assert_eq!(
            run(&variables, &shell("test \"$TOKEN\" = 'a$b'")).unwrap(),
            0
        );
        assert_eq!(run(&variables, &shell("exit 7")).unwrap(), 7);
        assert!(run(&variables, &[]).is_err());
        assert!(run(&variables, &[OsString::from("keyman-no-such-program")]).is_err());
    }
}
