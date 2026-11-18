mod cli;
mod completions;

use keyman_cli::cloud::account::Unlocked;
use keyman_cli::cloud::account::{self, Prompt};
use keyman_cli::cloud::groups::{self, Removal};
use keyman_cli::cloud::keystore::SystemStore;
use keyman_cli::cloud::merge::{choose_all, Conflict, Keep};
use keyman_cli::cloud::state;
use keyman_cli::cloud::sync::{self, LinkStatus, PullReport, PullResult, PushResult, Resolver};
use keyman_cli::envfile::{self, Change};
use keyman_cli::{master, save, ui};

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

use cli::Parsed;
use master::{home_dir, read_master, Variables};
use save::Edit;
use ui::{accent, bold, dim, display_path, green, plural, red, safe_text, yellow};

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
            let (_, yaml) = read_master(&home_dir()?)?;
            let target = resolve_target(arg(0), &yaml)?;
            let project = master::read_project(&yaml, &target.project)?;
            let reveal = cli::has(&flags, "--reveal");
            if target.stage.is_none() && project.has_stages() {
                show_stages(&target.project, &project, reveal)
            } else {
                let variables =
                    master::stage_variables(&project, &target.project, target.stage.as_deref())?;
                show(&target.label(), &variables, reveal)
            }
        }
        "inject" => {
            let (project, variables) = project_for(arg(0))?;
            let directory = arg(1).map_or_else(|| PathBuf::from("."), PathBuf::from);
            let injected = inject(&variables, &directory)?;
            if envfile::unignored(&injected.path) {
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
                cli::has(&flags, "--reveal"),
            )
        }
        "export" => {
            let (_, variables) = project_for(arg(0))?;
            master::format_variables(&variables, "export ")
                .trim_end()
                .to_owned()
        }
        "completions" => completions::script(&positional[0].to_string_lossy())?,
        "login" => login(&home_dir()?, &flags)?,
        "logout" => {
            account::logout(&home_dir()?, &SystemStore::new())?;
            ui::success("Signed out. Keyman removed the keys from this computer.")
        }
        "recover" => {
            account::recover(
                &home_dir()?,
                &SystemStore::new(),
                &mut || choose_provider(&flags),
                &mut TerminalPrompt,
            )?;
            ui::success("Keyman changed the vault password.")
        }
        "password" => {
            account::change_password(&home_dir()?, &SystemStore::new(), &mut TerminalPrompt)?;
            ui::success("Keyman changed the vault password.")
        }
        "link" => {
            let home = home_dir()?;
            let (_, yaml) = read_master(&home)?;
            whole_project(arg(0), &yaml)?;
            let project = resolve_project(arg(0), &yaml)?;
            let group = cli::value(&flags, "--group").map(|name| name.to_string_lossy());
            sync::ensure_unlinked(&home, &project)?;
            let unlocked = unlock(&home)?;
            sync::link(&home, &unlocked, &project, group.as_deref())?;
            ui::success(&format!("Linked {project} to Keyman Cloud."))
        }
        "unlink" => {
            if let Ok((_, yaml)) = read_master(&home_dir()?) {
                whole_project(arg(0), &yaml)?;
            }
            let project = match arg(0) {
                Some(name) => name.to_string_lossy().into_owned(),
                None => current_folder_name()?,
            };
            sync::unlink(&home_dir()?, &project)?;
            ui::success(&format!("Removed the link for {project}."))
        }
        "push" | "pull" => {
            if let Ok((_, yaml)) = read_master(&home_dir()?) {
                whole_project(arg(0), &yaml)?;
            }
            return if spec.name == "push" {
                push_command(arg(0))
            } else {
                pull_command(arg(0), &flags)
            };
        }
        "cloud" => return cloud_command(&positional[0].to_string_lossy(), arg(1)),
        "group" => return group_command(&positional),
        other => return Err(format!("command '{other}' is not implemented")),
    };
    Ok(Outcome::Print(output))
}

/// Unlocks the cloud account. This signs in and asks for the vault password
/// only when it needs to.
fn unlock(home: &Path) -> Result<Unlocked, String> {
    account::session(home, &SystemStore::new(), &mut TerminalPrompt)
}

/// Prints the lines. A non-zero code means the command did not finish all work.
fn finish(lines: Vec<String>, code: i32) -> Outcome {
    if code == 0 {
        Outcome::Print(lines.join("\n"))
    } else {
        println!("{}", lines.join("\n"));
        Outcome::Exit(code)
    }
}

