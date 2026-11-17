//! Command definitions, help text, and argument parsing.

use std::ffi::OsString;

use crate::ui::{self, accent, bold, did_you_mean, dim, heading, highlight, Stream};

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    group: &'static str,
    pub args: &'static str,
    /// Positional arguments: `<name>` is required, `[name]` is optional.
    arguments: &'static [(&'static str, &'static str)],
    pub summary: &'static str,
    pub details: &'static str,
    pub flags: &'static [(&'static str, &'static str)],
    min: usize,
    max: usize,
}

pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

/// Commands that take a project name as their first argument.
pub const PROJECT_COMMANDS: &[&str] = &[
    "show", "inject", "save", "diff", "export", "run", "link", "unlink", "push", "pull",
];

const PROJECT: &str = "Project name in the master file. Default: the current folder name";

const GROUPS: &[&str] = &["Set up", "Projects", "In a project folder", "More", "Cloud"];

pub const COMMANDS: &[Spec] = &[
    Spec {
        name: "init",
        group: "Set up",
        args: "[file]",
        arguments: &[("[file]", "Where to create the master file. Default: ~/.keyman.yml")],
        summary: "Create a starter master file and load it",
        details: "Creates FILE (default: ~/.keyman.yml) with an example project, then loads it.\nOn Unix the file gets 600 permissions. Keyman never overwrites an existing file.",
        flags: &[],
        min: 0,
        max: 1,
    },
    Spec {
        name: "load",
        group: "Set up",
        args: "<file>",
        arguments: &[("<file>", "Your existing master file, ending in .yml or .yaml")],
        summary: "Use an existing master file",
        details: "Checks that FILE is valid, then records its absolute path in ~/.keyman_config.\nThe file must end in .yml or .yaml. On Unix its permissions are set to 600.",
        flags: &[],
        min: 1,
        max: 1,
    },
    Spec {
        name: "edit",
        group: "Set up",
        args: "",
        arguments: &[],
        summary: "Open the master file in your editor",
        details: "Opens the master file in $VISUAL, $EDITOR, or the system default\n(Notepad on Windows, vi elsewhere), then checks it when the editor closes.\nFor editors that return at once, use the wait option, for example: EDITOR=\"code --wait\"",
        flags: &[],
        min: 0,
        max: 0,
    },
    Spec {
        name: "status",
        group: "Projects",
        args: "",
        arguments: &[],
        summary: "Show the master file and check every project",
        details: "Shows the master file, then each project with its variable count or the\nproblem that stops it from loading. The project for the current folder is marked.",
        flags: &[],
        min: 0,
        max: 0,
    },
    Spec {
        name: "list",
        group: "Projects",
        args: "",
        arguments: &[],
        summary: "List project names, one per line",
        details: "Prints only the project names, so the output works in scripts.",
        flags: &[],
        min: 0,
        max: 0,
    },
    Spec {
        name: "show",
        group: "Projects",
        args: "[project] [--reveal]",
        arguments: &[("[project]", PROJECT)],
        summary: "Show a project's variables, with values masked",
        details: "Shows each variable name. Values stay masked unless you add --reveal.",
        flags: &[("--reveal", "Show the real values")],
        min: 0,
        max: 1,
    },
    Spec {
        name: "inject",
        group: "In a project folder",
        args: "[project] [directory]",
        arguments: &[("[project]", PROJECT), ("[directory]", "Folder to write .env into. Default: the current directory")],
        summary: "Write a project's variables to .env",
        details: "Writes DIRECTORY/.env (default: the current directory). An existing .env is\nreplaced. On Unix the file gets 600 permissions. Keyman warns you when the\ndirectory is in a Git repository that does not ignore .env.",
        flags: &[],
        min: 0,
        max: 2,
    },
    Spec {
        name: "save",
        group: "In a project folder",
        args: "[project] [directory] [--dry-run] [--replace] [--new]",
        arguments: &[("[project]", PROJECT), ("[directory]", "Folder whose .env is saved. Default: the current directory")],
        summary: "Save a folder's .env back into the master file",
        details: "Reads DIRECTORY/.env (default: the current directory) and updates the project\nin the master file: new variables are added and changed values are updated.\nVariables that are only in the master file are kept unless you add --replace.\nA project that does not exist yet is created.\n\nKeyman edits only the project's own lines, so comments and the rest of the\nfile stay as they are. It checks the result before writing, and stops if any\nother project would change.",
        flags: &[
            ("--dry-run", "Show the changes without saving them"),
            ("--replace", "Also remove variables that are not in .env"),
            ("--new", "Create the project even if its name is close to an existing one"),
        ],
        min: 0,
        max: 2,
    },
    Spec {
        name: "diff",
        group: "In a project folder",
        args: "[project] [directory] [--reveal]",
        arguments: &[("[project]", PROJECT), ("[directory]", "Folder whose .env is compared. Default: the current directory")],
        summary: "Compare a folder's .env with the master file",
        details: "Shows how DIRECTORY/.env (default: the current directory) differs from the\nproject in the master file, like git diff: red - lines are the master file,\ngreen + lines are .env, and dim lines are the same in both.\nValues stay masked unless you add --reveal.\n\nThen run 'keyman save' to copy .env into the master file, or\n'keyman inject' to overwrite .env from the master file.",
        flags: &[("--reveal", "Show the real values")],
        min: 0,
        max: 2,
    },
    Spec {
        name: "export",
        group: "In a project folder",
        args: "[project]",
        arguments: &[("[project]", PROJECT)],
        summary: "Print export lines for your shell",
        details: "Prints export NAME=\"value\" lines for bash, zsh, and other POSIX shells.\nTo load them into the current shell:\n\n  eval \"$(keyman export my-app)\"",
        flags: &[],
        min: 0,
        max: 1,
    },
    Spec {
        name: "run",
        group: "In a project folder",
        args: "[project] [--] <command>...",
        arguments: &[("[project]", PROJECT), ("<command>...", "The command to run, with its own arguments")],
        summary: "Run a command with a project's variables",
        details: "Runs COMMAND with the project's variables in its environment. No .env file is\nwritten. Keyman exits with the command's exit code. Put -- before the command\nwhen you omit the project or when the command starts with a dash.\n\n  keyman run my-app -- npm start\n  keyman run -- python manage.py runserver\n\nOn Windows, run .cmd and .bat scripts through cmd /c:\n\n  keyman run my-app -- cmd /c npm start",
        flags: &[],
        min: 1,
        max: usize::MAX,
    },
    Spec {
        name: "completions",
        group: "More",
        args: "<shell>",
        arguments: &[("<shell>", "One of: bash, zsh, fish, powershell")],
        summary: "Print a shell completion script",
        details: "Completes commands, options, and project names. Add one line to your shell\nstartup file:\n\n  bash        eval \"$(keyman completions bash)\"\n  zsh         source <(keyman completions zsh)\n  fish        keyman completions fish | source\n  powershell  keyman completions powershell | Out-String | Invoke-Expression",
        flags: &[],
        min: 1,
        max: 1,
    },
    Spec {
        name: "help",
        group: "More",
        args: "[command]",
        arguments: &[("[command]", "The command to explain. Default: list every command")],
        summary: "Show help for Keyman or for one command",
        details: "",
        flags: &[],
        min: 0,
        max: 1,
    },
    Spec {
        name: "login",
        group: "Cloud",
        args: "[--github] [--google]",
        arguments: &[],
        summary: "Sign in to Keyman Cloud",
        details: "Opens your browser to sign in with GitHub or Google. If you do not add an
option, Keyman asks which one to use.

The first time, Keyman asks you to make a vault password. Keyman uses it to
encrypt your secrets, and it cannot reset the password. Keyman also shows a
recovery code one time. Write it down and keep it in a safe place.

On a new computer, type the vault password you made before.",
        flags: &[("--github", "Sign in with GitHub"), ("--google", "Sign in with Google")],
        min: 0,
        max: 0,
    },
    Spec {
        name: "logout",
        group: "Cloud",
        args: "",
        arguments: &[],
        summary: "Sign out and remove the keys from this computer",
        details: "Signs out of Keyman Cloud and removes the keys from this computer.
Your projects stay in the master file, and their links stay too.",
        flags: &[],
        min: 0,
        max: 0,
    },
    Spec {
        name: "recover",
        group: "Cloud",
        args: "[--github] [--google]",
        arguments: &[],
        summary: "Set a new vault password with your recovery code",
        details: "Use this when you forgot the vault password. Keyman asks for your recovery
code, then for a new vault password.

If you are not signed in (for example on a new computer), Keyman opens your
browser to sign in first. If you do not add an option, Keyman asks which
one to use.",
        flags: &[("--github", "Sign in with GitHub"), ("--google", "Sign in with Google")],
        min: 0,
        max: 0,
    },
    Spec {
        name: "password",
        group: "Cloud",
        args: "",
        arguments: &[],
        summary: "Change your vault password",
        details: "Asks for your current vault password, then for a new one. Your projects and
your recovery code do not change.",
        flags: &[],
        min: 0,
        max: 0,
    },
    Spec {
        name: "link",
        group: "Cloud",
        args: "[project] [--group <name>]",
        arguments: &[("[project]", PROJECT)],
        summary: "Connect a project to Keyman Cloud",
        details: "Encrypts the project on this computer and sends it to Keyman Cloud. Keyman
remembers the link on this computer. Use 'keyman push' and 'keyman pull' to sync.",
        flags: &[("--group <name>", "Put the project in a shared group")],
        min: 0,
        max: 1,
    },
    Spec {
        name: "unlink",
        group: "Cloud",
        args: "[project]",
        arguments: &[("[project]", PROJECT)],
        summary: "Remove the cloud link on this computer",
        details: "Removes the link on this computer only. The project stays in the master file
and in the cloud. This command does not use the network.",
        flags: &[],
        min: 0,
        max: 1,
    },
    Spec {
        name: "push",
        group: "Cloud",
        args: "[project]",
        arguments: &[("[project]", "Project to send. Default: every linked project")],
        summary: "Send local changes to the cloud",
        details: "Sends the local changes of linked projects to the cloud. If the cloud has newer
changes, Keyman stops. Run 'keyman pull' first.",
        flags: &[],
        min: 0,
        max: 1,
    },
    Spec {
        name: "pull",
        group: "Cloud",
        args: "[project] [--as <name>] [--keep-local] [--keep-cloud]",
        arguments: &[("[project]", "Project to get. Default: every project in the cloud")],
        summary: "Get changes from the cloud and merge them",
        details: "Gets the projects from the cloud and merges them into the master file. If both
sides changed the same variable, Keyman asks which value to keep.

Use --as to give a new project a different local name.",
        flags: &[
            ("--as <name>", "Local name for a project that is new on this computer"),
            ("--keep-local", "On a conflict, keep the value on this computer"),
            ("--keep-cloud", "On a conflict, keep the value from the cloud"),
        ],
        min: 0,
        max: 1,
    },
    Spec {
        name: "cloud",
        group: "Cloud",
        args: "<action> [project]",
        arguments: &[
            ("<action>", "status or delete"),
            ("[project]", "Project for delete. Default: the current folder name"),
        ],
        summary: "Cloud status and cloud delete",
        details: "  keyman cloud status           Show each link. Works without the network.
  keyman cloud delete <project>  Delete a project from the cloud for all members.",
        flags: &[],
        min: 1,
        max: 2,
    },
];

