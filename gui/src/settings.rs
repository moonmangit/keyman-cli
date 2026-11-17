//! The app's own settings, kept with Rune's prefs store between launches.

use rune::prefs::Prefs;

use crate::i18n::Language;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    /// Each choice with its English and Thai label.
    pub const ALL: [(Theme, &'static str, &'static str); 3] = [
        (Theme::System, "System", "ตามระบบ"),
        (Theme::Light, "Light", "สว่าง"),
        (Theme::Dark, "Dark", "มืด"),
    ];

    fn id(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn from_id(id: &str) -> Theme {
        match id {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

/// Languages, each named in itself so anyone can find their own.
pub const LANGUAGES: [(Language, &str, &str); 3] = [
    (Language::System, "System", "ตามระบบ"),
    (Language::English, "English", "English"),
    (Language::Thai, "ไทย", "ไทย"),
];

/// Interface sizes as a UI zoom factor.
pub const SIZES: [(f32, &str, &str); 4] = [
    (0.9, "Small", "เล็ก"),
    (1.0, "Default", "ปกติ"),
    (1.1, "Large", "ใหญ่"),
    (1.25, "Larger", "ใหญ่มาก"),
];

/// Window size presets, in points.
pub const WINDOW_SIZES: [((f32, f32), &str, &str); 3] = [
    ((900.0, 620.0), "Compact", "กะทัดรัด"),
    ((1100.0, 740.0), "Standard", "มาตรฐาน"),
    ((1320.0, 860.0), "Spacious", "กว้างขวาง"),
];

/// Seconds a copied secret stays on the clipboard.
pub const CLIPBOARD_TIMES: [(u64, &str, &str); 4] = [
    (15, "15 s", "15 วิ"),
    (30, "30 s", "30 วิ"),
    (60, "1 min", "1 นาที"),
    (120, "2 min", "2 นาที"),
];

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub language: Language,
    pub theme: Theme,
    /// Index into Rune's six accents.
    pub accent: usize,
    pub zoom: f32,
    pub clipboard_secs: u64,
    /// Warn when Git would commit a written .env.
    pub git_warning: bool,
    /// Remember each project's folders between launches.
    pub remember_folders: bool,
    /// Hide revealed values again when switching projects.
    pub rehide_on_switch: bool,
    /// Ask before overwriting an existing .env.
    pub confirm_overwrite: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: Language::System,
            theme: Theme::System,
            accent: 0,
            zoom: 1.0,
            clipboard_secs: 60,
            git_warning: true,
            remember_folders: true,
            rehide_on_switch: true,
            confirm_overwrite: true,
        }
    }
}

impl Settings {
    pub fn load(prefs: &Prefs) -> Settings {
        let d = Settings::default();
        Settings {
            language: prefs
                .get::<String>("language")
                .map_or(d.language, |id| Language::from_id(&id)),
            theme: prefs
                .get::<String>("theme")
                .map_or(d.theme, |id| Theme::from_id(&id)),
            accent: prefs
                .get::<usize>("accent")
                .filter(|&i| i < 6)
                .unwrap_or(d.accent),
            zoom: prefs
                .get::<f32>("zoom")
                .filter(|z| SIZES.iter().any(|(s, _, _)| s == z))
                .unwrap_or(d.zoom),
            clipboard_secs: prefs.get("clipboard_secs").unwrap_or(d.clipboard_secs),
            git_warning: prefs.get("git_warning").unwrap_or(d.git_warning),
            remember_folders: prefs.get("remember_folders").unwrap_or(d.remember_folders),
            rehide_on_switch: prefs.get("rehide_on_switch").unwrap_or(d.rehide_on_switch),
            confirm_overwrite: prefs
                .get("confirm_overwrite")
                .unwrap_or(d.confirm_overwrite),
        }
    }

    pub fn save(&self, prefs: &mut Prefs) -> Result<(), String> {
        let result = (|| {
            prefs.set("language", self.language.id())?;
            prefs.set("theme", self.theme.id())?;
            prefs.set("accent", &self.accent)?;
            prefs.set("zoom", &self.zoom)?;
            prefs.set("clipboard_secs", &self.clipboard_secs)?;
            prefs.set("git_warning", &self.git_warning)?;
            prefs.set("remember_folders", &self.remember_folders)?;
            prefs.set("rehide_on_switch", &self.rehide_on_switch)?;
            prefs.set("confirm_overwrite", &self.confirm_overwrite)
        })();
        result.map_err(|error| format!("cannot save settings: {error:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_prefs() {
        let mut prefs = Prefs::memory();
        assert_eq!(Settings::load(&prefs), Settings::default());
        let changed = Settings {
            language: Language::Thai,
            theme: Theme::Dark,
            accent: 4,
            zoom: 1.25,
            clipboard_secs: 15,
            git_warning: false,
            remember_folders: false,
            rehide_on_switch: false,
            confirm_overwrite: false,
        };
        changed.save(&mut prefs).unwrap();
        assert_eq!(Settings::load(&prefs), changed);
    }

    #[test]
    fn unknown_values_fall_back_to_defaults() {
        let mut prefs = Prefs::memory();
        prefs.set("zoom", &3.0f32).unwrap();
        prefs.set("accent", &9usize).unwrap();
        prefs.set("theme", "neon").unwrap();
        prefs.set("language", "klingon").unwrap();
        let loaded = Settings::load(&prefs);
        assert_eq!(
            (loaded.zoom, loaded.accent, loaded.theme, loaded.language),
            (1.0, 0, Theme::System, Language::System)
        );
    }
}
