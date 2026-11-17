//! Keyman for the desktop: the `keyman` commands as a window. Pick a project,
//! tie it to its folders, then write, compare, or save its .env files.
//! Every string is in English and Thai (`i18n`).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
// The cloud screens (sign-in, link, sync) use this; until then parts are unused.
#[allow(dead_code)]
mod cloud;
mod core;
mod diagram;
mod icons;
mod menu;
mod resolve;
mod settings;
mod ties;
mod views;
mod widgets;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use keyman_cli::envfile::Change;
use keyman_cli::ui::display_path;
use rune::prefs::Prefs;
use rune::{
    App as RuneApp, AppHandle, AppSpec, FileDialogOptions, Key, KeyEvent, TextEdit, Titlebar, Tx,
};

use crate::cloud::{Busy, CloudState, Question};
use crate::core::{Conflict, DiffLine, FolderStatus, Master, SavePreview};
use crate::i18n::{count, t};
use crate::settings::Settings;
use crate::ties::Ties;

const APP_ID: &str = "dev.keyman.app";
pub const DOCS_URL: &str = "https://github.com/moonmangit/keyman-cli#readme";
pub const REPO_URL: &str = "https://github.com/moonmangit/keyman-cli";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsPage {
    General,
    Appearance,
    Privacy,
    About,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HelpPage {
    Start,
    Shortcuts,
    Commands,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialog {
    Settings(SettingsPage),
    Help(HelpPage),
    ConfirmOverwrite,
    ConfirmWriteAll,
    Resolve,
    /// The compare or save-to-master preview in `App::panel`.
    Panel,
}

/// What the compare / save modal shows.
pub enum Panel {
    None,
    Diff(Vec<DiffLine>),
    Save(SavePreview),
}

/// Which value a conflicting variable keeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    Master,
    Env,
    Edit,
}

pub struct ResolveItem {
    pub conflict: Conflict,
    pub pick: Pick,
    /// The value typed when `pick` is `Edit`.
    pub edit: TextEdit,
}

/// The conflicts between a project and one tied folder, being resolved.
pub struct Resolver {
    pub folder: PathBuf,
    pub items: Vec<ResolveItem>,
    pub reveal: bool,
}

/// Where a folder's action panel opens.
#[derive(Clone, PartialEq, Debug)]
pub enum MenuAt {
    /// The right-click point, in window points.
    Point(f32, f32),
    /// Below an element, by key (the ⋯ button).
    Below(String),
}

/// The right-click action panel for one tied folder.
pub struct FolderMenu {
    pub folder: PathBuf,
    pub at: MenuAt,
}

pub struct Toast {
    pub id: u64,
    pub ok: bool,
    pub text: String,
}

pub struct App {
    pub home: Option<PathBuf>,
    pub handle: Option<AppHandle<App>>,
    pub prefs: Prefs,
    pub settings: Settings,
    pub master: Result<Master, String>,
    pub selected: Option<String>,
    /// Each project's tied folders and which one is active.
    pub ties: Ties,
    pub revealed: BTreeSet<String>,
    pub reveal_all: bool,
    pub panel: Panel,
    pub replace: bool,
    pub dialog: Option<Dialog>,
    pub menu: Option<FolderMenu>,
    pub toast: Option<Toast>,
    pub search: TextEdit,
    pub resolver: Option<Resolver>,
    /// Backs a resolver field for the one frame it may outlive its resolver.
    pub scratch: TextEdit,
    /// The cloud sign-in on this computer, from local data.
    pub cloud: CloudState,
    /// The cloud task that runs now; only one at a time.
    pub busy: Option<Busy>,
    /// The cloud task's question that waits for a dialog answer.
    pub question: Option<Question>,
}

impl App {
    fn new(handle: Option<AppHandle<App>>, prefs: Prefs) -> App {
        let home = keyman_cli::master::home_dir().ok();
        let cloud = home
            .as_deref()
            .map_or(CloudState::SignedOut, cloud::initial_state);
        let mut app = App::build(handle, prefs, home, cloud);
        i18n::set(app.settings.language);
        app.reload();
        app
    }

    /// An app for unit tests: no window, no home folder, no credential store, no reload.
    #[cfg(test)]
    pub fn for_tests() -> App {
        App::build(None, Prefs::memory(), None, CloudState::SignedOut)
    }

    fn build(
        handle: Option<AppHandle<App>>,
        prefs: Prefs,
        home: Option<PathBuf>,
        cloud: CloudState,
    ) -> App {
        let settings = Settings::load(&prefs);
        let ties = if settings.remember_folders {
            Ties::load(&prefs)
        } else {
            Ties::default()
        };
        App {
            home,
            handle,
            prefs,
            settings,
            master: Err(String::new()),
            selected: None,
            ties,
            revealed: BTreeSet::new(),
            reveal_all: false,
            panel: Panel::None,
            replace: false,
            dialog: None,
            menu: None,
            toast: None,
            search: TextEdit::new(""),
            resolver: None,
            scratch: TextEdit::new(""),
            cloud,
            busy: None,
            question: None,
        }
    }

    pub fn home(&self) -> Result<PathBuf, String> {
        self.home
            .clone()
            .ok_or_else(|| t("cannot find the home directory", "หาโฟลเดอร์หลักของผู้ใช้ไม่พบ").to_owned())
    }

    /// Re-reads the master file, keeping the selection when the project still exists.
    pub fn reload(&mut self) {
        self.master = self.home().and_then(|home| core::read(&home));
        let names: Vec<String> = match &self.master {
            Ok(master) => master.projects.iter().map(|p| p.name.clone()).collect(),
            Err(_) => Vec::new(),
        };
        if !self
            .selected
            .as_ref()
            .is_some_and(|name| names.contains(name))
        {
            self.select(names.into_iter().next());
        }
    }

    /// Reload, then say whether the master file is valid.
    pub fn reload_and_report(&mut self) {
        self.reload();
        match &self.master {
            Ok(master) if master.problems() > 0 => {
                let n = master.problems();
                self.fail(if i18n::thai() {
                    format!("มี {n} โปรเจกต์ที่มีปัญหา แก้ไขไฟล์หลักเพื่อแก้")
                } else {
                    format!(
                        "{} {} a problem. Edit the master file to fix it.",
                        count(n, "project", "projects", ""),
                        if n == 1 { "has" } else { "have" }
                    )
                });
            }
            Ok(master) => {
                let message = tf!(
                    "Master file is valid · {}",
                    "ไฟล์หลักถูกต้อง · {}",
                    count(master.projects.len(), "project", "projects", "โปรเจกต์")
                );
                self.ok(message);
            }
            Err(error) => {
                let error = error.clone();
                self.fail(error);
            }
        }
    }

    pub fn select(&mut self, name: Option<String>) {
        if self.selected != name {
            self.selected = name;
            self.panel = Panel::None;
            self.menu = None;
            if self.settings.rehide_on_switch {
                self.revealed.clear();
                self.reveal_all = false;
            }
        }
    }

    pub fn variables(&self) -> Option<&Result<Vec<(String, String)>, String>> {
        let name = self.selected.as_ref()?;
        self.master
            .as_ref()
            .ok()?
            .project(name)
            .map(|project| &project.variables)
    }

    /// The selected project's active folder.
    pub fn folder(&self) -> Option<PathBuf> {
        let name = self.selected.as_ref()?;
        self.ties.active(name).map(PathBuf::from)
    }

    /// The selected project's tied folders.
    pub fn tied(&self) -> Vec<PathBuf> {
        self.selected.as_ref().map_or_else(Vec::new, |name| {
            self.ties.folders(name).iter().map(PathBuf::from).collect()
        })
    }

    /// Ties a folder to a project and makes it the active one.
    pub fn tie(&mut self, project: &str, folder: &Path) {
        self.ties.tie(project, &folder.to_string_lossy());
        self.panel = Panel::None;
        self.store_ties();
    }

    pub fn untie(&mut self, folder: &Path) {
        let Some(project) = self.selected.clone() else {
            return;
        };
        self.ties.untie(&project, &folder.to_string_lossy());
        self.panel = Panel::None;
        self.store_ties();
        self.ok(tf!(
            "Untied {} from {}",
            "เลิกผูก {} จาก {} แล้ว",
            display_path(folder),
            project
        ));
    }

    pub fn set_active(&mut self, folder: &Path) {
        let Some(project) = self.selected.clone() else {
            return;
        };
        self.ties.set_active(&project, &folder.to_string_lossy());
        self.panel = Panel::None;
        self.store_ties();
    }

    fn store_ties(&mut self) {
        let result = if self.settings.remember_folders {
            self.ties.save(&mut self.prefs)
        } else {
            Ties::forget(&mut self.prefs)
        };
        if let Err(error) = result {
            self.fail(error);
        }
    }

    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.remember_folders;
        change(&mut self.settings);
        i18n::set(self.settings.language);
        if before != self.settings.remember_folders {
            self.store_ties();
        }
        if let Err(error) = self.settings.save(&mut self.prefs) {
            self.fail(error);
        }
    }

    pub fn clipboard_time(&self) -> Duration {
        Duration::from_secs(self.settings.clipboard_secs)
    }

    /// Closes the compare / save modal.
    pub fn close_panel(&mut self) {
        self.panel = Panel::None;
        if self.dialog == Some(Dialog::Panel) {
            self.dialog = None;
        }
    }

    // ── Feedback ────────────────────────────────────────────────────────────

    /// A success toast that hides itself after a few seconds.
    pub fn ok(&mut self, text: impl Into<String>) {
        let id = self.show_toast(true, text.into());
        if let Some(handle) = self.handle.clone() {
            let _ = rune::task::spawn_after("keyman-toast", Duration::from_secs(4), move || {
                handle.update(move |s, _| {
                    if s.toast.as_ref().is_some_and(|toast| toast.id == id) {
                        s.toast = None;
                    }
                });
            });
        }
    }

    /// An error toast; it stays until dismissed.
    pub fn fail(&mut self, text: impl Into<String>) {
        self.show_toast(false, text.into());
    }

    fn show_toast(&mut self, ok: bool, text: String) -> u64 {
        let id = self.toast.as_ref().map_or(1, |toast| toast.id + 1);
        self.toast = Some(Toast { id, ok, text });
        id
    }

    // ── Commands on the active folder ───────────────────────────────────────

    /// Makes `folder` active, then runs `action` on it (the action panel's items).
    pub fn on_folder(&mut self, folder: &Path, action: fn(&mut App)) {
        self.menu = None;
        self.set_active(folder);
        action(self);
    }

    /// `keyman inject`, asking first when it would overwrite a different .env.
    pub fn inject(&mut self) {
        let (Some(folder), Some(Ok(variables))) = (self.folder(), self.variables()) else {
            return;
        };
        if self.settings.confirm_overwrite && core::env_would_change(variables, &folder) {
            self.dialog = Some(Dialog::ConfirmOverwrite);
        } else {
            self.inject_now();
        }
    }

    pub fn inject_now(&mut self) {
        self.dialog = None;
        let (Some(project), Some(folder), Some(Ok(variables))) = (
            self.selected.clone(),
            self.folder(),
            self.variables().cloned(),
        ) else {
            return;
        };
        let warn = self.settings.git_warning;
        let result = core::inject(&variables, &folder).map(|injected| {
            let path = display_path(&injected.path);
            let n = count(variables.len(), "variable", "variables", "ตัวแปร");
            let mut message = match injected.change {
                Change::Created => tf!(
                    "Created {} · {} from {}",
                    "สร้าง {} แล้ว · {} จาก {}",
                    path,
                    n,
                    project
                ),
                Change::Replaced => tf!(
                    "Updated {} · {} from {}",
                    "อัปเดต {} แล้ว · {} จาก {}",
                    path,
                    n,
                    project
                ),
                Change::Unchanged => tf!("{} is already up to date", "{} เป็นปัจจุบันอยู่แล้ว", path),
            };
            if warn && injected.unignored {
                message.push('\n');
                message.push_str(t(
                    "Git does not ignore .env here. Add '.env' to .gitignore.",
                    "Git ไม่ได้ละเว้น .env ในโฟลเดอร์นี้ เพิ่ม '.env' ลงใน .gitignore",
                ));
            }
            (message, warn && injected.unignored)
        });
        self.panel = Panel::None;
        match result {
            Ok((message, false)) => self.ok(message),
            Ok((message, true)) => self.fail(message),
            Err(error) => self.fail(error),
        }
    }

    /// `keyman inject` into every tied folder that exists.
    pub fn write_all(&mut self) {
        let Some(Ok(variables)) = self.variables() else {
            return;
        };
        let would_change = self
            .tied()
            .iter()
            .any(|folder| core::env_would_change(variables, folder));
        if self.settings.confirm_overwrite && would_change {
            self.dialog = Some(Dialog::ConfirmWriteAll);
        } else {
            self.write_all_now();
        }
    }

    pub fn write_all_now(&mut self) {
        self.dialog = None;
        let Some(Ok(variables)) = self.variables().cloned() else {
            return;
        };
        let (mut written, mut current, mut skipped, mut unignored) = (0, 0, 0, Vec::new());
        let mut errors = Vec::new();
        for folder in self.tied() {
            if !folder.is_dir() {
                skipped += 1;
                continue;
            }
            match core::inject(&variables, &folder) {
                Ok(injected) => {
                    if injected.change == Change::Unchanged {
                        current += 1
                    } else {
                        written += 1
                    }
                    if injected.unignored {
                        unignored.push(display_path(&folder));
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        self.panel = Panel::None;
        let mut message = tf!(
            "Wrote .env to {}",
            "เขียน .env ลง {} แล้ว",
            count(written, "folder", "folders", "โฟลเดอร์")
        );
        if current > 0 {
            message.push_str(&tf!(
                " · {} already up to date",
                " · {} เป็นปัจจุบันอยู่แล้ว",
                current
            ));
        }
        if skipped > 0 {
            message.push_str(&tf!(" · {} missing, skipped", " · ข้าม {} ที่หาไม่พบ", skipped));
        }
        if self.settings.git_warning && !unignored.is_empty() {
            message.push_str(&tf!(
                "\nGit does not ignore .env in {}.",
                "\nGit ไม่ได้ละเว้น .env ใน {}",
                unignored.join(", ")
            ));
        }
        if !errors.is_empty() {
            message.push_str(&format!("\n{}", errors.join("\n")));
        }
        if errors.is_empty() && (unignored.is_empty() || !self.settings.git_warning) {
            self.ok(message);
        } else {
            self.fail(message);
        }
    }

    /// Brings a just-tied folder in line with the project: writes .env when there is none, and
    /// opens the conflict resolver when its .env disagrees with the master file.
    pub fn sync_folder(&mut self, folder: &Path) {
        let (Some(project), Some(Ok(vars))) = (self.selected.clone(), self.variables().cloned())
        else {
            return;
        };
        let shown = display_path(folder);
        match core::folder_status(&vars, folder) {
            FolderStatus::NoEnv => match core::inject(&vars, folder) {
                Ok(_) => self.ok(tf!(
                    "Tied {} and wrote its .env · {}",
                    "ผูก {} และเขียน .env แล้ว · {}",
                    shown,
                    count(vars.len(), "variable", "variables", "ตัวแปร")
                )),
                Err(error) => self.fail(error),
            },
            FolderStatus::InSync => self.ok(tf!(
                "Tied {} · its .env already matches {}",
                "ผูก {} แล้ว · .env ตรงกับ {} อยู่แล้ว",
                shown,
                project
            )),
            FolderStatus::Differs(_) => self.open_resolver(folder),
            FolderStatus::Missing => self.fail(tf!("{} does not exist", "ไม่พบ {}", shown)),
            FolderStatus::Unreadable => self.fail(tf!(
                "Tied {}, but its .env cannot be read. Fix it, then resolve.",
                "ผูก {} แล้ว แต่อ่าน .env ไม่ได้ แก้ไขไฟล์แล้วค่อยแก้ความต่าง",
                shown
            )),
        }
    }

    /// Opens the conflict resolver for the active folder.
    pub fn resolve_active(&mut self) {
        if let Some(folder) = self.folder() {
            self.open_resolver(&folder);
        }
    }

    /// Opens the conflict resolver for a folder whose .env differs from the master file.
    pub fn open_resolver(&mut self, folder: &Path) {
        let Some(Ok(vars)) = self.variables() else {
            return;
        };
        match core::conflicts(vars, folder) {
            Ok(found) if found.is_empty() => self.ok(t("Already in sync", "ตรงกันอยู่แล้ว")),
            Ok(found) => {
                let items = found
                    .into_iter()
                    .map(|conflict| {
                        let start = conflict
                            .env
                            .clone()
                            .or_else(|| conflict.master.clone())
                            .unwrap_or_default();
                        ResolveItem {
                            conflict,
                            pick: Pick::Master,
                            edit: TextEdit::new(start),
                        }
                    })
                    .collect();
                self.resolver = Some(Resolver {
                    folder: folder.to_path_buf(),
                    items,
                    reveal: false,
                });
                self.dialog = Some(Dialog::Resolve);
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn pick_all(&mut self, pick: Pick) {
        if let Some(resolver) = &mut self.resolver {
            for item in &mut resolver.items {
                item.pick = pick;
            }
        }
    }

    /// Writes the chosen values into both the master file and the folder's .env.
    pub fn apply_resolution(&mut self) {
        let (Some(project), Some(resolver)) = (self.selected.clone(), self.resolver.take()) else {
            return;
        };
        let resolved: Vec<(String, Option<String>)> = resolver
            .items
            .iter()
            .map(|item| {
                let value = match item.pick {
                    Pick::Master => item.conflict.master.clone(),
                    Pick::Env => item.conflict.env.clone(),
                    Pick::Edit => Some(item.edit.text.clone()),
                };
                (item.conflict.name.clone(), value)
            })
            .collect();
        match self
            .home()
            .and_then(|home| core::resolve(&home, &project, &resolver.folder, &resolved))
        {
            Ok(_) => {
                self.dialog = None;
                self.panel = Panel::None;
                self.reload();
                self.select(Some(project.clone()));
                self.ok(tf!(
                    "Synced {} with {} · {} resolved",
                    "ซิงก์ {} กับ {} แล้ว · แก้ไข {}",
                    display_path(&resolver.folder),
                    project,
                    count(resolved.len(), "variable", "variables", "ตัวแปร")
                ));
            }
            Err(error) => {
                // Keep the dialog open so nothing typed is lost.
                self.resolver = Some(resolver);
                self.fail(error);
            }
        }
    }

    pub fn resolver_edit(&mut self, i: usize) -> &mut TextEdit {
        if self.resolver.as_ref().is_some_and(|r| i < r.items.len()) {
            &mut self.resolver.as_mut().expect("checked above").items[i].edit
        } else {
            &mut self.scratch
        }
    }

    /// `keyman diff`, shown in a modal.
    pub fn diff(&mut self) {
        let (Some(folder), Some(Ok(variables))) = (self.folder(), self.variables()) else {
            return;
        };
        match core::diff(variables, &folder) {
            Ok(lines) => {
                self.panel = Panel::Diff(lines);
                self.dialog = Some(Dialog::Panel);
            }
            Err(error) => self.fail(error),
        }
    }

    /// `keyman save --dry-run`, shown in a modal.
    pub fn preview_save(&mut self) {
        let (Some(project), Some(folder)) = (self.selected.clone(), self.folder()) else {
            return;
        };
        match self
            .home()
            .and_then(|home| core::save_preview(&home, &project, &folder, self.replace))
        {
            Ok(preview) => {
                self.panel = Panel::Save(preview);
                self.dialog = Some(Dialog::Panel);
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn apply_save(&mut self) {
        let (Some(project), Some(folder)) = (self.selected.clone(), self.folder()) else {
            return;
        };
        let result = self
            .home()
            .and_then(|home| core::save_apply(&home, &project, &folder, self.replace));
        self.close_panel();
        self.reload();
        self.select(Some(project.clone()));
        match result {
            Ok(0) => self.ok(tf!(
                "{} is already up to date with .env",
                "{} ตรงกับ .env อยู่แล้ว",
                project
            )),
            Ok(n) => self.ok(tf!(
                "Saved {} to {}",
                "บันทึก {} ลง {} แล้ว",
                count(n, "change", "changes", "รายการ"),
                project
            )),
            Err(error) => self.fail(error),
        }
    }

    pub fn edit_master(&mut self) {
        let Ok(master) = &self.master else { return };
        match core::open_in_editor(&master.path) {
            Ok(()) => self.ok(t(
                "Opened the master file. Reload after you save it.",
                "เปิดไฟล์หลักแล้ว กดโหลดใหม่หลังบันทึก",
            )),
            Err(error) => self.fail(error),
        }
    }

    pub fn init_master(&mut self) {
        match self.home().and_then(|home| core::init(&home)) {
            Ok(()) => {
                self.reload();
                self.ok(t(
                    "Created ~/.keyman.yml with an example project",
                    "สร้าง ~/.keyman.yml พร้อมโปรเจกต์ตัวอย่างแล้ว",
                ));
            }
            Err(error) => self.fail(error),
        }
    }
}

// ── Dialog actions that need a Tx ───────────────────────────────────────────

pub fn load_dialog(_: &mut App, tx: &mut Tx) {
    let options = FileDialogOptions {
        title: t("Choose a master file", "เลือกไฟล์หลัก").into(),
        ..Default::default()
    }
    .filter("YAML", &["yml", "yaml"]);
    tx.open_file(options, |s: &mut App, result, _| match result {
        Ok(Some(path)) => match s.home().and_then(|home| core::load(&path, &home)) {
            Ok(()) => {
                s.reload();
                s.ok(tf!("Loaded {}", "โหลด {} แล้ว", display_path(&path)));
            }
            Err(error) => s.fail(error),
        },
        Ok(None) => {}
        Err(error) => s.fail(error.to_string()),
    });
}

pub fn new_project_dialog(_: &mut App, tx: &mut Tx) {
    let options = FileDialogOptions {
        title: t(
            "Choose a project folder with a .env file",
            "เลือกโฟลเดอร์โปรเจกต์ที่มีไฟล์ .env",
        )
        .into(),
        ..Default::default()
    };
    tx.open_folder(options, |s: &mut App, result, _| match result {
        Ok(Some(folder)) => {
            let Some(name) = core::folder_name(&folder) else {
                return s.fail(t("that folder has no name", "โฟลเดอร์นี้ไม่มีชื่อ"));
            };
            s.select(Some(name.clone()));
            s.tie(&name, &folder);
            s.preview_save();
        }
        Ok(None) => {}
        Err(error) => s.fail(error.to_string()),
    });
}

pub fn tie_folder_dialog(s: &mut App, tx: &mut Tx) {
    let Some(project) = s.selected.clone() else {
        return;
    };
    let options = FileDialogOptions {
        title: tf!("Tie a folder to {}", "ผูกโฟลเดอร์กับ {}", project),
        ..Default::default()
    };
    tx.open_folder(options, move |s: &mut App, result, _| match result {
        Ok(Some(folder)) => {
            s.tie(&project, &folder);
            s.sync_folder(&folder);
        }
        Ok(None) => {}
        Err(error) => s.fail(error.to_string()),
    });
}

pub fn open_link(s: &mut App, tx: &mut Tx, url: &str) {
    if let Err(error) = tx.open_url(url) {
        s.fail(tf!("cannot open the link: {:?}", "เปิดลิงก์ไม่ได้: {:?}", error));
    }
}

/// App-wide shortcuts. ⌘ on macOS, Ctrl on Windows.
fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape && s.menu.is_some() {
        s.menu = None;
        return true;
    }
    if ev.key == Key::Escape && s.dialog.is_some() {
        s.dialog = None;
        s.panel = Panel::None;
        return true;
    }
    if ev.key == Key::Function(11) || (ev.mods.ctrl && ev.mods.cmd && ev.key == Key::Command('f')) {
        tx.toggle_full_screen();
        return true;
    }
    if ev.key == Key::Function(1) {
        s.dialog = Some(Dialog::Help(HelpPage::Start));
        return true;
    }
    let Key::Command(c) = ev.key else {
        return false;
    };
    match c {
        ',' => s.dialog = Some(Dialog::Settings(SettingsPage::General)),
        '/' => s.dialog = Some(Dialog::Help(HelpPage::Shortcuts)),
        'r' => s.reload_and_report(),
        'e' => s.edit_master(),
        'o' => load_dialog(s, tx),
        'n' => new_project_dialog(s, tx),
        'f' => tx.focus("search", true),
        _ => return false,
    }
    true
}

/// The Dock icon on macOS (also when run with `cargo run`, outside a bundle), and the title bar,
/// taskbar and Alt+Tab icon on Windows (the resource build.rs embeds).
fn set_icon() {
    #[cfg(windows)]
    rune::set_app_icon_resource(1);
    #[cfg(not(windows))]
    if let Err(error) = rune::set_app_icon(include_bytes!("../assets/icon-macos.png").as_slice()) {
        eprintln!("keyman: {error}");
    }
}

fn thai(s: &mut App) {
    s.settings.language = i18n::Language::Thai;
    i18n::set(i18n::Language::Thai);
}

fn main() {
    let app = RuneApp::new(AppSpec::<App> {
        min_size: Some((820.0, 560.0)),
        on_key: Some(on_key),
        // Headless states for `--snap shot.png --scene <name>`. `th-*` scenes are in Thai.
        scenes: &[
            ("revealed", |s| s.reveal_all = true),
            ("diff", |s| s.diff()),
            ("save", |s| s.preview_save()),
            ("menu", |s| {
                if let Some(folder) = s.folder() {
                    s.menu = Some(FolderMenu {
                        folder,
                        at: MenuAt::Point(760.0, 470.0),
                    });
                }
            }),
            ("settings", |s| {
                s.dialog = Some(Dialog::Settings(SettingsPage::General))
            }),
            ("appearance", |s| {
                s.dialog = Some(Dialog::Settings(SettingsPage::Appearance))
            }),
            ("privacy", |s| {
                s.dialog = Some(Dialog::Settings(SettingsPage::Privacy))
            }),
            ("about", |s| {
                s.dialog = Some(Dialog::Settings(SettingsPage::About))
            }),
            ("help", |s| s.dialog = Some(Dialog::Help(HelpPage::Start))),
            ("shortcuts", |s| {
                s.dialog = Some(Dialog::Help(HelpPage::Shortcuts))
            }),
            ("commands", |s| {
                s.dialog = Some(Dialog::Help(HelpPage::Commands))
            }),
            ("confirm", |s| s.dialog = Some(Dialog::ConfirmOverwrite)),
            ("confirm-all", |s| s.dialog = Some(Dialog::ConfirmWriteAll)),
            ("resolve", |s| s.resolve_active()),
            ("resolve-edit", |s| {
                s.resolve_active();
                if let Some(r) = &mut s.resolver {
                    r.reveal = true;
                    r.items[0].pick = Pick::Env;
                    r.items[1].pick = Pick::Edit;
                }
            }),
            ("toast", |s| {
                s.show_toast(
                    true,
                    "Updated ~/code/my-app/.env · 3 variables from my-app".into(),
                );
            }),
            ("light", |s| s.settings.theme = settings::Theme::Light),
            ("th", thai),
            ("th-menu", |s| {
                thai(s);
                if let Some(folder) = s.folder() {
                    s.menu = Some(FolderMenu {
                        folder,
                        at: MenuAt::Point(760.0, 470.0),
                    });
                }
            }),
            ("th-diff", |s| {
                thai(s);
                s.diff();
            }),
            ("th-resolve", |s| {
                thai(s);
                s.resolve_active();
            }),
            ("th-settings", |s| {
                thai(s);
                s.dialog = Some(Dialog::Settings(SettingsPage::Appearance));
            }),
            ("th-help", |s| {
                thai(s);
                s.dialog = Some(Dialog::Help(HelpPage::Start));
            }),
        ],
        ..AppSpec::new(
            "Keyman",
            (1100.0, 740.0),
            || App::new(None, Prefs::memory()),
            views::view,
        )
    });
    set_icon();
    let handle = app.handle();
    app.titlebar(Titlebar::new(widgets::TITLE_H))
        .init(move || App::new(Some(handle), Prefs::open(APP_ID)))
        .run();
}
