//! Command definitions, help text, and argument parsing.

use std::ffi::OsString;

use crate::ui::{accent, bold, did_you_mean, dim, heading, highlight, Stream};

#[derive(Debug)]
pub struct Spec {
    pub name: &'static str,
    group: &'static str,
    pub args: &'static str,
    pub summary: &'static str,
    pub details: &'static str,
    pub flags: &'static [(&'static str, &'static str)],
    min: usize,
    max: usize,
}

pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

/// Commands that take a project name as their first argument.
pub const PROJECT_COMMANDS: &[&str] = &["show", "inject", "save", "diff", "export", "run"];

const GROUPS: &[&str] = &["Set up", "Projects", "In a project folder", "More"];

pub const COMMANDS: &[Spec] = &[
    Spec {
        name: "init",
        group: "Set up",
        args: "[file]",
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
        summary: "Show help for Keyman or for one command",
        details: "",
        flags: &[],
        min: 0,
        max: 1,
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

pub fn help() -> String {
    // One column width for commands and options, so descriptions line up.
    let width = COMMANDS
        .iter()
        .map(|spec| spec.name.len())
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
                accent(&format!("{:<width$}", spec.name)),
                spec.summary
            ));
        }
    }
    text.push_str(&format!(
        "\n\n{}\n  {}  Show help\n  {}  Show the version\n\n{}\n{}",
        heading("Options"),
        accent(&format!("{:<width$}", "-h, --help")),
        accent(&format!("{:<width$}", "-V, --version")),
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

fn wrong_arguments(spec: &Spec) -> String {
    format!(
        "wrong arguments for 'keyman {}'\n\nUsage: {}\nRun 'keyman help {}' for details.",
        spec.name,
        usage(spec),
        spec.name
    )
}

#[derive(Debug)]
pub enum Parsed<'a> {
    Help,
    Args {
        positional: Vec<&'a OsString>,
        flags: Vec<&'static str>,
    },
}

/// Parses the arguments after the command name. `--` ends option parsing.
pub fn parse<'a>(spec: &Spec, args: &'a [OsString]) -> Result<Parsed<'a>, String> {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut options_ended = false;
    for arg in args {
        if !options_ended {
            match arg.to_str() {
                Some("-h" | "--help") => return Ok(Parsed::Help),
                Some("--") => {
                    options_ended = true;
                    continue;
                }
                Some(option) if option.starts_with('-') && option.len() > 1 => {
                    let known = spec.flags.iter().find(|(flag, _)| *flag == option);
                    match known {
                        Some((flag, _)) => flags.push(*flag),
                        None => {
                            return Err(format!(
                                "unknown option '{option}' for 'keyman {}'{}\n\nUsage: {}",
                                spec.name,
                                did_you_mean(option, spec.flags.iter().map(|(flag, _)| *flag)),
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
        return Err(wrong_arguments(spec));
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
        assert_eq!(flags, ["--reveal"]);
        assert!(matches!(parse(show, &args(&["-h"])), Ok(Parsed::Help)));
        assert!(parse(show, &args(&["--revel"]))
            .unwrap_err()
            .contains("Did you mean '--reveal'"));
        assert!(parse(show, &args(&["a", "b"])).is_err());
        assert!(find("injct").unwrap_err().contains("Did you mean 'inject'"));
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