fn push_command(given: Option<&OsString>) -> Result<Outcome, String> {
    let home = home_dir()?;
    let project = given.map(|name| name.to_string_lossy().into_owned());
    let unlocked = unlock(&home)?;
    let results = sync::push(&home, &unlocked, project.as_deref())?;
    if results.is_empty() {
        return Ok(Outcome::Print(dim(
            "No linked projects. Run 'keyman link' to connect one.",
        )));
    }
    let mut code = 0;
    let lines = results
        .iter()
        .map(|result| match result {
            PushResult::Pushed { project, version } => ui::success(&format!(
                "Sent {} {}",
                accent(&safe_text(project)),
                dim(&format!("(version {version})"))
            )),
            PushResult::UpToDate(project) => {
                dim(&format!("  {} is up to date", safe_text(project)))
            }
            PushResult::CloudIsNewer(project) => {
                code = 1;
                red(&format!(
                    "The cloud has newer changes for {}. Run 'keyman pull' first.",
                    safe_text(project)
                ))
            }
            PushResult::Skipped { project, reason } => {
                code = 1;
                yellow(&format!(
                    "Skipped {}. {}",
                    safe_text(project),
                    safe_lines(reason)
                ))
            }
        })
        .collect();
    Ok(finish(lines, code))
}

fn pull_command(given: Option<&OsString>, flags: &cli::Flags) -> Result<Outcome, String> {
    let home = home_dir()?;
    let project = given.map(|name| name.to_string_lossy().into_owned());
    let local_name = cli::value(flags, "--as").map(|name| name.to_string_lossy().into_owned());
    if local_name.is_some() && project.is_none() {
        return Err("The option --as needs a project name.".to_string());
    }
    let mut resolver: Box<dyn Resolver> = match (
        cli::has(flags, "--keep-local"),
        cli::has(flags, "--keep-cloud"),
    ) {
        (true, true) => return Err("Choose one: --keep-local or --keep-cloud.".to_string()),
        (true, false) => Box::new(KeepResolver { local: true }),
        (false, true) => Box::new(KeepResolver { local: false }),
        (false, false) if std::io::stdin().is_terminal() => Box::new(TerminalResolver),
        (false, false) => Box::new(NoTerminalResolver),
    };
    let unlocked = unlock(&home)?;
    let report = sync::pull(
        &home,
        &unlocked,
        project.as_deref(),
        local_name.as_deref(),
        resolver.as_mut(),
    )?;
    let output = pull_output(&report);
    // The pull is done. A failure here must not hide its results.
    let waiting_line = match groups::waiting(&unlocked) {
        Ok(0) => None,
        Ok(waiting) => Some(yellow(&format!(
            "Waiting for access to {}. A member must run 'keyman pull'.",
            plural(waiting, "group")
        ))),
        Err(_) => Some(yellow(
            "Keyman cannot check for waiting invites now. Run 'keyman pull' again later.",
        )),
    };
    if report.results.is_empty() {
        let mut lines = vec![dim("No projects in the cloud yet.")];
        lines.extend(waiting_line);
        return Ok(Outcome::Print(lines.join("\n")));
    }
    let PullOutput { mut lines, code } = output;
    lines.extend(waiting_line);
    Ok(finish(lines, code))
}

/// What the pull command prints for a report: the result lines and the exit
/// code. The invite lines are printed earlier, by `Resolver::invites`.
struct PullOutput {
    lines: Vec<String>,
    code: i32,
}

/// Prints what Keyman did for invites: access given (stdout), warnings (stderr).
fn print_invites(given: &[(String, String)], warnings: &[String]) {
    for (group, email) in given {
        println!("Gave {email} access to {group}.");
    }
    for warning in warnings {
        eprintln!("{warning}");
    }
}

fn pull_output(report: &PullReport) -> PullOutput {
    let mut code = 0;
    let lines: Vec<String> = report
        .results
        .iter()
        .map(|result| match result {
            PullResult::Updated { project, changes } => ui::success(&format!(
                "Updated {} {}",
                accent(&safe_text(project)),
                dim(&format!("({})", plural(*changes, "change")))
            )),
            PullResult::Added(project) => {
                ui::success(&format!("Added {}", accent(&safe_text(project))))
            }
            PullResult::UpToDate(project) => {
                dim(&format!("  {} is up to date", safe_text(project)))
            }
            PullResult::Stopped { project, conflicts } => {
                code = 1;
                red(&format!(
                    "Pull stopped: {} in {}. Run 'keyman pull' in a terminal, or add --keep-local or --keep-cloud.",
                    plural(*conflicts, "conflict"),
                    safe_text(project)
                ))
            }
            PullResult::Skipped { project, reason } => {
                code = 1;
                yellow(&format!(
                    "Skipped {}. {}",
                    safe_text(project),
                    safe_lines(reason)
                ))
            }
            PullResult::NameTaken { project } => {
                code = 1;
                yellow(&format!(
                    "Skipped {}. {}",
                    safe_text(project),
                    safe_lines(&sync::name_taken_reason(project))
                ))
            }
        })
        .collect();
    PullOutput { lines, code }
}

