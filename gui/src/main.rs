//! Keyman for the desktop: the `keyman` commands as a window. Pick a project,
//! tie it to its folders, then write, compare, or save its .env files.
//! Every string is in English and Thai (`i18n`).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod cloud;
mod cloud_views;
mod core;
mod diagram;
mod icons;
mod menu;
mod resolve;
mod settings;
mod ties;
mod views;
mod widgets;

use widgets::Pane;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use keyman_cli::cloud::sync::{self, LinkStatus};
use keyman_cli::envfile::Change;
use keyman_cli::ui::display_path;
use rune::prefs::Prefs;
use rune::{
    App as RuneApp, AppHandle, AppSpec, FileDialogOptions, Key, KeyEvent, TextEdit, Titlebar, Tx,
};

use crate::cloud::{Ask, Attention, Busy, CloudState, Question};
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
    Account,
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
    ConfirmSignOut,
    /// Where to keep a project in the cloud (`App::link_picker`).
    LinkPicker,
    /// Type the project name to delete it from the cloud (`App::delete_confirm`).
    ConfirmDelete,
    /// What a **Sync** found that needs the person (`App::sync_results`).
    SyncResults,
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

/// The link picker: where to keep one project in the cloud.
pub struct LinkPicker {
    pub project: String,
    /// The person's groups, `(vault id, name)`. `None` while they load.
    pub groups: Option<Vec<(String, String)>>,
    /// The chosen group's name. `None` is the personal vault.
    pub choice: Option<String>,
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
    /// The stage shown for each project with stages (this session only).
    pub view_stage: BTreeMap<String, String>,
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
    /// The fields of the question dialogs; empty again after each answer.
    pub cloud_fields: cloud::Fields,
    /// The Account page asks GitHub or Google for **Use recovery code**.
    pub recover_pick: bool,
    /// This computer has a credential store (read once at start).
    pub cloud_store: bool,
    /// Each linked project and how it compares with its last sync. Local data only.
    pub link_status: Vec<(String, LinkStatus)>,
    pub link_picker: Option<LinkPicker>,
    /// The project whose cloud delete waits for its typed name.
    pub delete_confirm: Option<String>,
    pub delete_typed: TextEdit,
    /// The project page's ⋯ menu.
    pub project_menu: Option<MenuAt>,
    /// The **Sync results** dialog's content.
    pub sync_results: Option<cloud::SyncSummary>,
    /// The choices of the open cloud conflict question; wiped after the answer.
    pub cloud_picks: Option<cloud::CloudPicks>,
}

impl App {
    fn new(handle: Option<AppHandle<App>>, prefs: Prefs) -> App {
        let home = keyman_cli::master::home_dir().ok();
        let cloud = home
            .as_deref()
            .map_or(CloudState::SignedOut, cloud::initial_state);
        let mut app = App::build(handle, prefs, home, cloud);
        app.cloud_store = cloud::store_available();
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
            view_stage: BTreeMap::new(),
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
            cloud_fields: cloud::Fields::default(),
            recover_pick: false,
            cloud_store: true,
            link_status: Vec::new(),
            link_picker: None,
            delete_confirm: None,
            delete_typed: TextEdit::new(""),
            project_menu: None,
            sync_results: None,
            cloud_picks: None,
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
        self.refresh_link_status();
    }

    /// Reads which projects are linked to the cloud, and whether each has local changes.
    /// Local data only: no network.
    pub fn refresh_link_status(&mut self) {
        self.link_status = self
            .home()
            .and_then(|home| sync::local_status(&home))
            .unwrap_or_default();
    }

