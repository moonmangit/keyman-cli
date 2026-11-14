//! `keyman save`: read a .env file and update one project in the master file.
//!
//! The master file is edited as text, one project's lines at a time, so
//! comments and the layout of every other project survive. The edited text is
//! parsed again and must give exactly the expected projects and variables, or
//! nothing is written.

use crate::master::{self, Variables};

/// Reads the variables of a .env file. Accepts `NAME=value`, an optional
/// `export ` prefix, single or double quotes, and `#` comments. Errors name
/// the line but never repeat its contents, which may be secret.
pub fn parse_dotenv(text: &str) -> Result<Variables, String> {
    let mut variables: Variables = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |problem: String| format!(".env line {}: {problem}", index + 1);
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let (name, rest) = line
            .split_once('=')
            .ok_or_else(|| at("expected NAME=value".to_owned()))?;
        let name = name.trim();
        if !master::valid_variable_name(name) {
            return Err(at(format!("'{name}' is not a valid variable name")));
        }
        let value = parse_value(rest.trim_start()).map_err(at)?;
        if value.contains('\0') {
            return Err(at("the value contains a NUL character".to_owned()));
        }
        match variables.iter_mut().find(|(existing, _)| existing == name) {
            Some(entry) => entry.1 = value,
            None => variables.push((name.to_owned(), value)),
        }
    }
    Ok(variables)
}

fn parse_value(text: &str) -> Result<String, String> {
    if let Some(inner) = text.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = inner.chars();
        loop {
            match chars.next() {
                None => {
                    return Err(
                        "the closing \" is missing (multi-line values are not supported)"
                            .to_owned(),
                    )
                }
                Some('"') => break,
                Some('\\') => match chars.next() {
                    Some(escaped @ ('\\' | '"' | '$' | '`')) => value.push(escaped),
                    Some('n' | 'r') => {
                        return Err(
                            "the value contains a line break, which Keyman cannot store".to_owned()
                        )
                    }
                    Some(other) => {
                        value.push('\\');
                        value.push(other);
                    }
                    None => return Err("the closing \" is missing".to_owned()),
                },
                Some(character) => value.push(character),
            }
        }
        check_trailing(chars.as_str())?;
        Ok(value)
    } else if let Some(inner) = text.strip_prefix('\'') {
        let end = inner
            .find('\'')
            .ok_or_else(|| "the closing ' is missing".to_owned())?;
        check_trailing(&inner[end + 1..])?;
        Ok(inner[..end].to_owned())
    } else {
        // In an unquoted value, a comment starts at " #".
        let value = text.find(" #").map_or(text, |comment| &text[..comment]);
        Ok(value.trim_end().to_owned())
    }
}