fn group_command(positional: &[&OsString]) -> Result<Outcome, String> {
    let text = |index: usize| {
        positional
            .get(index)
            .map(|v| v.to_string_lossy().into_owned())
    };
    let action = text(0).unwrap_or_default();
    let (group, email) = (text(1), text(2));
    // Check the arguments first: a mistake must not ask for the password.
    let (usage, wanted) = match action.as_str() {
        "create" | "delete" => (format!("{action} <group>"), 1),
        "invite" | "remove" => (format!("{action} <group> <email>"), 2),
        "list" => ("list".to_string(), 0),
        other => {
            return Err(format!(
                "unknown action '{other}' for 'keyman group'{}

Use 'keyman group create', 'invite', 'remove', 'list' or 'delete'.",
                ui::did_you_mean(other, ["create", "invite", "remove", "list", "delete"])
            ))
        }
    };
    let given = positional.len() - 1;
    if given < wanted {
        return Err(format!(
            "The action '{action}' needs more arguments. Use 'keyman group {usage}'."
        ));
    }
    if given > wanted {
        return Err(format!(
            "The action '{action}' has too many arguments. Use 'keyman group {usage}'."
        ));
    }
    let home = home_dir()?;
    let unlocked = unlock(&home)?;
    let group = group.unwrap_or_default();
    let email = email.unwrap_or_default();
    match action.as_str() {
        "create" => {
            groups::create(&unlocked, &group)?;
            Ok(Outcome::Print(ui::success(&format!(
                "Made the group {group}. Run 'keyman group invite {group} <email>' to add a person."
            ))))
        }
        "invite" => {
            groups::invite(&unlocked, &group, &email)?;
            Ok(Outcome::Print(ui::success(&format!(
                "Invited {} to {group}. A member of the group must run 'keyman pull' to give access.",
                email.trim().to_lowercase()
            ))))
        }
        "remove" => {
            let vaults = sync::Vaults::load(&unlocked)?;
            let shown = email.trim().to_lowercase();
            if let Removal::Member { projects } =
                groups::check_remove(&unlocked, &vaults, &group, &email)?
            {
                let encrypted = if projects == 1 {
                    "the 1 project".to_string()
                } else {
                    format!("all {projects} projects")
                };
                eprintln!(
                    "Keyman makes a new group key and encrypts {encrypted} again. {shown} keeps the data that they got before."
                );
                eprintln!("Type the email address to confirm:");
                if read_answer()?.to_lowercase() != shown {
                    return Err(
                        "The email address is not the same. Keyman did not remove anyone."
                            .to_string(),
                    );
                }
            }
            Ok(Outcome::Print(
                match groups::remove(&unlocked, &vaults, &group, &email)? {
                    Removal::Member { .. } => ui::success(&format!(
                        "Removed {shown} from {group}. The group has a new key."
                    )),
                    Removal::Invite => {
                        ui::success(&format!("Keyman removed the invite for {shown}."))
                    }
                },
            ))
        }
        "list" => {
            let vaults = sync::Vaults::load(&unlocked)?;
            let found = groups::list(&unlocked, &vaults)?;
            let mut lines = vec![
                bold(&format!("Groups{}", dim(&format!(" · {}", found.len())))),
                String::new(),
            ];
            if found.is_empty() {
                lines.push(dim(
                    "No groups yet. Run 'keyman group create <group>' to make one.",
                ));
            }
            let width = found
                .iter()
                .map(|g| g.name.chars().count())
                .max()
                .unwrap_or(0);
            for item in &found {
                lines.push(format!(
                    "  {}  {}",
                    accent(&ui::pad(&item.name, width)),
                    dim(&item.detail())
                ));
            }
            Ok(Outcome::Print(lines.join("\n")))
        }
        "delete" => {
            let vaults = sync::Vaults::load(&unlocked)?;
            let vault_id = groups::check_delete(&unlocked, &vaults, &group)?;
            eprintln!(
                "Delete the group {group} and all its projects from the cloud for all members? Type the group name to confirm:"
            );
            if read_answer()? != group {
                return Err("The name is not the same. Keyman did not delete anything.".to_string());
            }
            groups::delete(&unlocked, &vaults, &group)?;
            let links = groups::forget_links(&home, &vault_id)?;
            let mut text =
                format!("Deleted the group {group}. The projects stay in the master file.");
            if links > 0 {
                text.push_str(&format!(" Keyman removed {}.", plural(links, "link")));
            }
            Ok(Outcome::Print(ui::success(&text)))
        }
        _ => unreachable!("the action was checked above"),
    }
}