pub fn find(name: &str) -> Result<&'static Spec, String> {
    COMMANDS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| {
            format!(
                "unknown command '{name}'{}\n\nRun 'keyman help' to see all commands.",
                did_you_mean(name, COMMANDS.iter().map(|spec| spec.name))
            )
        })
}

fn title() -> String {
    format!(
        "{} {}",
        accent(&bold("keyman")),
        dim(env!("CARGO_PKG_VERSION"))
    )
}

/// `<required>` arguments are bold, `[optional]` ones are dim.
fn paint_argument(name: &str) -> String {
    if name.starts_with('<') {
        bold(name)
    } else {
        dim(name)
    }
}

/// The command name followed by its positional arguments, plus its width
/// without color codes.
fn signature(spec: &Spec) -> (String, usize) {
    let mut plain = spec.name.to_owned();
    let mut painted = accent(spec.name);
    for (name, _) in spec.arguments {
        plain.push(' ');
        plain.push_str(name);
        painted.push(' ');
        painted.push_str(&paint_argument(name));
    }
    (painted, plain.chars().count())
}

fn padded((painted, width): (String, usize), column: usize) -> String {
    format!("{painted}{}", " ".repeat(column.saturating_sub(width)))
}

pub fn help() -> String {
    // One column width for commands and options, so descriptions line up.
    let column = COMMANDS
        .iter()
        .map(|spec| signature(spec).1)
        .chain(["-V, --version".len()])
        .max()
        .unwrap_or(0);
    let mut text = format!(
        "{}\nKeep every project's environment variables in one private YAML file.\n\n{}  keyman <command> [arguments]",
        title(),
        heading("Usage"),
    );
    for group in GROUPS {
        text.push_str(&format!("\n\n{}", heading(group)));
        for spec in COMMANDS.iter().filter(|spec| spec.group == *group) {
            text.push_str(&format!(
                "\n  {}  {}",
                padded(signature(spec), column),
                spec.summary
            ));
        }
    }
    text.push_str(&format!(
        "\n\n{}\n  {}  Show help\n  {}  Show the version\n\n{}\n{}\n{}",
        heading("Options"),
        accent(&format!("{:<column$}", "-h, --help")),
        accent(&format!("{:<column$}", "-V, --version")),
        dim(&highlight(
            Stream::Out,
            "<argument> is required. [argument] is optional."
        )),
        dim("Leave out [project] to use the current folder name."),
        dim(&highlight(
            Stream::Out,
            "Run 'keyman help <command>' for details about a command."
        )),
    ));
    text
}