fn check_trailing(rest: &str) -> Result<(), String> {
    let rest = rest.trim_start();
    if rest.is_empty() || rest.starts_with('#') {
        Ok(())
    } else {
        Err("unexpected text after the closing quote".to_owned())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edit {
    Added,
    Changed,
    Removed,
    /// Only in the master file, and kept because --replace was not given.
    Kept,
    Unchanged,
}

#[derive(Debug)]
pub struct Plan {
    pub edits: Vec<(String, Edit)>,
    pub result: Variables,
}

impl Plan {
    pub fn count(&self, edit: Edit) -> usize {
        self.edits.iter().filter(|(_, kind)| *kind == edit).count()
    }

    pub fn changes(&self) -> usize {
        self.count(Edit::Added) + self.count(Edit::Changed) + self.count(Edit::Removed)
    }
}

/// Merges `incoming` into `existing`. The master file's order is kept and new
/// variables go at the end.
pub fn plan(existing: &[(String, String)], incoming: &[(String, String)], replace: bool) -> Plan {
    let find = |list: &[(String, String)], name: &str| {
        list.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
    };
    let mut edits = Vec::new();
    let mut result = Vec::new();
    for (name, value) in existing {
        let edit = match find(incoming, name) {
            Some(new) if new == *value => Edit::Unchanged,
            Some(new) => {
                result.push((name.clone(), new));
                edits.push((name.clone(), Edit::Changed));
                continue;
            }
            None if replace => Edit::Removed,
            None => Edit::Kept,
        };
        if edit != Edit::Removed {
            result.push((name.clone(), value.clone()));
        }
        edits.push((name.clone(), edit));
    }
    for (name, value) in incoming {
        if find(existing, name).is_none() {
            result.push((name.clone(), value.clone()));
            edits.push((name.clone(), Edit::Added));
        }
    }
    Plan { edits, result }
}

/// Returns the master file text with `project` set to `plan.result`.
pub fn update_project(
    yaml: &str,
    project: &str,
    plan: &Plan,
    exists: bool,
) -> Result<String, String> {
    let unsafe_layout = || {
        format!(
            "cannot update project '{project}' safely because of how it is written in the master file\n\nRun 'keyman edit' to change it by hand, or write the project in the block style that 'keyman init' uses."
        )
    };
    let newline = if yaml.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = yaml.lines().map(str::to_owned).collect();

    if exists {
        edit_block(&mut lines, project, plan).ok_or_else(unsafe_layout)?;
    } else {
        if lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("{}:", yaml_key(project)));
        for (name, value) in &plan.result {
            lines.push(format!("  {}: {}", yaml_key(name), yaml_string(value)));
        }
    }

    let mut updated = lines.join(newline);
    updated.push_str(newline);
    verify(yaml, &updated, project, &plan.result, exists).ok_or_else(unsafe_layout)?;
    Ok(updated)
}

/// Edits the lines of an existing block-style project. Returns None when the
/// project is written in a layout this function does not handle.
fn edit_block(lines: &mut Vec<String>, project: &str, plan: &Plan) -> Option<()> {
    let header = lines
        .iter()
        .position(|line| header_rest(line, project).is_some())?;
    let rest = header_rest(&lines[header], project)?.trim();
    let rest = rest.split(" #").next().unwrap_or_default().trim();
    match rest {
        "" => {}
        "{}" => lines[header] = format!("{}:", yaml_key(project)),
        _ if rest.starts_with('#') => {}
        _ => return None,
    }

    // The block ends at the next line that starts in column 0 and is not a comment.
    let end = (header + 1..lines.len())
        .find(|&index| {
            let line = &lines[index];
            !line.is_empty() && !line.starts_with([' ', '\t', '#'])
        })
        .unwrap_or(lines.len());

    let mut variable_lines = Vec::new();
    for (index, line) in lines.iter().enumerate().take(end).skip(header + 1) {
        let content = line.trim_start();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let indent = &line[..line.len() - content.len()];
        let (key, name) = split_key(content)?;
        variable_lines.push((index, indent.to_owned(), key.to_owned(), name));
    }
    let indent = variable_lines
        .first()
        .map_or_else(|| "  ".to_owned(), |(_, indent, _, _)| indent.clone());
    if variable_lines
        .iter()
        .any(|(_, line_indent, _, _)| *line_indent != indent)
    {
        return None;
    }
    let insert_at = variable_lines
        .last()
        .map_or(header + 1, |(index, ..)| index + 1);

    let value_of = |name: &str| {
        plan.result
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value)
    };
    let added: Vec<String> = plan
        .edits
        .iter()
        .filter(|(_, edit)| *edit == Edit::Added)
        .map(|(name, _)| {
            format!(
                "{indent}{}: {}",
                yaml_key(name),
                yaml_string(value_of(name).map_or("", String::as_str))
            )
        })
        .collect();
    lines.splice(insert_at..insert_at, added);

    for (index, line_indent, key, name) in variable_lines.into_iter().rev() {
        match plan
            .edits
            .iter()
            .find(|(candidate, _)| *candidate == name)?
            .1
        {
            Edit::Changed => {
                lines[index] = format!("{line_indent}{key}: {}", yaml_string(value_of(&name)?));
            }
            Edit::Removed => {
                lines.remove(index);
            }
            Edit::Added | Edit::Kept | Edit::Unchanged => {}
        }
    }
    Some(())
}

/// When `line` is the top-level key for `project`, returns the text after the colon.
fn header_rest<'a>(line: &'a str, project: &str) -> Option<&'a str> {
    [
        format!("{project}:"),
        format!("\"{project}\":"),
        format!("'{project}':"),
    ]
    .iter()
    .find_map(|prefix| line.strip_prefix(prefix.as_str()))
    .filter(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// Splits `KEY: value` into the key as written and the key's name.
fn split_key(content: &str) -> Option<(&str, String)> {
    let (key, name) = match content.chars().next()? {
        quote @ ('"' | '\'') => {
            let end = content[1..].find(quote)? + 2;
            (&content[..end], content[1..end - 1].to_owned())
        }
        _ => {
            let end = content.find(':')?;
            let key = content[..end].trim_end();
            (key, key.to_owned())
        }
    };
    let after = content[key.len()..].trim_start();
    after.strip_prefix(':')?;
    Some((key, name))
}

/// Checks that only `project` changed and that it now holds `expected`.
fn verify(old: &str, new: &str, project: &str, expected: &Variables, existed: bool) -> Option<()> {
    let old_names = master::project_names(old).ok()?;
    let mut expected_names = old_names.clone();
    if !existed {
        expected_names.push(project.to_owned());
    }
    if master::project_names(new).ok()? != expected_names {
        return None;
    }
    let others_unchanged = old_names
        .iter()
        .filter(|name| *name != project)
        .all(|name| master::project_variables(old, name) == master::project_variables(new, name));
    (others_unchanged && master::project_variables(new, project).ok()? == *expected).then_some(())
}

/// A YAML double-quoted string.
fn yaml_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\t' => output.push('\t'),
            control if u32::from(control) < 0x20 || control == '\u{7f}' => {
                output.push_str(&format!("\\x{:02X}", u32::from(control)));
            }
            other => output.push(other),
        }
    }
    output.push('"');
    output
}