fn cloud_command(action: &str, given: Option<&OsString>) -> Result<Outcome, String> {
    let home = home_dir()?;
    match action {
        "status" => {
            let links = sync::local_status(&home)?;
            if links.is_empty() {
                return Ok(Outcome::Print(dim(
                    "No linked projects. Run 'keyman link' to connect one.",
                )));
            }
            let width = links.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
            let mut lines: Vec<String> = links
                .iter()
                .map(|(name, status)| {
                    let (mark, text) = match status {
                        LinkStatus::Synced => (green("✓"), dim("synced")),
                        LinkStatus::LocalChanges => (yellow("●"), "local changes".to_string()),
                        LinkStatus::Missing => (red("✗"), red("not in master file")),
                    };
                    format!("  {mark} {}  {text}", accent(&ui::pad(name, width)))
                })
                .collect();
            lines.push(String::new());
            lines.push(dim("Run 'keyman pull' to check the cloud for new changes."));
            Ok(Outcome::Print(lines.join("\n")))
        }
        "delete" => {
            let project = match given {
                Some(name) => name.to_string_lossy().into_owned(),
                None => current_folder_name()?,
            };
            if state::load(&home)?.link(&project).is_none() {
                return Err(format!(
                    "Project '{project}' is not linked. Run 'keyman cloud status' to see the links."
                ));
            }
            eprintln!(
                "Delete {} from the cloud for all members? Type the project name to confirm:",
                safe_text(&project)
            );
            if read_answer()? != project {
                return Err(
                    "The name is not the same. Keyman did not delete anything.".to_string()
                );
            }
            let unlocked = unlock(&home)?;
            sync::delete_cloud(&home, &unlocked, &project)?;
            Ok(Outcome::Print(ui::success(&format!(
                "Deleted {project} from the cloud. The project stays in the master file."
            ))))
        }
        other => Err(format!(
            "unknown action '{other}' for 'keyman cloud'{}\n\nUse 'keyman cloud status' or 'keyman cloud delete <project>'.",
            ui::did_you_mean(other, ["status", "delete"])
        )),
    }
}

/// On a conflict, takes the same side for every variable.
struct KeepResolver {
    local: bool,
}

impl Resolver for KeepResolver {
    fn resolve(
        &mut self,
        _project: &str,
        conflicts: &[Conflict],
    ) -> Result<Option<Vec<Option<String>>>, String> {
        let keep = if self.local { Keep::Local } else { Keep::Cloud };
        Ok(Some(choose_all(conflicts, keep)))
    }

    fn invites(&mut self, given: &[(String, String)], warnings: &[String]) {
        print_invites(given, warnings);
    }
}

/// With no terminal and no option, a conflict stops the pull.
struct NoTerminalResolver;

impl Resolver for NoTerminalResolver {
    fn resolve(
        &mut self,
        _project: &str,
        _conflicts: &[Conflict],
    ) -> Result<Option<Vec<Option<String>>>, String> {
        Ok(None)
    }

    fn invites(&mut self, given: &[(String, String)], warnings: &[String]) {
        print_invites(given, warnings);
    }
}

/// Each line of a message, with hidden characters replaced. The message can
/// hold a project name from the cloud.
fn safe_lines(text: &str) -> String {
    text.lines().map(safe_text).collect::<Vec<_>>().join("\n")
}

/// Asks about each conflict in the terminal. Values stay hidden until the
/// user chooses to show them.
struct TerminalResolver;

fn read_answer() -> Result<String, String> {
    let mut answer = String::new();
    let read = std::io::stdin()
        .read_line(&mut answer)
        .map_err(|_| "Keyman cannot read your answer.".to_string())?;
    if read == 0 {
        return Err("Keyman cannot read your answer.".to_string());
    }
    Ok(answer.trim().to_string())
}

fn hidden_value(value: &Option<String>) -> String {
    match value {
        Some(_) => "••••••••".to_string(),
        None => "(removed)".to_string(),
    }
}

fn clear_value(value: &Option<String>) -> String {
    match value {
        Some(text) => text.clone(),
        None => "(removed)".to_string(),
    }
}

/// Reads a hidden value. The prompt goes to stderr, like all resolver text.
fn ask_hidden_stderr(label: &str) -> Result<String, String> {
    use std::io::Write;
    eprint!("{label}");
    let _ = std::io::stderr().flush();
    let value =
        rpassword::read_password().map_err(|_| "Keyman cannot read your answer.".to_string())?;
    eprintln!();
    Ok(value)
}

fn ask_new_value() -> Result<Option<String>, String> {
    let first = ask_hidden_stderr("New value: ")?;
    let second = ask_hidden_stderr("Type the value again: ")?;
    if first == second {
        Ok(Some(first))
    } else {
        eprintln!("The two values are not the same.");
        Ok(None)
    }
}