    /// How `project` compares with the cloud copy, or `None` when it is not linked.
    pub fn link_of(&self, project: &str) -> Option<&LinkStatus> {
        self.link_status
            .iter()
            .find(|(name, _)| name == project)
            .map(|(_, status)| status)
            .filter(|status| **status != LinkStatus::Missing)
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
            self.project_menu = None;
            if self.settings.rehide_on_switch {
                self.revealed.clear();
                self.reveal_all = false;
            }
        }
    }

    fn project(&self) -> Option<&core::Project> {
        let name = self.selected.as_ref()?;
        self.master.as_ref().ok()?.project(name)
    }

    /// The selected project's stages; empty when it has none.
    pub fn stages(&self) -> Vec<String> {
        self.project()
            .map(core::Project::stages)
            .unwrap_or_default()
    }

    /// The stage on show: the one picked, else the active folder's, else the first.
    /// `None` for a project without stages.
    pub fn stage(&self) -> Option<String> {
        let stages = self.stages();
        let name = self.selected.as_ref()?;
        let valid = |stage: &&str| stages.iter().any(|s| s == stage);
        self.view_stage
            .get(name)
            .map(String::as_str)
            .filter(valid)
            .or_else(|| {
                let folder = self.ties.active(name)?;
                self.ties.stage(name, folder).filter(valid)
            })
            .or(stages.first().map(String::as_str))
            .map(str::to_owned)
    }

    /// The stage a tied folder writes from. `None` for a project without stages, and
    /// for a folder whose stage was never picked or no longer exists: such a folder is
    /// not written until a stage is picked for it, as the CLI asks for one too.
    pub fn folder_stage(&self, folder: &Path) -> Option<String> {
        let stages = self.stages();
        let name = self.selected.as_ref()?;
        self.ties
            .stage(name, &folder.to_string_lossy())
            .filter(|stage| stages.iter().any(|s| s == stage))
            .map(str::to_owned)
    }

    /// True when the project has stages but `folder` has none picked.
    pub fn needs_stage(&self, folder: &Path) -> bool {
        !self.stages().is_empty() && self.folder_stage(folder).is_none()
    }

    /// How a tied folder's .env stands against what it is written from.
    pub fn folder_status(&self, folder: &Path) -> FolderStatus {
        if !folder.is_dir() {
            return FolderStatus::Missing;
        }
        if self.needs_stage(folder) {
            return FolderStatus::NoStage;
        }
        match self.folder_variables(folder) {
            Some(Ok(vars)) => core::folder_status(&vars, folder),
            _ => FolderStatus::Unreadable,
        }
    }

    /// Says how to pick a stage for `folder`.
    fn fail_no_stage(&mut self, folder: &Path) {
        self.fail(tf!(
            "Pick a stage for {} first: right-click it › Write from stage",
            "เลือกสเตจให้ {} ก่อน: คลิกขวา › เขียนจากสเตจ",
            display_path(folder)
        ));
    }

    /// `my-app` or `my-app:staging`, for messages.
    pub fn target_label(&self, stage: Option<&str>) -> String {
        let name = self.selected.clone().unwrap_or_default();
        match stage {
            Some(stage) => format!("{name}{}{stage}", keyman_cli::master::STAGE_SEPARATOR),
            None => name,
        }
    }

    /// The variables of the stage on show (all of them for a project without stages).
    pub fn variables(&self) -> Option<Result<Vec<(String, String)>, String>> {
        Some(self.project()?.variables(self.stage().as_deref()))
    }

    /// The variables a tied folder's .env is written from; `None` when there is no
    /// project, or the folder needs a stage first.
    pub fn folder_variables(&self, folder: &Path) -> Option<Result<Vec<(String, String)>, String>> {
        if self.needs_stage(folder) {
            return None;
        }
        Some(
            self.project()?
                .variables(self.folder_stage(folder).as_deref()),
        )
    }

    /// The names the stage on show sets itself (its own or overridden values).
    pub fn stage_own(&self) -> Vec<String> {
        let (Some(project), Some(stage)) = (self.project(), self.stage()) else {
            return Vec::new();
        };
        project
            .data
            .as_ref()
            .ok()
            .and_then(|data| data.stage(&stage))
            .map(|vars| vars.iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn show_stage(&mut self, stage: &str) {
        if let Some(name) = self.selected.clone() {
            self.view_stage.insert(name, stage.to_owned());
        }
    }

    /// Writes `folder`'s .env from `stage` from now on.
    pub fn set_folder_stage(&mut self, folder: &Path, stage: &str) {
        let Some(project) = self.selected.clone() else {
            return;
        };
        self.menu = None;
        self.ties
            .set_stage(&project, &folder.to_string_lossy(), stage);
        self.view_stage.insert(project.clone(), stage.to_owned());
        self.store_ties();
        self.ok(tf!(
            "{} now uses {} · Write .env to apply it",
            "{} ใช้ {} แล้ว · กด เขียน .env เพื่อใช้ค่าใหม่",
            display_path(folder),
            self.target_label(Some(stage))
        ));
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
        let stage = (self.selected.as_deref() == Some(project))
            .then(|| self.stage())
            .flatten();
        self.ties.tie(project, &folder.to_string_lossy());
        if let Some(stage) = stage {
            self.ties
                .set_stage(project, &folder.to_string_lossy(), &stage);
        }
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
        // The page follows the folder's stage.
        if let Some(stage) = self.folder_stage(folder) {
            self.view_stage.insert(project.clone(), stage);
        }
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

    /// A resizable section's width in points (the split as the Variables column's width).
    pub fn pane_width(&self, pane: Pane) -> f32 {
        match pane {
            Pane::Sidebar => self.settings.sidebar_w,
            Pane::Stages => self.settings.stages_w,
            Pane::Split(total) => settings::split_width(self.settings.split, total),
        }
    }

    /// Sets a section's width, kept within its limits. Saved by `store_settings`.
    pub fn set_pane_width(&mut self, pane: Pane, width: f32) {
        let clamp = |value: f32, (_, min, max): (f32, f32, f32)| value.clamp(min, max);
        match pane {
            Pane::Sidebar => self.settings.sidebar_w = clamp(width, settings::SIDEBAR_W),
            Pane::Stages => self.settings.stages_w = clamp(width, settings::STAGES_W),
            Pane::Split(total) if total > 0.0 => {
                let width = settings::split_width(width / total, total);
                self.settings.split = clamp(width / total, settings::SPLIT);
            }
            Pane::Split(_) => {}
        }
    }

    pub fn reset_pane_width(&mut self, pane: Pane) {
        match pane {
            Pane::Sidebar => self.settings.sidebar_w = settings::SIDEBAR_W.0,
            Pane::Stages => self.settings.stages_w = settings::STAGES_W.0,
            Pane::Split(_) => self.settings.split = settings::SPLIT.0,
        }
    }

    /// Saves the settings as they are (after a resize).
    pub fn store_settings(&mut self) {
        if let Err(error) = self.settings.save(&mut self.prefs) {
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

    /// Closes the open dialog (the close button, Escape, a click outside).
    pub fn close_dialog(&mut self) {
        self.dialog = None;
        self.panel = Panel::None;
        // A group list that still loads stops with its picker.
        if self
            .link_picker
            .as_ref()
            .is_some_and(|picker| picker.groups.is_none())
        {
            self.cloud_cancel();
        }
        self.link_picker = None;
        self.delete_confirm = None;
        self.delete_typed.replace_text("");
        self.sync_results = None;
        // The Account page opens on its sign-in buttons next time.
        self.recover_pick = false;
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
        let Some(folder) = self.folder() else { return };
        if self.needs_stage(&folder) {
            return self.fail_no_stage(&folder);
        }
        let Some(Ok(variables)) = self.folder_variables(&folder) else {
            return;
        };
        if self.settings.confirm_overwrite && core::env_would_change(&variables, &folder) {
            self.dialog = Some(Dialog::ConfirmOverwrite);
        } else {
            self.inject_now();
        }
    }

    pub fn inject_now(&mut self) {
        self.dialog = None;
        let Some(folder) = self.folder() else { return };
        let Some(Ok(variables)) = self.folder_variables(&folder) else {
            return;
        };
        let project = self.target_label(self.folder_stage(&folder).as_deref());
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
        let would_change = self.tied().iter().any(|folder| {
            matches!(self.folder_variables(folder), Some(Ok(variables)) if core::env_would_change(&variables, folder))
        });
        if self.settings.confirm_overwrite && would_change {
            self.dialog = Some(Dialog::ConfirmWriteAll);
        } else {
            self.write_all_now();
        }
    }

    pub fn write_all_now(&mut self) {
        self.dialog = None;
        let (mut written, mut current, mut skipped, mut unignored) = (0, 0, 0, Vec::new());
        let (mut errors, mut no_stage) = (Vec::new(), 0);
        for folder in self.tied() {
            if !folder.is_dir() {
                skipped += 1;
                continue;
            }
            if self.needs_stage(&folder) {
                no_stage += 1;
                continue;
            }
            let variables = match self.folder_variables(&folder) {
                Some(Ok(variables)) => variables,
                Some(Err(error)) => {
                    errors.push(error);
                    continue;
                }
                None => continue,
            };
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
        if no_stage > 0 {
            message.push_str(&tf!(
                " · {} without a stage, skipped (right-click › Write from stage)",
                " · ข้าม {} ที่ยังไม่เลือกสเตจ (คลิกขวา › เขียนจากสเตจ)",
                count(no_stage, "folder", "folders", "โฟลเดอร์")
            ));
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
        if errors.is_empty()
            && no_stage == 0
            && (unignored.is_empty() || !self.settings.git_warning)
        {
            self.ok(message);
        } else {
            self.fail(message);
        }
    }

    /// Brings a just-tied folder in line with the project: writes .env when there is none, and
    /// opens the conflict resolver when its .env disagrees with the master file.
    pub fn sync_folder(&mut self, folder: &Path) {
        let project = self.target_label(self.folder_stage(folder).as_deref());
        let shown = display_path(folder);
        let vars = match self.folder_variables(folder) {
            Some(Ok(vars)) => vars,
            _ => Vec::new(),
        };
        match self.folder_status(folder) {
            FolderStatus::NoEnv => match core::inject(&vars, folder) {
                Ok(_) => self.ok(tf!(
                    "Tied {} and wrote its .env from {} · {}",
                    "ผูก {} และเขียน .env จาก {} แล้ว · {}",
                    shown,
                    project,
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
            FolderStatus::NoStage => self.fail_no_stage(folder),
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
        // A cloud task may change the master file: the dialog would show old values.
        if !cloud::can_start(&self.busy) {
            self.fail(cloud::folder_waits_text());
            return;
        }
        let Some(Ok(vars)) = self.folder_variables(folder) else {
            return;
        };
        match core::conflicts(&vars, folder) {
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
        // A cloud task may change the master file: the dialog's master values may be old.
        if !cloud::can_start(&self.busy) {
            self.fail(cloud::folder_waits_text());
            return;
        }
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
        let stage = self.folder_stage(&resolver.folder);
        match self.home().and_then(|home| {
            core::resolve(
                &home,
                &project,
                stage.as_deref(),
                &resolver.folder,
                &resolved,
            )
        }) {
            Ok(_) => {
                self.dialog = None;
                self.panel = Panel::None;
                self.reload();
                self.select(Some(project.clone()));
                self.ok(tf!(
                    "Synced {} with {} · {} resolved",
                    "ซิงก์ {} กับ {} แล้ว · แก้ไข {}",
                    display_path(&resolver.folder),
                    self.target_label(stage.as_deref()),
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
        let Some(folder) = self.folder() else { return };
        if self.needs_stage(&folder) {
            return self.fail_no_stage(&folder);
        }
        let Some(Ok(variables)) = self.folder_variables(&folder) else {
            return;
        };
        match core::diff(&variables, &folder) {
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
        if self.needs_stage(&folder) {
            return self.fail_no_stage(&folder);
        }
        let stage = self.folder_stage(&folder);
        match self.home().and_then(|home| {
            core::save_preview(&home, &project, stage.as_deref(), &folder, self.replace)
        }) {
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
        if self.needs_stage(&folder) {
            return self.fail_no_stage(&folder);
        }
        let stage = self.folder_stage(&folder);
        let result = self.home().and_then(|home| {
            core::save_apply(&home, &project, stage.as_deref(), &folder, self.replace)
        });
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
    // A cloud question is above every dialog: Escape cancels it. For a cloud conflict, that
    // is the dialog's Cancel (nothing changes for that project); else it stops the task.
    if ev.key == Key::Escape && s.question.is_some() {
        if matches!(
            s.question.as_ref().map(|q| &q.ask),
            Some(Ask::Resolve { .. })
        ) {
            s.cloud_resolve_cancel();
        } else {
            s.cloud_cancel();
        }
        return true;
    }
    if ev.key == Key::Escape && s.project_menu.is_some() {
        s.project_menu = None;
        return true;
    }
    if ev.key == Key::Escape && s.menu.is_some() {
        s.menu = None;
        return true;
    }
    if ev.key == Key::Escape && s.dialog.is_some() {
        s.close_dialog();
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

/// A sample recovery code for the scenes (not a real one).
const SCENE_CODE: &str = "7K2M-QD9T-4XWB-HN3R-V8JC-P6YF-1EAZ-G5SL";

/// English, whatever the system language: the cloud scenes check the English text.
fn english(s: &mut App) {
    s.settings.language = i18n::Language::English;
    i18n::set(i18n::Language::English);
}

/// The Account page, signed out, with no task.
fn cloud_signed_out(s: &mut App) {
    s.cloud = CloudState::SignedOut;
    s.dialog = Some(Dialog::Settings(SettingsPage::Account));
}

/// A cloud question over the Account page, while a sign-in runs.
fn cloud_question(s: &mut App, ask: Ask) {
    cloud_signed_out(s);
    s.busy = Some(Busy::new(cloud::signing_in_label()));
    s.question = Some(Question::unanswered(ask));
}

/// A signed-in app whose selected project is linked (`status`), and the next project has
/// the other state, so the sidebar shows both marks.
fn cloud_project(s: &mut App, status: Option<LinkStatus>) {
    s.cloud = CloudState::SignedIn {
        email: "dana@example.com".into(),
    };
    s.link_status.clear();
    let names: Vec<String> = match &s.master {
        Ok(master) => master.projects.iter().map(|p| p.name.clone()).collect(),
        Err(_) => Vec::new(),
    };
    if let (Some(status), Some(name)) = (status, s.selected.clone()) {
        s.link_status.push((name, status));
    }
    // Another project in the other state, for the sidebar marks.
    if let Some(other) = names.iter().find(|n| Some(*n) != s.selected.as_ref()) {
        let status = match s.selected.as_ref().and_then(|n| s.link_of(n)) {
            Some(LinkStatus::Synced) => LinkStatus::LocalChanges,
            _ => LinkStatus::Synced,
        };
        s.link_status.push((other.clone(), status));
    }
}

/// Signed in, for the cloud scenes.
fn signed_in(s: &mut App) {
    s.cloud = CloudState::SignedIn {
        email: "dana@example.com".into(),
    };
}

/// The **Sync results** dialog with one project of each kind.
fn sync_results(s: &mut App) {
    signed_in(s);
    s.sync_results = Some(cloud::SyncSummary {
        updated: 1,
        added: vec!["billing-api".into()],
        sent: 1,
        attention: vec![
            Attention::Stopped {
                project: "my-app".into(),
                conflicts: 2,
            },
            Attention::Skipped {
                project: "web".into(),
                reason: "The master file changed while Keyman worked. Keyman did not write anything. Run 'keyman pull' again.".into(),
            },
            Attention::CloudIsNewer("worker".into()),
            Attention::CanMerge("payments".into()),
        ],
        given: vec![("Backend team".into(), "sam@example.com".into())],
        warnings: vec!["Keyman cannot give lee@example.com access to Design now.".into()],
        not_sent: false,
        nothing_linked: false,
    });
    s.dialog = Some(Dialog::SyncResults);
}

/// A cloud conflict during **Sync**: the second keeps the cloud value, the third is typed.
fn cloud_resolve(s: &mut App) {
    use keyman_cli::cloud::merge::Conflict as CloudConflict;
    signed_in(s);
    s.busy = Some(Busy::new(cloud::syncing_label()));
    let conflict = |name: &str, local: Option<&str>, cloud: Option<&str>| CloudConflict {
        name: name.into(),
        local: local.map(Into::into),
        cloud: cloud.map(Into::into),
    };
    let ask = Ask::Resolve {
        project: "my-app".into(),
        conflicts: vec![
            conflict(
                "DATABASE_URL",
                Some("postgres://localhost/app"),
                Some("postgres://db.internal/app"),
            ),
            conflict("API_KEY", None, Some("sk-live-2f9c")),
            conflict("DEBUG", Some("true"), Some("false")),
        ],
    };
    cloud::post(s, Question::unanswered(ask));
    if let Some(picks) = &mut s.cloud_picks {
        picks.items[1].pick = Pick::Env;
        picks.items[2].pick = Pick::Edit;
    }
}

/// The link picker, with its groups loaded.
fn link_picker(s: &mut App) {
    cloud_project(s, None);
    s.link_picker = Some(LinkPicker {
        project: s.selected.clone().unwrap_or_default(),
        groups: Some(vec![
            ("v1".into(), "Backend team".into()),
            ("v2".into(), "Design".into()),
        ]),
        choice: Some("Backend team".into()),
    });
    s.dialog = Some(Dialog::LinkPicker);
}

/// The weights the views use (400, 500, 600) of Inter and of Noto Sans Thai.
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/NotoSansThai-Regular.ttf"),
    include_bytes!("../assets/fonts/NotoSansThai-Medium.ttf"),
    include_bytes!("../assets/fonts/NotoSansThai-SemiBold.ttf"),
];

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
                    // However many conflicts the folder has: the first keeps .env, the second is typed.
                    if let Some(item) = r.items.get_mut(0) {
                        item.pick = Pick::Env;
                    }
                    if let Some(item) = r.items.get_mut(1) {
                        item.pick = Pick::Edit;
                    }
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
            ("cloud-signed-out", |s| {
                english(s);
                cloud_signed_out(s);
                // Shows the line for a computer without a credential store.
                s.cloud_store = false;
            }),
            ("cloud-signed-in", |s| {
                english(s);
                s.cloud = CloudState::SignedIn {
                    email: "dana@example.com".into(),
                };
                s.dialog = Some(Dialog::Settings(SettingsPage::Account));
            }),
            ("cloud-busy", |s| {
                english(s);
                cloud_signed_out(s);
                let mut busy = Busy::new(cloud::waiting_label());
                busy.link = Some(
                    "https://keyman.example.com/auth/v1/authorize?provider=github&redirect_to=http%3A%2F%2F127.0.0.1%3A47821%2Fcallback&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into(),
                );
                s.busy = Some(busy);
            }),
            ("cloud-first-use", |s| {
                english(s);
                s.cloud = CloudState::SignedOut;
                s.settings.cloud_prompt_done = false;
            }),
            ("cloud-new-password", |s| {
                english(s);
                cloud_question(s, Ask::NewPassword);
                s.cloud_fields.first.replace_text("short");
            }),
            ("cloud-recovery-code", |s| {
                english(s);
                cloud_question(s, Ask::ShowRecoveryCode(SCENE_CODE.to_owned().into()));
            }),
            ("cloud-password", |s| {
                english(s);
                cloud_question(s, Ask::Password { tries_left: 2 });
                s.show_toast(false, "The vault password is not correct.".into());
            }),
            ("cloud-project-linked", |s| {
                english(s);
                cloud_project(s, Some(LinkStatus::Synced));
                s.project_menu = Some(MenuAt::Point(960.0, 118.0));
            }),
            ("cloud-project-unlinked", |s| {
                english(s);
                cloud_project(s, None);
            }),
            ("cloud-link-picker", |s| {
                english(s);
                link_picker(s);
            }),
            ("cloud-link-loading", |s| {
                english(s);
                link_picker(s);
                if let Some(picker) = &mut s.link_picker {
                    picker.groups = None;
                }
                s.busy = Some(Busy::new("Loading groups…"));
            }),
            ("cloud-delete-confirm", |s| {
                english(s);
                cloud_project(s, Some(LinkStatus::Synced));
                let name = s.selected.clone().unwrap_or_default();
                s.delete_typed.replace_text(&name);
                s.delete_confirm = Some(name);
                s.dialog = Some(Dialog::ConfirmDelete);
            }),
            ("cloud-sync-busy", |s| {
                english(s);
                signed_in(s);
                s.busy = Some(Busy::new(cloud::syncing_label()));
            }),
            ("cloud-sync-results", |s| {
                english(s);
                sync_results(s);
            }),
            ("cloud-resolve", |s| {
                english(s);
                cloud_resolve(s);
            }),
            ("th-cloud-sync-results", |s| {
                thai(s);
                sync_results(s);
            }),
            ("th-cloud-resolve", |s| {
                thai(s);
                cloud_resolve(s);
            }),
            ("th-cloud-link-picker", |s| {
                thai(s);
                link_picker(s);
            }),
            ("th-cloud-signed-out", |s| {
                thai(s);
                cloud_signed_out(s);
            }),
            ("th-cloud-recovery-code", |s| {
                thai(s);
                cloud_question(s, Ask::ShowRecoveryCode(SCENE_CODE.to_owned().into()));
            }),
        ],
        // Registering our own fonts replaces Rune's built-in Inter, so Inter ships here too.
        // Latin draws in Inter; Thai, which Inter lacks, falls through to Noto Sans Thai.
        fonts: FONTS,
        families: ("Inter, Noto Sans Thai", "Inter, Noto Sans Thai"),
        ..AppSpec::new(
            "Keyman",
            (1100.0, 740.0),
            || App::new(None, Prefs::memory()),
            views::view,
        )
    });
    set_icon();
    let handle = app.handle();
    // Scenes apply on top of this state. A headless scene starts with the first-use card
    // closed, so the older scenes look as before; `cloud-first-use` opens it. Not saved.
    let scene = std::env::args().any(|arg| arg == "--scene");
    app.titlebar(Titlebar::new(widgets::TITLE_H))
        .init(move || {
            let mut app = App::new(Some(handle), Prefs::open(APP_ID));
            if scene {
                app.settings.cloud_prompt_done = true;
            }
            app
        })
        .run();
}

#[cfg(test)]
mod pane_tests {
    use super::*;

    #[test]
    fn dividers_resize_within_limits_and_reset() {
        let mut s = App::for_tests();
        s.set_pane_width(Pane::Sidebar, 9999.0);
        assert_eq!(s.settings.sidebar_w, settings::SIDEBAR_W.2);
        s.set_pane_width(Pane::Stages, 10.0);
        assert_eq!(s.settings.stages_w, settings::STAGES_W.1);
        // Variables dragged to 900 of 1000: Folders keeps its minimum.
        s.set_pane_width(Pane::Split(1000.0), 900.0);
        assert_eq!(
            s.pane_width(Pane::Split(1000.0)),
            1000.0 - settings::FOLDERS_MIN
        );
        for pane in [Pane::Sidebar, Pane::Stages, Pane::Split(1000.0)] {
            s.reset_pane_width(pane);
        }
        let d = Settings::default();
        assert_eq!(
            (s.settings.sidebar_w, s.settings.stages_w, s.settings.split),
            (d.sidebar_w, d.stages_w, d.split)
        );
    }

    /// A folder writes its own stage, whatever stage is on show, and a folder
    /// with no stage picked is never written.
    #[test]
    fn folders_write_their_own_stage_or_nothing() {
        let root = std::env::temp_dir().join(format!("keyman-app-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, prod, loose) = (root.join("home"), root.join("prod"), root.join("loose"));
        for dir in [&home, &prod, &loose] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let master = home.join("master.yml");
        std::fs::write(
            home.join(".keyman_config"),
            master.to_string_lossy().as_bytes(),
        )
        .unwrap();
        std::fs::write(
            &master,
            "app:\n  LOG: \"info\"\n  production:\n    DB: \"prod\"\n  staging:\n    DB: \"stage\"\n",
        )
        .unwrap();
        let mut s = App::build(None, Prefs::memory(), Some(home), CloudState::SignedOut);
        s.reload();
        s.select(Some("app".into()));
        s.ties.tie("app", &prod.to_string_lossy());
        s.ties
            .set_stage("app", &prod.to_string_lossy(), "production");
        s.ties.tie("app", &loose.to_string_lossy());

        // Staging on show, production folder active: Write .env writes production.
        s.show_stage("staging");
        s.ties.set_active("app", &prod.to_string_lossy());
        s.show_stage("staging");
        s.inject_now();
        let written = std::fs::read_to_string(prod.join(".env")).unwrap();
        assert!(written.contains("DB=\"prod\""), "{written}");

        // The folder without a stage is left alone, by Write .env and Write to all.
        assert_eq!(s.folder_status(&loose), FolderStatus::NoStage);
        s.set_active(&loose);
        s.inject();
        s.write_all_now();
        assert!(!loose.join(".env").exists());

        // Picking its stage makes it writable.
        s.set_folder_stage(&loose, "staging");
        assert_eq!(s.folder_status(&loose), FolderStatus::NoEnv);
        s.write_all_now();
        let written = std::fs::read_to_string(loose.join(".env")).unwrap();
        assert!(written.contains("DB=\"stage\""), "{written}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
