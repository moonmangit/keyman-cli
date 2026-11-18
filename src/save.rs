//! `keyman save`: read a .env file and update one project in the master file.
//!
//! The master file is edited as text, one project's lines at a time, so
//! comments and the layout of every other project survive. The edited text is
//! parsed again and must give exactly the expected projects and variables, or
//! nothing is written.

use crate::master::{self, Project, Variables};

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

/// Returns the master file text with `project`'s shared variables set to
/// `plan.result`. The project's stages stay as they are.
pub fn update_project(
    yaml: &str,
    project: &str,
    plan: &Plan,
    exists: bool,
) -> Result<String, String> {
    let mut desired = if exists {
        master::read_project(yaml, project)?
    } else {
        Project::default()
    };
    desired.shared = plan.result.clone();
    write_project(yaml, project, &desired, exists)
}

/// Plans saving `incoming` (a stage's .env) into `stage`. Compares against the
/// stage's effective variables, and writes differences as the stage's own
/// values. A shared variable cannot be removed from one stage, so `--replace`
/// keeps it. Returns the plan (as effective variables) and the new project.
pub fn plan_stage(
    project: &Project,
    stage: &str,
    incoming: &[(String, String)],
    replace: bool,
) -> (Plan, Project) {
    let current = project
        .effective(stage)
        .unwrap_or_else(|| project.shared.clone());
    let mut planned = plan(&current, incoming, replace);
    let mut own = project.stage(stage).cloned().unwrap_or_default();
    let shared_value = |name: &str| {
        project
            .shared
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
    };
    for (name, edit) in &mut planned.edits {
        match edit {
            Edit::Added | Edit::Changed => {
                let value = incoming
                    .iter()
                    .find(|(candidate, _)| candidate == name)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default();
                if shared_value(name).as_ref() == Some(&value) {
                    own.retain(|(candidate, _)| candidate != name);
                } else if let Some(entry) = own.iter_mut().find(|(candidate, _)| candidate == name)
                {
                    entry.1 = value;
                } else {
                    own.push((name.clone(), value));
                }
            }
            Edit::Removed if shared_value(name).is_some() => *edit = Edit::Kept,
            Edit::Removed => own.retain(|(candidate, _)| candidate != name),
            Edit::Kept | Edit::Unchanged => {}
        }
    }
    let mut updated = project.clone();
    match updated.stages.iter_mut().find(|(name, _)| name == stage) {
        Some(entry) => entry.1 = own,
        None => updated.stages.push((stage.to_owned(), own)),
    }
    planned.result = updated.effective(stage).unwrap_or_default();
    (planned, updated)
}

/// Returns the master file text with `project` set to `desired`, stages
/// included. Only that project's changed lines are edited.
pub fn write_project(
    yaml: &str,
    project: &str,
    desired: &Project,
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
        let current = master::read_project(yaml, project)?;
        edit_block(&mut lines, project, &current, desired).ok_or_else(unsafe_layout)?;
    } else {
        if lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("{}:", yaml_key(project)));
        for (name, value) in &desired.shared {
            lines.push(format!("  {}: {}", yaml_key(name), yaml_string(value)));
        }
        for (stage, variables) in &desired.stages {
            lines.push(format!("  {}:", yaml_key(stage)));
            for (name, value) in variables {
                lines.push(format!("    {}: {}", yaml_key(name), yaml_string(value)));
            }
        }
    }

    let mut updated = lines.join(newline);
    updated.push_str(newline);
    verify(yaml, &updated, project, desired, exists).ok_or_else(unsafe_layout)?;
    Ok(updated)
}

/// A `KEY: value` line inside a project.
struct Entry {
    index: usize,
    indent: String,
    key: String,
    name: String,
    /// Written as `name:` or `name: {}`: a stage.
    stage: bool,
    inline_empty: bool,
}

fn entry(index: usize, line: &str) -> Option<Option<Entry>> {
    let content = line.trim_start();
    if content.is_empty() || content.starts_with('#') {
        return Some(None);
    }
    let indent = line[..line.len() - content.len()].to_owned();
    let (key, name) = split_key(content)?;
    let rest = content[key.len()..].trim_start().strip_prefix(':')?.trim();
    let rest = if rest.starts_with('#') {
        ""
    } else {
        rest.split(" #").next().unwrap_or_default().trim()
    };
    Some(Some(Entry {
        index,
        indent,
        key: key.to_owned(),
        name,
        stage: rest.is_empty() || rest == "{}",
        inline_empty: rest == "{}",
    }))
}

/// Line edits gathered first and applied at the end, so indexes stay valid.
#[derive(Default)]
struct Edits {
    replace: Vec<(usize, String)>,
    delete: Vec<usize>,
    insert_after: Vec<(usize, Vec<String>)>,
}