impl Resolver for TerminalResolver {
    fn resolve(
        &mut self,
        project: &str,
        conflicts: &[Conflict],
    ) -> Result<Option<Vec<Option<String>>>, String> {
        let mut choices = Vec::new();
        for conflict in conflicts {
            eprintln!(
                "Conflict in {}: {}",
                safe_text(project),
                safe_text(&conflict.name)
            );
            eprintln!("  1) This computer: {}", hidden_value(&conflict.local));
            eprintln!("  2) Cloud:         {}", hidden_value(&conflict.cloud));
            eprintln!("  3) Type a new value");
            eprintln!("  4) Show the values");
            let choice = loop {
                eprintln!("Select 1, 2, 3 or 4:");
                match read_answer()?.as_str() {
                    "1" => break conflict.local.clone(),
                    "2" => break conflict.cloud.clone(),
                    "3" => {
                        if let Some(value) = ask_new_value()? {
                            break Some(value);
                        }
                    }
                    "4" if std::io::stderr().is_terminal() => {
                        eprintln!("  This computer: {}", clear_value(&conflict.local));
                        eprintln!("  Cloud:         {}", clear_value(&conflict.cloud));
                    }
                    "4" => eprintln!("Keyman cannot show the values here."),
                    _ => eprintln!("Type 1, 2, 3 or 4."),
                }
            };
            choices.push(choice);
        }
        Ok(Some(choices))
    }

    fn invites(&mut self, given: &[(String, String)], warnings: &[String]) {
        print_invites(given, warnings);
    }
}

/// Asks the questions in the terminal. Passwords are not shown.
struct TerminalPrompt;

fn ask_hidden(label: &str) -> Result<String, String> {
    rpassword::prompt_password(label).map_err(|_| "Keyman cannot read your answer.".to_string())
}

impl Prompt for TerminalPrompt {
    fn new_password(&mut self) -> Result<String, String> {
        eprintln!("Make a vault password. Keyman uses it to encrypt your secrets.");
        eprintln!("Keyman cannot reset this password.");
        let first = ask_hidden("Vault password: ")?;
        if first.chars().count() < 10 {
            return Err("The password must have 10 characters or more.".to_string());
        }
        let second = ask_hidden("Type the password again: ")?;
        if first != second {
            return Err("The two passwords are not the same.".to_string());
        }
        Ok(first)
    }

    fn password(&mut self) -> Result<String, String> {
        ask_hidden("Vault password: ")
    }

    fn recovery_code(&mut self) -> Result<String, String> {
        ask_hidden("Recovery code: ")
    }

    fn show_recovery_code(&mut self, code: &str) -> Result<(), String> {
        eprintln!(
            "
Your recovery code:

    {}
",
            bold(code)
        );
        eprintln!("Write down this code and keep it in a safe place.");
        eprintln!("If you lose the password and this code, nobody can get your cloud data back.");
        eprintln!("Type the code again to confirm:");
        Ok(())
    }

    fn notice(&mut self, text: &str) {
        ui::warning(text);
    }

    fn sign_in_link(&mut self, url: &str) {
        println!("Open this link to sign in:\n{url}");
    }
}

/// The provider from the options, or from a question when there is no option.
fn choose_provider(flags: &cli::Flags) -> Result<&'static str, String> {
    match (cli::has(flags, "--github"), cli::has(flags, "--google")) {
        (true, true) => Err("Choose one: --github or --google.".to_string()),
        (true, false) => Ok("github"),
        (false, true) => Ok("google"),
        (false, false) => {
            eprintln!("Sign in with: 1) GitHub  2) Google");
            let mut answer = String::new();
            std::io::stdin()
                .read_line(&mut answer)
                .map_err(|_| "Keyman cannot read your answer.".to_string())?;
            match answer.trim().to_lowercase().as_str() {
                "1" | "github" => Ok("github"),
                "2" | "google" => Ok("google"),
                _ => Err("Type 1 for GitHub or 2 for Google.".to_string()),
            }
        }
    }
}

