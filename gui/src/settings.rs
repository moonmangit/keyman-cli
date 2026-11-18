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

/// Resizable widths: (default, min, max) in points. The split is the Variables
/// column's share of the page, as a fraction.
pub const SIDEBAR_W: (f32, f32, f32) = (264.0, 230.0, 420.0);
pub const STAGES_W: (f32, f32, f32) = (208.0, 150.0, 380.0);
pub const SPLIT: (f32, f32, f32) = (0.5, 0.1, 0.9);
/// The narrowest the Variables and Folders columns get, in points.
pub const VARS_MIN: f32 = 320.0;
pub const FOLDERS_MIN: f32 = 400.0;

/// The Variables column's width for a share of `shared` points, keeping both
/// columns at their minimum while there is room for that.
pub fn split_width(split: f32, shared: f32) -> f32 {
    let width = split * shared;
    if shared >= VARS_MIN + FOLDERS_MIN {
        width.clamp(VARS_MIN, shared - FOLDERS_MIN)
    } else {
        // Too narrow for both minimums: share the room in proportion.
        shared * VARS_MIN / (VARS_MIN + FOLDERS_MIN)
    }
}

fn in_range(value: f32, (_, min, max): (f32, f32, f32)) -> bool {
    value.is_finite() && (min..=max).contains(&value)
}

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
    /// The Keyman Cloud first-use card was answered (signed in or skipped).
    pub cloud_prompt_done: bool,
    /// Width of the project list.
    pub sidebar_w: f32,
    /// Width of the stage column.
    pub stages_w: f32,
    /// The Variables column's share of the page (Folders gets the rest).
    pub split: f32,
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
            cloud_prompt_done: false,
            sidebar_w: SIDEBAR_W.0,
            stages_w: STAGES_W.0,
            split: SPLIT.0,
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
            cloud_prompt_done: prefs
                .get("cloud_prompt_done")
                .unwrap_or(d.cloud_prompt_done),
            sidebar_w: prefs
                .get::<f32>("sidebar_w")
                .filter(|&w| in_range(w, SIDEBAR_W))
                .unwrap_or(d.sidebar_w),
            stages_w: prefs
                .get::<f32>("stages_w")
                .filter(|&w| in_range(w, STAGES_W))
                .unwrap_or(d.stages_w),
            split: prefs
                .get::<f32>("split")
                .filter(|&f| in_range(f, SPLIT))
                .unwrap_or(d.split),
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
            prefs.set("confirm_overwrite", &self.confirm_overwrite)?;
            prefs.set("cloud_prompt_done", &self.cloud_prompt_done)?;
            prefs.set("sidebar_w", &self.sidebar_w)?;
            prefs.set("stages_w", &self.stages_w)?;
            prefs.set("split", &self.split)
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
            cloud_prompt_done: true,
            sidebar_w: 300.0,
            stages_w: 180.0,
            split: 0.6,
        };
        changed.save(&mut prefs).unwrap();
        assert_eq!(Settings::load(&prefs), changed);
    }

    #[test]
    fn skip_is_saved() {
        let mut prefs = Prefs::memory();
        // Prefs from before this setting: the card shows.
        prefs.set("theme", "dark").unwrap();
        assert!(!Settings::load(&prefs).cloud_prompt_done);
        let skipped = Settings {
            cloud_prompt_done: true,
            ..Settings::default()
        };
        skipped.save(&mut prefs).unwrap();
        assert!(Settings::load(&prefs).cloud_prompt_done);
    }

    #[test]
    fn split_keeps_both_columns_usable() {
        // Room for both minimums: the share applies, within them.
        assert_eq!(split_width(0.5, 1000.0), 500.0);
        assert_eq!(split_width(0.9, 1000.0), 1000.0 - FOLDERS_MIN);
        assert_eq!(split_width(0.1, 1000.0), VARS_MIN);
        // Too narrow for both: shared in proportion to the minimums.
        assert_eq!(split_width(0.9, 360.0), 160.0);
    }

    #[test]
    fn unknown_values_fall_back_to_defaults() {
        let mut prefs = Prefs::memory();
        prefs.set("zoom", &3.0f32).unwrap();
        prefs.set("accent", &9usize).unwrap();
        prefs.set("theme", "neon").unwrap();
        prefs.set("language", "klingon").unwrap();
        prefs.set("sidebar_w", &5000.0f32).unwrap();
        prefs.set("split", &f32::NAN).unwrap();
        let loaded = Settings::load(&prefs);
        assert_eq!(
            (loaded.zoom, loaded.accent, loaded.theme, loaded.language),
            (1.0, 0, Theme::System, Language::System)
        );
        assert_eq!((loaded.sidebar_w, loaded.split), (SIDEBAR_W.0, SPLIT.0));
    }
}