pub fn usage(spec: &Spec) -> String {
    format!("keyman {} {}", spec.name, spec.args)
        .trim_end()
        .to_owned()
}

pub fn command_help(spec: &Spec) -> String {
    let mut text = format!(
        "{}  {}\n\n{}  {}",
        accent(&bold(&format!("keyman {}", spec.name))),
        spec.summary,
        heading("Usage"),
        usage(spec)
    );
    if !spec.arguments.is_empty() {
        let width = spec
            .arguments
            .iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(0);
        text.push_str(&format!("\n\n{}", heading("Arguments")));
        for (name, description) in spec.arguments {
            let label = if name.starts_with('<') {
                ui::yellow("required")
            } else {
                dim("optional")
            };
            text.push_str(&format!(
                "\n  {}{}  {label}  {description}",
                paint_argument(name),
                " ".repeat(width - name.len())
            ));
        }
    }
    if !spec.details.is_empty() {
        text.push_str("\n\n");
        text.push_str(spec.details);
    }
    let width = spec
        .flags
        .iter()
        .map(|(flag, _)| flag.len())
        .chain([10])
        .max()
        .unwrap_or(10);
    text.push_str(&format!("\n\n{}", heading("Options")));
    for (flag, description) in spec.flags {
        text.push_str(&format!(
            "\n  {}  {description}",
            accent(&format!("{flag:<width$}"))
        ));
    }
    text.push_str(&format!(
        "\n  {}  Show this help",
        accent(&format!("{:<width$}", "-h, --help"))
    ));
    text
}