fn login(home: &Path, flags: &cli::Flags) -> Result<String, String> {
    let provider = choose_provider(flags)?;
    account::login(home, &SystemStore::new(), provider, &mut TerminalPrompt)?;
    let email = state::load(home)?.email.unwrap_or_default();
    Ok(ui::success(&format!(
        "Signed in as {email}. Keyman Cloud is ready."
    )))
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
/// A project, or one of its stages, as named on the command line.
#[derive(Debug)]
struct Target {
    project: String,
    stage: Option<String>,
}

impl Target {
    /// `my-app` or `my-app:staging`.
    fn label(&self) -> String {
        match &self.stage {
            Some(stage) => format!("{}{}{stage}", self.project, master::STAGE_SEPARATOR),
            None => self.project.clone(),
        }
    }
}

/// Resolves `project` or `project:stage`, defaulting to the current folder name.
fn resolve_target(given: Option<&OsString>, yaml: &str) -> Result<Target, String> {
    if let Some(name) = given {
        let names = master::project_names(yaml)?;
        let name = name.to_string_lossy();
        // ':staging' names a stage of the project named after the current folder.
        if let Some(stage) = folder_stage(&name, &names) {
            return Ok(Target {
                project: resolve_project(None, yaml)?,
                stage: Some(stage.to_owned()),
            });
        }
        if let (project, Some(stage)) = master::split_target(&name, &names) {
            let project = resolve_project(Some(&OsString::from(project)), yaml)?;
            return Ok(Target {
                project,
                stage: Some(stage.to_owned()),
            });
        }
    }
    Ok(Target {
        project: resolve_project(given, yaml)?,
        stage: None,
    })
}

/// The stage in ':staging', a short form for a stage of the project named after
/// the current folder. A project whose own name starts with ':' wins.
fn folder_stage<'a>(target: &'a str, names: &[String]) -> Option<&'a str> {
    target
        .strip_prefix(master::STAGE_SEPARATOR)
        .filter(|stage| !stage.is_empty() && !names.iter().any(|name| name == target))
}

/// Cloud commands link a whole project, stages included.
fn whole_project(given: Option<&OsString>, yaml: &str) -> Result<(), String> {
    let Some(name) = given else { return Ok(()) };
    let names = master::project_names(yaml).unwrap_or_default();
    match master::split_target(&name.to_string_lossy(), &names) {
        (project, Some(_)) if names.iter().any(|n| n == project) => Err(format!(
            "Keyman Cloud syncs a whole project with all its stages\n\nUse the project name only, for example '{project}'."
        )),
        _ => Ok(()),
    }
}

fn project_for(given: Option<&OsString>) -> Result<(String, Variables), String> {
    let (_, yaml) = read_master(&home_dir()?)?;
    let target = resolve_target(given, &yaml)?;
    let project = master::read_project(&yaml, &target.project)?;
    let variables = master::stage_variables(&project, &target.project, target.stage.as_deref())?;
    Ok((target.label(), variables))
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
        let (mark, detail) = match master::read_project(&yaml, name) {
            Ok(project) if project.has_stages() => (
                green("✓"),
                dim(&format!(
                    "{} · {}: {}",
                    plural(project.shared.len(), "shared variable"),
                    if project.stages.len() == 1 {
                        "stage"
                    } else {
                        "stages"
                    },
                    project.stage_names().join(", ")
                )),
            ),
            Ok(project) => (green("✓"), dim(&plural(project.shared.len(), "variable"))),
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
    let mut lines = Vec::new();
    for name in names {
        let stages = master::read_project(&yaml, &name)
            .map(|project| project.stage_names())
            .unwrap_or_default();
        lines.push(name.clone());
        for stage in stages {
            lines.push(format!("{name}{}{stage}", master::STAGE_SEPARATOR));
        }
    }
    Ok(lines.join("\n"))
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
    variable_lines(&mut lines, variables, reveal, "  ");
    if !reveal {
        lines.push(String::new());
        lines.push(dim("Values are hidden. Add --reveal to show them."));
    }
    lines.join("\n")
}

fn variable_lines(
    lines: &mut Vec<String>,
    variables: &[(String, String)],
    reveal: bool,
    indent: &str,
) {
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
        lines.push(format!("{indent}{}  {value}", bold(&ui::pad(name, width))));
    }
}

/// `keyman show` for a project with stages: the shared variables, then each
/// stage's own variables.
fn show_stages(name: &str, project: &master::Project, reveal: bool) -> String {
    let mut lines = vec![
        format!(
            "{}{}",
            accent(&bold(name)),
            dim(&format!(
                " · {} · {}",
                plural(project.shared.len(), "shared variable"),
                plural(project.stages.len(), "stage")
            ))
        ),
        String::new(),
        bold("  Shared"),
    ];
    if project.shared.is_empty() {
        lines.push(dim("    No shared variables."));
    }
    variable_lines(&mut lines, &project.shared, reveal, "    ");
    for (stage, variables) in &project.stages {
        lines.push(String::new());
        lines.push(format!("  {}", accent(&bold(stage))));
        if variables.is_empty() {
            lines.push(dim("    Only the shared variables."));
        }
        variable_lines(&mut lines, variables, reveal, "    ");
    }
    lines.push(String::new());
    lines.push(dim(&format!(
        "A stage's values replace the shared ones. Run 'keyman show {name}{}<stage>' to see\nwhat one stage writes.",
        master::STAGE_SEPARATOR
    )));
    if !reveal {
        lines.push(dim("Values are hidden. Add --reveal to show them."));
    }
    lines.join("\n")
}

