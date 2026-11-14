//! Terminal output: one small visual language used by every command.
//!
//! Colors come from the terminal's own 16-color palette, so they follow the
//! user's theme. Cyan marks names you can type (projects, commands), green,
//! yellow, and red mark outcomes, dim marks secondary details, and bold
//! marks headings. Without color support the text is unchanged.

use std::env;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub enum Stream {
    Out,
    Err,
}

#[derive(Clone, Copy)]
pub enum Tone {
    Accent,
    Success,
    Warning,
    Danger,
    Muted,
    Strong,
}

impl Tone {
    /// Start and end codes. Each tone resets only its own attribute, so tones
    /// can nest without cancelling the outer style.
    fn codes(self) -> (&'static str, &'static str) {
        match self {
            Tone::Accent => ("36", "39"),
            Tone::Success => ("32", "39"),
            Tone::Warning => ("33", "39"),
            Tone::Danger => ("31", "39"),
            Tone::Muted => ("2", "22"),
            Tone::Strong => ("1", "22"),
        }
    }
}

fn color_enabled(stream: Stream) -> bool {
    static OUT: OnceLock<bool> = OnceLock::new();
    static ERR: OnceLock<bool> = OnceLock::new();
    let cell = match stream {
        Stream::Out => &OUT,
        Stream::Err => &ERR,
    };
    *cell.get_or_init(|| detect_color(stream))
}

/// Follows the NO_COLOR and FORCE_COLOR conventions, then uses color only
/// for terminals that understand ANSI escape codes.
fn detect_color(stream: Stream) -> bool {
    let set = |name| env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0");
    if cfg!(test) || set("NO_COLOR") {
        return false;
    }
    if set("FORCE_COLOR") {
        return true;
    }
    let terminal = match stream {
        Stream::Out => io::stdout().is_terminal(),
        Stream::Err => io::stderr().is_terminal(),
    };
    let dumb = env::var_os("TERM").is_some_and(|term| term == "dumb");
    // Legacy Windows consoles print escape codes literally. Windows Terminal,
    // VS Code, and MSYS terminals announce themselves with these variables.
    let ansi = !cfg!(windows)
        || [
            "WT_SESSION",
            "TERM",
            "TERM_PROGRAM",
            "ConEmuANSI",
            "ANSICON",
        ]
        .iter()
        .any(|name| env::var_os(name).is_some());
    terminal && !dumb && ansi
}

pub fn paint(stream: Stream, tone: Tone, text: &str) -> String {
    if text.is_empty() || !color_enabled(stream) {
        return text.to_owned();
    }
    let (start, end) = tone.codes();
    format!("\x1b[{start}m{text}\x1b[{end}m")
}

pub fn accent(text: &str) -> String {
    paint(Stream::Out, Tone::Accent, text)
}

pub fn bold(text: &str) -> String {
    paint(Stream::Out, Tone::Strong, text)
}

pub fn dim(text: &str) -> String {
    paint(Stream::Out, Tone::Muted, text)
}

pub fn green(text: &str) -> String {
    paint(Stream::Out, Tone::Success, text)
}

pub fn yellow(text: &str) -> String {
    paint(Stream::Out, Tone::Warning, text)
}

pub fn red(text: &str) -> String {
    paint(Stream::Out, Tone::Danger, text)
}

/// A heading such as "Usage" or "Next steps".
pub fn heading(text: &str) -> String {
    bold(text)
}

pub fn success(text: &str) -> String {
    format!("{} {text}", green(&bold("✓")))
}

/// Prints an error. The first paragraph is the headline; later paragraphs
/// are hints, indented and dimmed so the headline stands out.
pub fn error(message: &str) {
    let mut paragraphs = message.split("\n\n");
    let mut headline = paragraphs.next().unwrap_or_default().lines();
    let mark = paint(
        Stream::Err,
        Tone::Danger,
        &paint(Stream::Err, Tone::Strong, "✗"),
    );
    let first = highlight(
        Stream::Err,
        &capitalize(headline.next().unwrap_or_default()),
    );
    let mut output = format!("{mark} {}", paint(Stream::Err, Tone::Strong, &first));
    for line in headline {
        output.push_str(&format!("\n  {}", highlight(Stream::Err, line)));
    }
    let hints: Vec<&str> = paragraphs.flat_map(str::lines).collect();
    if !hints.is_empty() {
        output.push('\n');
        for hint in hints {
            let hint = paint(Stream::Err, Tone::Muted, &highlight(Stream::Err, hint));
            output.push_str(&format!("\n  {hint}"));
        }
    }
    eprintln!("{output}");
}