/// Names the missing required argument, or says there are too many.
fn wrong_arguments(spec: &Spec, given: usize) -> String {
    let headline = if given < spec.min {
        let (name, description) = spec.arguments[given];
        format!(
            "missing {name} for 'keyman {}'\n\n{name}: {description}",
            spec.name
        )
    } else {
        format!(
            "too many arguments for 'keyman {}' (it takes at most {})",
            spec.name, spec.max
        )
    };
    format!(
        "{headline}\n\nUsage: {}\nRun 'keyman help {}' for details.",
        usage(spec),
        spec.name
    )
}

#[derive(Debug)]
pub enum Parsed<'a> {
    Help,
    Args {
        positional: Vec<&'a OsString>,
        flags: Flags<'a>,
    },
}

/// Options found by `parse`. A value option carries the argument after it.
pub type Flags<'a> = Vec<(&'static str, Option<&'a OsString>)>;

/// The option name without its value placeholder: `--group <name>` is `--group`.
pub fn flag_name(spec: &'static str) -> &'static str {
    spec.split(' ').next().unwrap_or(spec)
}

pub fn has(flags: &[(&'static str, Option<&OsString>)], name: &str) -> bool {
    flags.iter().any(|(flag, _)| *flag == name)
}

pub fn value<'a>(
    flags: &[(&'static str, Option<&'a OsString>)],
    name: &str,
) -> Option<&'a OsString> {
    flags
        .iter()
        .find(|(flag, _)| *flag == name)
        .and_then(|(_, value)| *value)
}

/// Parses the arguments after the command name. `--` ends option parsing.
pub fn parse<'a>(spec: &Spec, args: &'a [OsString]) -> Result<Parsed<'a>, String> {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut options_ended = false;
    let mut remaining = args.iter();
    while let Some(arg) = remaining.next() {
        if !options_ended {
            match arg.to_str() {
                Some("-h" | "--help") => return Ok(Parsed::Help),
                Some("--") => {
                    options_ended = true;
                    continue;
                }
                Some(option) if option.starts_with('-') && option.len() > 1 => {
                    let known = spec
                        .flags
                        .iter()
                        .find(|(flag, _)| flag_name(flag) == option);
                    match known {
                        Some((flag, _)) => {
                            let name = flag_name(flag);
                            if flag.len() > name.len() {
                                let Some(next) = remaining.next() else {
                                    return Err(format!("the option '{name}' needs a value"));
                                };
                                flags.push((name, Some(next)));
                            } else {
                                flags.push((name, None));
                            }
                        }
                        None => {
                            return Err(format!(
                                "unknown option '{option}' for 'keyman {}'{}

Usage: {}",
                                spec.name,
                                did_you_mean(
                                    option,
                                    spec.flags.iter().map(|(flag, _)| flag_name(flag))
                                ),
                                usage(spec)
                            ))
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        positional.push(arg);
    }
    if positional.len() < spec.min || positional.len() > spec.max {
        return Err(wrong_arguments(spec, positional.len()));
    }
    Ok(Parsed::Args { positional, flags })
}

/// An optional project name and the command to run.
pub type RunArgs<'a> = (Option<&'a OsString>, &'a [OsString]);

/// Splits `run` arguments into an optional project and the command.
pub fn parse_run(args: &[OsString]) -> Result<Option<RunArgs<'_>>, String> {
    let is = |arg: &OsString, text: &str| arg.to_str() == Some(text);
    let (project, command) = match args {
        [first, ..] if is(first, "-h") || is(first, "--help") => return Ok(None),
        [separator, command @ ..] if is(separator, "--") => (None, command),
        [project, separator, command @ ..] if is(separator, "--") => (Some(project), command),
        [project, command @ ..] => (Some(project), command),
        [] => (None, args),
    };
    if command.is_empty() {
        return Err(format!(
            "missing the command to run\n\nUsage: {}\nExample: keyman run my-app -- npm start",
            usage(find("run")?)
        ));
    }
    Ok(Some((project, command)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_flags_and_counts() {
        let show = find("show").unwrap();
        let input = args(&["api", "--reveal"]);
        let Parsed::Args { positional, flags } = parse(show, &input).unwrap() else {
            panic!("expected arguments");
        };
        assert_eq!(positional, [&input[0]]);
        assert_eq!(flags, [("--reveal", None)]);
        assert!(matches!(parse(show, &args(&["-h"])), Ok(Parsed::Help)));
        assert!(parse(show, &args(&["--revel"]))
            .unwrap_err()
            .contains("Did you mean '--reveal'"));
        assert!(parse(show, &args(&["a", "b"]))
            .unwrap_err()
            .contains("too many arguments"));
        let missing = parse(find("load").unwrap(), &[]).unwrap_err();
        assert!(missing.starts_with("missing <file> for 'keyman load'"));
        assert!(find("injct").unwrap_err().contains("Did you mean 'inject'"));
    }

    #[test]
    fn arguments_match_usage() {
        for spec in COMMANDS {
            let required = spec
                .arguments
                .iter()
                .take_while(|(name, _)| name.starts_with('<'))
                .count();
            for (name, _) in spec.arguments {
                assert!(spec.args.contains(name), "{} {name}", spec.name);
            }
            if spec.name != "run" {
                assert_eq!(spec.min, required, "{}", spec.name);
                assert_eq!(spec.max, spec.arguments.len(), "{}", spec.name);
            }
        }
        let help = command_help(find("load").unwrap());
        assert!(help.contains(
            "Arguments
  <file>  required  Your existing master file"
        ));
        assert!(super::help().contains("  load <file>"));
    }

    #[test]
    fn login_accepts_provider_flag() {
        let login = find("login").unwrap();
        let input = args(&["--github"]);
        let Parsed::Args { positional, flags } = parse(login, &input).unwrap() else {
            panic!("expected arguments");
        };
        assert!(positional.is_empty());
        assert_eq!(flags, [("--github", None)]);
        assert!(parse(login, &args(&["--google"])).is_ok());
        assert!(parse(login, &args(&["--gitlab"])).is_err());
        assert!(parse(login, &args(&["extra"])).is_err());
    }

    #[test]
    fn value_flag_takes_next_argument() {
        let link = find("link").unwrap();
        let input = args(&["app", "--group", "team"]);
        let Parsed::Args { positional, flags } = parse(link, &input).unwrap() else {
            panic!("expected arguments");
        };
        assert_eq!(positional, [&input[0]]);
        assert_eq!(flags, [("--group", Some(&input[2]))]);
        assert_eq!(value(&flags, "--group"), Some(&input[2]));
        assert!(has(&flags, "--group"));
        assert_eq!(value(&flags, "--other"), None);
        let pull = find("pull").unwrap();
        let input = args(&["--as", "x", "--keep-local"]);
        let Parsed::Args { positional, flags } = parse(pull, &input).unwrap() else {
            panic!("expected arguments");
        };
        assert!(positional.is_empty());
        assert!(has(&flags, "--keep-local"));
        assert_eq!(value(&flags, "--as"), Some(&input[1]));
    }

    #[test]
    fn value_flag_without_value_is_error() {
        let link = find("link").unwrap();
        assert_eq!(
            parse(link, &args(&["app", "--group"])).unwrap_err(),
            "the option '--group' needs a value"
        );
    }

    #[test]
    fn cloud_group_in_help() {
        let help = super::help();
        assert!(help.contains("Cloud"));
        assert!(help.contains("login"));
        for name in ["logout", "recover", "password"] {
            assert!(help.contains(name), "{name}");
        }
    }

    #[test]
    fn splits_run_arguments() {
        let input = args(&["api", "--", "npm", "start"]);
        let (project, command) = parse_run(&input).unwrap().unwrap();
        assert_eq!(project, Some(&input[0]));
        assert_eq!(command, &input[2..]);

        let input = args(&["--", "make"]);
        let (project, command) = parse_run(&input).unwrap().unwrap();
        assert_eq!(project, None);
        assert_eq!(command, &input[1..]);

        let input = args(&["api", "make", "-j4"]);
        let (project, command) = parse_run(&input).unwrap().unwrap();
        assert_eq!(project, Some(&input[0]));
        assert_eq!(command, &input[1..]);

        assert!(parse_run(&args(&["api"])).is_err());
        assert!(parse_run(&args(&["--help"])).unwrap().is_none());
    }
}