/// Reads a folder's .env and merges it into a project in the master file.
fn save_command(
    given: Option<&OsString>,
    directory: Option<&OsString>,
    flags: &cli::Flags,
) -> Result<String, String> {
    let (master_path, yaml) = read_master(&home_dir()?)?;
    let names = master::project_names(&yaml)?;
    let (full, from_folder) = match given {
        Some(name) => {
            let name = name.to_string_lossy().into_owned();
            match folder_stage(&name, &names) {
                Some(stage) => (
                    format!(
                        "{}{}{stage}",
                        current_folder_name()?,
                        master::STAGE_SEPARATOR
                    ),
                    true,
                ),
                None => (name, false),
            }
        }
        None => (current_folder_name()?, true),
    };
    let (project, stage) = master::split_target(&full, &names);
    let (project, stage) = (project.to_owned(), stage.map(str::to_owned));
    let exists = names.contains(&project);
    if !exists && !cli::has(flags, "--new") {
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
    let current = if exists {
        master::read_project(&yaml, &project).map_err(|error| {
            format!("{error}\n\nFix the project with 'keyman edit' before saving into it.")
        })?
    } else {
        master::Project::default()
    };
    let replace = cli::has(flags, "--replace");
    let new_stage = stage
        .as_deref()
        .is_some_and(|stage| current.stage(stage).is_none());
    if let Some(stage) = &stage {
        if !master::valid_stage_name(stage) {
            return Err(format!(
                "'{stage}' is not a valid stage name (use letters, digits, '-', '_' and '.')"
            ));
        }
        if exists && new_stage && !cli::has(flags, "--new") {
            let stages = current.stage_names();
            let hint = ui::did_you_mean(stage, stages.iter().map(String::as_str));
            // Keep the folder the person named, quoted when it has spaces.
            let folder = directory.map_or(String::new(), |d| {
                let d = d.to_string_lossy();
                if d.contains(char::is_whitespace) {
                    format!(" '{d}'")
                } else {
                    format!(" {d}")
                }
            });
            return Err(format!(
                "project '{project}' has no stage '{stage}'{hint}\n\nTo create it, run 'keyman save {full}{folder} --new'."
            ));
        }
    } else if current.has_stages() {
        master::stage_variables(&current, &project, None)?;
    }
    let (plan, desired) = match &stage {
        Some(stage) => save::plan_stage(&current, stage, &incoming, replace),
        None => {
            let plan = save::plan(&current.shared, &incoming, replace);
            let mut desired = current.clone();
            desired.shared = plan.result.clone();
            (plan, desired)
        }
    };
    let label = Target {
        project: project.clone(),
        stage: stage.clone(),
    }
    .label();
    let is_shared = |name: &str| stage.is_some() && current.shared.iter().any(|(n, _)| n == name);
    let env_shown = display_path(&fs::canonicalize(&env_path).unwrap_or(env_path));
    let master_shown = display_path(&master_path);

    if exists && !new_stage && plan.changes() == 0 {
        return Ok(ui::success(&format!(
            "{} is already up to date with {env_shown}",
            accent(&label)
        )));
    }

    let mut lines = vec![
        format!(
            "{} {} {} {}{}",
            bold("Saving"),
            env_shown,
            dim("→"),
            accent(&bold(&label)),
            dim(if !exists {
                " (new project)"
            } else if new_stage {
                " (new stage)"
            } else {
                ""
            })
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
            Edit::Kept if is_shared(name.trim_end()) => dim(&format!(
                "= {name}  shared by every stage, kept (change it with 'keyman edit')"
            )),
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

    if cli::has(flags, "--dry-run") {
        lines.push(dim(
            "Dry run: nothing was saved. Run again without --dry-run to save.",
        ));
        return Ok(lines.join("\n"));
    }
    let updated = save::write_project(&yaml, &project, &desired, exists)?;
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
    envfile::read(&directory.map_or_else(|| PathBuf::from("."), PathBuf::from))
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
    let (path, change) = envfile::write(variables, target_directory)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn pull_report_prints_like_before() {
        let report = PullReport {
            results: vec![PullResult::NameTaken {
                project: "app".to_string(),
            }],
            given: vec![("team".to_string(), "e@x.com".to_string())],
            warnings: vec!["A warning.".to_string()],
        };
        let output = pull_output(&report);
        assert_eq!(
            output.lines,
            vec!["Skipped app. A local project named 'app' exists. Run 'keyman pull app' to merge it with the local project, or use --as <name>."]
        );
        assert_eq!(output.code, 1);
    }

    /// Numbers each fixture so tests running in parallel never share a folder.
    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new(master: &str) -> Self {
            let root = env::temp_dir().join(format!(
                "keyman-test-{}-{}",
                process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            // A folder left by an earlier run with the same process ID.
            let _ = fs::remove_dir_all(&root);
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
    fn stages_in_list_status_show_and_targets() {
        let fixture = Fixture::new(
            "api:\n  DEBUG: \"0\"\n  prod:\n    DB: \"p\"\n  dev:\n    DEBUG: \"1\"\nweb: {}\n",
        );
        fixture.load();
        let (_, yaml) = read_master(&fixture.home).unwrap();
        assert_eq!(list(&fixture.home).unwrap(), "api\napi:prod\napi:dev\nweb");
        assert!(status(&fixture.home)
            .unwrap()
            .contains("✓ api  1 shared variable · stages: prod, dev"));
        let target = resolve_target(Some(&OsString::from("api:dev")), &yaml).unwrap();
        assert_eq!(
            (target.project.as_str(), target.label()),
            ("api", "api:dev".to_owned())
        );
        assert!(resolve_target(Some(&OsString::from("nope:dev")), &yaml)
            .unwrap_err()
            .contains("project 'nope' was not found"));
        let project = master::read_project(&yaml, "api").unwrap();
        assert_eq!(
            show_stages("api", &project, true),
            "api · 1 shared variable · 2 stages\n\n  Shared\n    DEBUG  0\n\n  prod\n    DB  p\n\n  dev\n    DEBUG  1\n\nA stage's values replace the shared ones. Run 'keyman show api:<stage>' to see\nwhat one stage writes."
        );
        assert!(whole_project(Some(&OsString::from("api:dev")), &yaml).is_err());
        assert!(whole_project(Some(&OsString::from("api")), &yaml).is_ok());
        // ':dev' is a stage of the project named after the current folder.
        let names = master::project_names(&yaml).unwrap();
        assert_eq!(folder_stage(":dev", &names), Some("dev"));
        assert_eq!(folder_stage(":", &names), None);
        assert_eq!(folder_stage("api:dev", &names), None);
        assert_eq!(folder_stage(":odd", &[":odd".to_owned()]), None);
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
    fn cloud_text_is_safe_to_print() {
        assert_eq!(
            safe_lines("Keyman cannot update 'a\u{1b}[2Jb' safely.\nline\u{202E}two"),
            "Keyman cannot update 'a?[2Jb' safely.\nline?two"
        );
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

    /// Serializes the tests that change the process environment.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores an environment variable when dropped.
    struct EnvGuard(&'static str, Option<OsString>);

    impl EnvGuard {
        fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old = env::var_os(name);
            env::set_var(name, value);
            EnvGuard(name, old)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.1 {
                Some(old) => env::set_var(self.0, old),
                None => env::remove_var(self.0),
            }
        }
    }

    /// A user who linked a project and kept a session must still work offline:
    /// the server address does not answer, so any network call would hang.
    #[test]
    fn existing_commands_do_no_network() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = Fixture::new("my-app:\n  API_KEY: \"secret\"\n");
        fixture.load();
        let target = fixture.root.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join(".env"), "API_KEY=\"changed\"\n").unwrap();
        fs::write(
            fixture.home.join(".keyman_sync"),
            r#"{"user_id":"u1","email":"a@example.com","refresh_token":"token","links":[{"project":"my-app","cloud_id":"c1","vault_id":"v1","version":1,"base":[["API_KEY","secret"]],"cloud_name":"my-app"}]}"#,
        )
        .unwrap();

        let _home = EnvGuard::set("HOME", &fixture.home);
        let _profile = EnvGuard::set("USERPROFILE", &fixture.home);
        let _url = EnvGuard::set("KEYMAN_SUPABASE_URL", "http://192.0.2.1:9");
        let target_arg = target.as_os_str().to_owned();
        let app = OsString::from("my-app");
        let commands: Vec<Vec<OsString>> = vec![
            vec!["status".into()],
            vec!["show".into(), app.clone()],
            vec!["inject".into(), app.clone(), target_arg.clone()],
            vec!["diff".into(), app.clone(), target_arg.clone()],
            vec![
                "save".into(),
                app.clone(),
                target_arg.clone(),
                "--dry-run".into(),
            ],
            vec!["export".into(), app],
        ];

        let start = std::time::Instant::now();
        for command in &commands {
            if let Err(error) = execute(command) {
                panic!("{command:?} failed: {error}");
            }
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "the commands took {:?}, so they may use the network",
            start.elapsed()
        );
    }
}