pub fn warning(message: &str) {
    let mark = paint(
        Stream::Err,
        Tone::Warning,
        &paint(Stream::Err, Tone::Strong, "!"),
    );
    eprintln!("{mark} {}", highlight(Stream::Err, &capitalize(message)));
}

pub fn note(message: &str) {
    let text = highlight(Stream::Err, &capitalize(message));
    eprintln!(
        "{} {}",
        paint(Stream::Err, Tone::Accent, "›"),
        paint(Stream::Err, Tone::Muted, &text)
    );
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Colors 'quoted' names and commands in a message. A quote opens only after
/// a non-letter and closes only before one, so apostrophes in words such as
/// "project's" are left alone. Without color the text is unchanged.
pub fn highlight(stream: Stream, text: &str) -> String {
    if !color_enabled(stream) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let opens = chars[index] == '\'' && (index == 0 || !chars[index - 1].is_alphanumeric());
        let close = opens
            .then(|| {
                (index + 2..chars.len()).find(|&end| {
                    chars[end] == '\''
                        && chars
                            .get(end + 1)
                            .is_none_or(|next| !next.is_alphanumeric())
                })
            })
            .flatten();
        match close {
            Some(end) => {
                let inner: String = chars[index + 1..end].iter().collect();
                output.push_str(&paint(stream, Tone::Accent, &inner));
                index = end + 1;
            }
            None => {
                output.push(chars[index]);
                index += 1;
            }
        }
    }
    output
}

/// Shows a path the way people type it: no Windows `\\?\` prefix, and the
/// home directory shortened to `~`.
pub fn display_path(path: &Path) -> String {
    let text = strip_verbatim(&path.display().to_string());
    if let Ok(home) = crate::master::home_dir() {
        let home = strip_verbatim(&home.display().to_string());
        let home = home.trim_end_matches(['/', '\\']);
        if !home.is_empty() {
            if let Some(rest) = text.strip_prefix(home) {
                if rest.is_empty() || rest.starts_with(['/', '\\']) {
                    return format!("~{rest}");
                }
            }
        }
    }
    text
}

fn strip_verbatim(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_owned()
    }
}

pub fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// Pads to a width measured in characters, before any color is applied.
pub fn pad(text: &str, width: usize) -> String {
    format!("{text:<width$}")
}

/// Returns the closest candidate when it is near enough to be a likely typo.
pub fn suggest<'a>(input: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (input.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .map(|candidate| (edit_distance(input, candidate), candidate))
        .filter(|(distance, _)| *distance <= limit)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

/// Formats a "Did you mean" hint, or nothing when no candidate is close.
pub fn did_you_mean<'a>(input: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    suggest(input, candidates)
        .map(|candidate| format!("\n\nDid you mean '{candidate}'?"))
        .unwrap_or_default()
}

/// Edit distance where swapping two neighboring letters counts as one edit,
/// so "wbe" is one step from "web".
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut rows = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(rows[i - 2][j - 2] + 1);
            }
            rows[i][j] = best;
        }
    }
    rows[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_names_only() {
        let names = ["inject", "init", "export"];
        assert_eq!(suggest("injct", names), Some("inject"));
        assert_eq!(suggest("exprot", names), Some("export"));
        assert_eq!(suggest("zzz", names), None);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(suggest("wbe", ["web", "api"]), Some("web"));
    }

    #[test]
    fn strips_windows_verbatim_prefix() {
        assert_eq!(strip_verbatim(r"\\?\C:\work\.env"), r"C:\work\.env");
        assert_eq!(strip_verbatim(r"\\?\UNC\server\share"), r"\\server\share");
        assert_eq!(strip_verbatim("/home/me/.env"), "/home/me/.env");
    }

    #[test]
    fn text_is_unchanged_without_color() {
        let text = "project 'api' isn't loaded";
        assert_eq!(highlight(Stream::Err, text), text);
        assert_eq!(capitalize("no master file"), "No master file");
    }
}