/// A YAML key: plain when that is unambiguous, otherwise double-quoted.
fn yaml_key(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "true" | "false" | "null"
        );
    if plain {
        name.to_owned()
    } else {
        yaml_string(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> Variables {
        list.iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn parses_dotenv_formats() {
        let text = "# comment\nexport A=plain # note\nB=\"q \\\"x\\\" \\$HOME\"\nC='single # kept'\n\nD=\nA=again\n";
        assert_eq!(
            parse_dotenv(text).unwrap(),
            vars(&[
                ("A", "again"),
                ("B", "q \"x\" $HOME"),
                ("C", "single # kept"),
                ("D", "")
            ])
        );
        assert!(parse_dotenv("A=\"open\n").unwrap_err().contains("line 1"));
        assert!(parse_dotenv("A=\"x\\ny\"\n").is_err());
        assert!(parse_dotenv("1A=x\n").is_err());
        assert!(parse_dotenv("just text\n")
            .unwrap_err()
            .contains("NAME=value"));
    }

    #[test]
    fn round_trips_inject_output() {
        let original = vars(&[("A", "a\"b\\c$d`e"), ("B", "x # y"), ("C", "")]);
        let written = master::format_variables(&original, "");
        assert_eq!(parse_dotenv(&written).unwrap(), original);
    }

    #[test]
    fn plans_merge_and_replace() {
        let existing = vars(&[("A", "1"), ("B", "2"), ("C", "3")]);
        let incoming = vars(&[("A", "1"), ("B", "20"), ("D", "4")]);
        let merged = plan(&existing, &incoming, false);
        assert_eq!(
            merged.edits,
            [
                ("A".to_owned(), Edit::Unchanged),
                ("B".to_owned(), Edit::Changed),
                ("C".to_owned(), Edit::Kept),
                ("D".to_owned(), Edit::Added)
            ]
        );
        assert_eq!(
            merged.result,
            vars(&[("A", "1"), ("B", "20"), ("C", "3"), ("D", "4")])
        );
        assert_eq!(merged.changes(), 2);
        let replaced = plan(&existing, &incoming, true);
        assert_eq!(
            replaced.result,
            vars(&[("A", "1"), ("B", "20"), ("D", "4")])
        );
        assert_eq!(replaced.changes(), 3);
    }

    #[test]
    fn edits_only_the_project_lines() {
        let yaml = "# my secrets\napi:\n  # database\n  A: \"1\"\n  B: \"2\"  # old\n  C: \"3\"\n\n# web app\nweb:\n  X: \"y\"\n";
        let incoming = vars(&[("A", "1"), ("B", "two \"q\""), ("D", "new")]);
        let existing = master::project_variables(yaml, "api").unwrap();
        let changes = plan(&existing, &incoming, true);
        let updated = update_project(yaml, "api", &changes, true).unwrap();
        assert_eq!(
            updated,
            "# my secrets\napi:\n  # database\n  A: \"1\"\n  B: \"two \\\"q\\\"\"\n  D: \"new\"\n\n# web app\nweb:\n  X: \"y\"\n"
        );
    }

    #[test]
    fn creates_new_and_empty_projects() {
        let yaml = "api:\n  A: \"1\"\nempty: {}\n";
        let incoming = vars(&[("TOKEN", "t")]);
        let created = update_project(yaml, "web-app", &plan(&[], &incoming, false), false).unwrap();
        assert_eq!(
            created,
            "api:\n  A: \"1\"\nempty: {}\n\nweb-app:\n  TOKEN: \"t\"\n"
        );
        let filled = update_project(yaml, "empty", &plan(&[], &incoming, false), true).unwrap();
        assert_eq!(filled, "api:\n  A: \"1\"\nempty:\n  TOKEN: \"t\"\n");
    }

    #[test]
    fn refuses_unusual_layouts() {
        let yaml = "api: {A: \"1\"}\n";
        let existing = master::project_variables(yaml, "api").unwrap();
        let changes = plan(&existing, &vars(&[("A", "2")]), false);
        assert!(update_project(yaml, "api", &changes, true)
            .unwrap_err()
            .contains("cannot update project 'api' safely"));
    }

    #[test]
    fn keeps_windows_line_endings() {
        let yaml = "api:\r\n  A: \"1\"\r\n";
        let changes = plan(&vars(&[("A", "1")]), &vars(&[("A", "2")]), false);
        assert_eq!(
            update_project(yaml, "api", &changes, true).unwrap(),
            "api:\r\n  A: \"2\"\r\n"
        );
    }
}
