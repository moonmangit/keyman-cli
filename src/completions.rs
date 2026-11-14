//! Shell completion scripts, generated from the command table so they always
//! match the commands and options. Project names come from `keyman list` at
//! completion time, so they stay current as the master file changes.

use crate::cli::{COMMANDS, PROJECT_COMMANDS, SHELLS};
use crate::ui::did_you_mean;

/// Commands whose second argument is a directory.
const DIRECTORY_COMMANDS: &[&str] = &["inject", "save", "diff"];

/// (command, space-separated flags) for every command that has flags.
fn flag_table() -> Vec<(&'static str, String)> {
    COMMANDS
        .iter()
        .filter(|spec| !spec.flags.is_empty())
        .map(|spec| {
            let flags: Vec<&str> = spec.flags.iter().map(|(flag, _)| *flag).collect();
            (spec.name, flags.join(" "))
        })
        .collect()
}

fn clean(text: &str) -> String {
    text.replace(['\'', '"'], "")
}

pub fn script(shell: &str) -> Result<String, String> {
    let names: Vec<&str> = COMMANDS.iter().map(|spec| spec.name).collect();
    let names = names.join(" ");
    let shells = SHELLS.join(" ");
    let projects_bar = PROJECT_COMMANDS.join("|");
    let projects_space = PROJECT_COMMANDS.join(" ");
    let directories_bar = DIRECTORY_COMMANDS.join("|");
    let flags = flag_table();

    let script = match shell {
        "bash" => {
            let flag_cases: String = flags
                .iter()
                .map(|(command, list)| format!("    {command}) flags=\"{list}\" ;;\n"))
                .collect();
            format!(
                r#"# keyman bash completion
_keyman() {{
  local cur=${{COMP_WORDS[COMP_CWORD]}} command=${{COMP_WORDS[1]}} flags=""
  if (( COMP_CWORD == 1 )); then
    COMPREPLY=($(compgen -W "{names}" -- "$cur"))
    return
  fi
  case $command in
{flag_cases}  esac
  if [[ $cur == -* ]]; then
    COMPREPLY=($(compgen -W "$flags" -- "$cur"))
    return
  fi
  case $command in
    {projects_bar})
      if (( COMP_CWORD == 2 )); then
        COMPREPLY=($(compgen -W "$(keyman list 2>/dev/null)" -- "$cur"))
      elif [[ $command == @({directories_bar}) ]] && (( COMP_CWORD == 3 )); then
        COMPREPLY=($(compgen -d -- "$cur"))
      fi ;;
    completions) COMPREPLY=($(compgen -W "{shells}" -- "$cur")) ;;
    help) COMPREPLY=($(compgen -W "{names}" -- "$cur")) ;;
  esac
}}
shopt -s extglob
complete -o default -F _keyman keyman keyman-cli
"#
            )
        }
        "zsh" => {
            let described: Vec<String> = COMMANDS
                .iter()
                .map(|spec| format!("    '{}:{}'", spec.name, clean(spec.summary)))
                .collect();
            let flag_cases: String = flags
                .iter()
                .map(|(command, list)| format!("    {command}) flags=({list}) ;;\n"))
                .collect();
            format!(
                r#"# keyman zsh completion
_keyman() {{
  local -a commands flags
  commands=(
{described}
  )
  if (( CURRENT == 2 )); then
    _describe 'command' commands
    return
  fi
  case $words[2] in
{flag_cases}  esac
  if [[ $PREFIX == -* ]]; then
    compadd -- $flags
    return
  fi
  case $words[2] in
    {projects_bar})
      if (( CURRENT == 3 )); then
        compadd -- ${{(f)"$(keyman list 2>/dev/null)"}}
      elif [[ $words[2] == ({directories_bar}) ]] && (( CURRENT == 4 )); then
        _directories
      fi ;;
    init|load) _files -g '*.(yml|yaml)' ;;
    completions) compadd -- {shells} ;;
    help) _describe 'command' commands ;;
  esac
}}
compdef _keyman keyman keyman-cli
"#,
                described = described.join("\n")
            )
        }
        "fish" => {
            let mut lines = vec![
                "# keyman fish completion".to_owned(),
                "complete -c keyman -f".to_owned(),
            ];
            for spec in COMMANDS {
                lines.push(format!(
                    "complete -c keyman -n __fish_use_subcommand -a {} -d '{}'",
                    spec.name,
                    clean(spec.summary)
                ));
                for (flag, description) in spec.flags {
                    lines.push(format!(
                        "complete -c keyman -n '__fish_seen_subcommand_from {}' -l {} -d '{}'",
                        spec.name,
                        flag.trim_start_matches('-'),
                        clean(description)
                    ));
                }
            }
            lines.extend([
                format!("complete -c keyman -n '__fish_seen_subcommand_from {projects_space}; and test (count (commandline -opc)) -eq 2' -a '(keyman list 2>/dev/null)'"),
                format!("complete -c keyman -n '__fish_seen_subcommand_from {}; and test (count (commandline -opc)) -eq 3' -a '(__fish_complete_directories)'", DIRECTORY_COMMANDS.join(" ")),
                "complete -c keyman -n '__fish_seen_subcommand_from init load' -F".to_owned(),
                format!("complete -c keyman -n '__fish_seen_subcommand_from completions' -a '{shells}'"),
                format!("complete -c keyman -n '__fish_seen_subcommand_from help' -a '{names}'"),
                "complete -c keyman-cli -w keyman".to_owned(),
            ]);
            lines.join("\n") + "\n"
        }
        "powershell" => {
            let quote = |list: &str| {
                list.split(' ')
                    .map(|item| format!("'{item}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let flag_cases: String = flags
                .iter()
                .map(|(command, list)| {
                    format!("    '{command}' {{ $flags = @({}) }}\n", quote(list))
                })
                .collect();
            format!(
                r#"# keyman PowerShell completion
Register-ArgumentCompleter -Native -CommandName keyman, keyman-cli -ScriptBlock {{
  param($wordToComplete, $commandAst, $cursorPosition)
  $words = @($commandAst.CommandElements | ForEach-Object {{ $_.ToString() }})
  $position = $words.Count
  if ($wordToComplete) {{ $position-- }}
  $flags = @()
  switch ($words[1]) {{
{flag_cases}  }}
  $candidates = @()
  if ($position -eq 1) {{
    $candidates = @({commands})
  }} elseif ($wordToComplete -like '-*') {{
    $candidates = $flags
  }} elseif ($position -eq 2) {{
    switch ($words[1]) {{
      {{ $_ -in @({projects}) }} {{ $candidates = @(keyman list 2>$null) }}
      'completions' {{ $candidates = @({shells}) }}
      'help' {{ $candidates = @({commands}) }}
    }}
  }}
  $candidates | Where-Object {{ $_ -like "$wordToComplete*" }} | ForEach-Object {{
    [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
  }}
}}
"#,
                commands = quote(&names),
                projects = quote(&projects_space),
                shells = quote(&shells),
            )
        }
        _ => {
            return Err(format!(
                "unsupported shell '{shell}'{}\n\nSupported shells: {}",
                did_you_mean(shell, SHELLS.iter().copied()),
                SHELLS.join(", ")
            ))
        }
    };
    Ok(script)
}
