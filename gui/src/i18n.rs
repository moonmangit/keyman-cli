//! English and Thai. Every string the app shows is written once in each
//! language at the place it appears: `t("Save", "บันทึก")` for fixed text,
//! `tf!("Saved {}", "บันทึกแล้ว {}", n)` for formatted text, and `count` for
//! numbers with a noun (Thai nouns have no plural form).

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    /// Follow the operating system's preferred language.
    System,
    English,
    Thai,
}

impl Language {
    pub fn id(self) -> &'static str {
        match self {
            Language::System => "system",
            Language::English => "en",
            Language::Thai => "th",
        }
    }

    pub fn from_id(id: &str) -> Language {
        match id {
            "en" => Language::English,
            "th" => Language::Thai,
            _ => Language::System,
        }
    }

    /// Whether this choice means Thai, asking the OS for `System`.
    pub fn is_thai(self) -> bool {
        match self {
            Language::Thai => true,
            Language::English => false,
            Language::System => system_is_thai(),
        }
    }
}

static THAI: AtomicBool = AtomicBool::new(false);

/// Switches every later lookup, and tells Rune's text system the language so Thai
/// shapes and breaks lines with Thai rules.
pub fn set(language: Language) {
    let thai = language.is_thai();
    THAI.store(thai, Ordering::Relaxed);
    // Cheap when unchanged: Rune only re-shapes text when the language differs.
    rune::text::set_language(Some(if thai { "th" } else { "en" }));
}

pub fn thai() -> bool {
    THAI.load(Ordering::Relaxed)
}

/// The text in the current language.
pub fn t(en: &'static str, th: &'static str) -> &'static str {
    if thai() { th } else { en }
}

/// A count with its noun: "1 variable", "3 variables", "3 ตัวแปร".
pub fn count(n: usize, singular: &str, plural: &str, thai_noun: &str) -> String {
    if thai() {
        format!("{n} {thai_noun}")
    } else if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// `format!` in the current language. Arguments are passed explicitly, the same for both.
#[macro_export]
macro_rules! tf {
    ($en:literal, $th:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::thai() { format!($th $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}

/// The OS asks for Thai first among its preferred languages. Cached: it is read on every
/// frame while the setting is System.
fn system_is_thai() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        rune_i18n::system_languages()
            .first()
            .is_some_and(|tag| tag == "th" || tag.starts_with("th-") || tag.starts_with("th_"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_language_and_counts() {
        set(Language::English);
        assert_eq!(t("Save", "บันทึก"), "Save");
        assert_eq!(count(1, "variable", "variables", "ตัวแปร"), "1 variable");
        assert_eq!(count(3, "variable", "variables", "ตัวแปร"), "3 variables");
        assert_eq!(
            tf!("Saved {} to {}", "บันทึก {} ไปที่ {}", 2, "app"),
            "Saved 2 to app"
        );
        set(Language::Thai);
        assert_eq!(t("Save", "บันทึก"), "บันทึก");
        assert_eq!(count(3, "variable", "variables", "ตัวแปร"), "3 ตัวแปร");
        assert_eq!(
            tf!("Saved {} to {}", "บันทึก {} ไปที่ {}", 2, "app"),
            "บันทึก 2 ไปที่ app"
        );
        set(Language::English);
        assert_eq!(Language::from_id(Language::Thai.id()), Language::Thai);
        assert_eq!(Language::from_id("unknown"), Language::System);
    }
}