impl Edits {
    fn insert(&mut self, after: usize, line: String) {
        match self
            .insert_after
            .iter_mut()
            .find(|(index, _)| *index == after)
        {
            Some((_, lines)) => lines.push(line),
            None => self.insert_after.push((after, vec![line])),
        }
    }

    fn apply(self, lines: &mut Vec<String>) {
        let mut output = Vec::with_capacity(lines.len());
        for (index, line) in lines.drain(..).enumerate() {
            if !self.delete.contains(&index) {
                match self.replace.iter().find(|(at, _)| *at == index) {
                    Some((_, new)) => output.push(new.clone()),
                    None => output.push(line),
                }
            }
            if let Some((_, added)) = self.insert_after.iter().find(|(at, _)| *at == index) {
                output.extend(added.iter().cloned());
            }
        }
        *lines = output;
    }
}

/// Edits one list of variables written at one indent: changed values in place,
/// removed lines deleted, new variables after the last one (or after `start`).
fn edit_section(
    edits: &mut Edits,
    entries: &[&Entry],
    start: usize,
    indent: &str,
    current: &[(String, String)],
    desired: &[(String, String)],
) {
    let value = |list: &[(String, String)], name: &str| {
        list.iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
    };
    for entry in entries {
        match value(desired, &entry.name) {
            None => edits.delete.push(entry.index),
            Some(new) if value(current, &entry.name).as_ref() != Some(&new) => {
                edits.replace.push((
                    entry.index,
                    format!("{}{}: {}", entry.indent, entry.key, yaml_string(&new)),
                ))
            }
            Some(_) => {}
        }
    }
    let after = entries.last().map_or(start, |entry| entry.index);
    for (name, new) in desired {
        if value(current, name).is_none() {
            edits.insert(
                after,
                format!("{indent}{}: {}", yaml_key(name), yaml_string(new)),
            );
        }
    }
}

/// Edits the lines of an existing block-style project. Returns None when the
/// project is written in a layout this function does not handle.
fn edit_block(
    lines: &mut Vec<String>,
    project: &str,
    current: &Project,
    desired: &Project,
) -> Option<()> {
    let header = lines
        .iter()
        .position(|line| header_rest(line, project).is_some())?;
    let rest = header_rest(&lines[header], project)?.trim();
    let rest = rest.split(" #").next().unwrap_or_default().trim();
    let mut edits = Edits::default();
    match rest {
        "" => {}
        "{}" => edits
            .replace
            .push((header, format!("{}:", yaml_key(project)))),
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

    let mut entries = Vec::new();
    for (index, line) in lines.iter().enumerate().take(end).skip(header + 1) {
        if let Some(found) = entry(index, line)? {
            entries.push(found);
        }
    }
    let indent = entries
        .first()
        .map_or_else(|| "  ".to_owned(), |entry| entry.indent.clone());

    // Group the lines: shared variables at `indent`, and each stage with the
    // deeper lines under it.
    let mut shared: Vec<&Entry> = Vec::new();
    let mut stages: Vec<(&Entry, Vec<&Entry>)> = Vec::new();
    for found in &entries {
        if found.indent == indent {
            if found.stage && current.stage(&found.name).is_some() {
                stages.push((found, Vec::new()));
            } else if found.stage {
                return None;
            } else {
                shared.push(found);
            }
        } else if found.indent.starts_with(&indent) && found.indent.len() > indent.len() {
            let (stage_header, children) = stages.last_mut()?;
            if stage_header.inline_empty
                || shared.last().is_some_and(|s| s.index > stage_header.index)
            {
                return None;
            }
            if children
                .first()
                .is_some_and(|first| first.indent != found.indent)
                || found.stage
            {
                return None;
            }
            children.push(found);
        } else {
            return None;
        }
    }

    edit_section(
        &mut edits,
        &shared,
        header,
        &indent,
        &current.shared,
        &desired.shared,
    );
    let deeper = |children: &[&Entry]| {
        children
            .first()
            .map_or_else(|| format!("{indent}  "), |child| child.indent.clone())
    };
    for (stage_header, children) in &stages {
        match desired.stage(&stage_header.name) {
            None => {
                edits.delete.push(stage_header.index);
                // Its variables, and comments indented under it.
                let next = entries
                    .iter()
                    .find(|e| e.index > stage_header.index && e.indent == indent)
                    .map_or(end, |e| e.index);
                for (index, line) in lines
                    .iter()
                    .enumerate()
                    .take(next)
                    .skip(stage_header.index + 1)
                {
                    let line_indent = line.len() - line.trim_start().len();
                    if !line.trim().is_empty() && line_indent > indent.len() {
                        edits.delete.push(index);
                    }
                }
            }
            Some(variables) => {
                let own = current.stage(&stage_header.name)?;
                if stage_header.inline_empty && !variables.is_empty() {
                    edits
                        .replace
                        .push((stage_header.index, format!("{indent}{}:", stage_header.key)));
                }
                edit_section(
                    &mut edits,
                    children,
                    stage_header.index,
                    &deeper(children),
                    own,
                    variables,
                );
            }
        }
    }

    // New stages go after the project's last indented line.
    let last = (header + 1..end)
        .rev()
        .find(|&index| lines[index].starts_with([' ', '\t']) && !lines[index].trim().is_empty())
        .unwrap_or(header);
    for (stage, variables) in &desired.stages {
        if current.stage(stage).is_none() {
            edits.insert(last, format!("{indent}{}:", yaml_key(stage)));
            for (name, value) in variables {
                edits.insert(
                    last,
                    format!("{indent}  {}: {}", yaml_key(name), yaml_string(value)),
                );
            }
        }
    }
    edits.apply(lines);
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
fn verify(old: &str, new: &str, project: &str, expected: &Project, existed: bool) -> Option<()> {
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
        .all(|name| master::read_project(old, name) == master::read_project(new, name));
    let written = master::read_project(new, project).ok()?;
    (others_unchanged && sorted(&written) == sorted(expected)).then_some(())
}

/// A project in name order, to compare content whatever the line order.
fn sorted(project: &Project) -> Project {
    let mut project = project.clone();
    project.shared.sort();
    for (_, variables) in &mut project.stages {
        variables.sort();
    }
    project.stages.sort();
    project
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

    const STAGED: &str = "# secrets\napi:\n  DEBUG: \"false\"\n  LOG: \"info\"  # shared\n  production:\n    # live\n    DB: \"prod\"\n  staging:\n    DB: \"stage\"\n    DEBUG: \"true\"\n  empty: {}\n\n# web\nweb:\n  X: \"y\"\n";

    #[test]
    fn saves_into_a_stage_as_its_own_values() {
        let project = master::read_project(STAGED, "api").unwrap();
        // DEBUG back to the shared value drops the override; NEW is staging-only;
        // LOG is shared, so --replace keeps it.
        let incoming = vars(&[("DEBUG", "false"), ("DB", "stage2"), ("NEW", "n")]);
        let (planned, updated) = plan_stage(&project, "staging", &incoming, true);
        assert!(planned.edits.contains(&("LOG".to_owned(), Edit::Kept)));
        assert_eq!(
            updated.stage("staging").unwrap(),
            &vars(&[("DB", "stage2"), ("NEW", "n")])
        );
        let written = write_project(STAGED, "api", &updated, true).unwrap();
        assert_eq!(
            written,
            STAGED.replace(
                "    DB: \"stage\"\n    DEBUG: \"true\"\n",
                "    DB: \"stage2\"\n    NEW: \"n\"\n"
            )
        );
        assert_eq!(
            master::project_variables(&written, "api:staging").unwrap(),
            vars(&[
                ("DEBUG", "false"),
                ("LOG", "info"),
                ("DB", "stage2"),
                ("NEW", "n")
            ])
        );
    }

    #[test]
    fn adds_removes_and_fills_stages() {
        let mut project = master::read_project(STAGED, "api").unwrap();
        project.shared.push(("TOKEN".into(), "t".into()));
        project.stages.retain(|(name, _)| name != "production");
        project
            .stages
            .iter_mut()
            .find(|(n, _)| n == "empty")
            .unwrap()
            .1 = vars(&[("A", "1")]);
        project
            .stages
            .push(("pre-production".into(), vars(&[("DB", "pre")])));
        let written = write_project(STAGED, "api", &project, true).unwrap();
        assert_eq!(
            written,
            "# secrets\napi:\n  DEBUG: \"false\"\n  LOG: \"info\"  # shared\n  TOKEN: \"t\"\n  staging:\n    DB: \"stage\"\n    DEBUG: \"true\"\n  empty:\n    A: \"1\"\n  pre-production:\n    DB: \"pre\"\n\n# web\nweb:\n  X: \"y\"\n"
        );
        // Shared-only edits through the old entry point keep the stages.
        let changes = plan(&project.shared, &vars(&[("DEBUG", "no")]), true);
        let shared_only = update_project(&written, "api", &changes, true).unwrap();
        assert_eq!(
            master::read_project(&shared_only, "api").unwrap().stages,
            project.stages
        );
    }

    #[test]
    fn creates_a_project_with_stages() {
        let project = master::Project {
            shared: vars(&[("A", "1")]),
            stages: vec![("prod".into(), vars(&[("B", "2")])), ("dev".into(), vec![])],
        };
        assert_eq!(
            write_project("web:\n  X: \"y\"\n", "api", &project, false).unwrap(),
            "web:\n  X: \"y\"\n\napi:\n  A: \"1\"\n  prod:\n    B: \"2\"\n  dev:\n"
        );
    }
}
