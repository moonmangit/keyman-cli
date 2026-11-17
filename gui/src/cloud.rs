//! Keyman Cloud in the app. A cloud action runs on a background thread and calls the same
//! library code as the CLI. When the library needs an answer (a password, the recovery code,
//! a conflict choice), the `Bridge` puts a `Question` into `App.question` and the thread waits
//! on a one-time channel for the dialog's `Reply`. The UI thread never waits.

use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use keyman_cli::cloud::account::{self, Prompt};
use keyman_cli::cloud::groups;
use keyman_cli::cloud::keystore::{self, Store, SystemStore};
use keyman_cli::cloud::merge::Conflict;
use keyman_cli::cloud::sync::{
    self, PullReport, PullResult, PushResult, Resolver as CloudResolver, Vaults,
};
use rune::{AppHandle, TextEdit};
use zeroize::{Zeroize, Zeroizing};

use crate::cloud_views;
use crate::i18n::t;
use crate::{App, Dialog, LinkPicker, Pick, ResolveItem, SettingsPage};

/// What a bridge question gives the library when the person cancels it.
pub const CANCELLED: &str = "cancelled";

/// The library asks for the vault password at most this many times in a row.
pub(crate) const PASSWORD_TRIES: u8 = 3;

/// Whether this computer has a cloud sign-in, from local data only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloudState {
    SignedOut,
    SignedIn { email: String },
}

/// The cloud task that runs now.
pub struct Busy {
    /// A short text for the screen, for example "Syncing…".
    pub label: &'static str,
    /// Set to stop the task at its next question or in the browser wait.
    pub cancel: Arc<AtomicBool>,
    /// The browser sign-in link, while the task waits for the browser. Not secret, but it is
    /// not written to a log.
    pub link: Option<String>,
}

impl Busy {
    pub fn new(label: &'static str) -> Busy {
        Busy {
            label,
            cancel: Arc::new(AtomicBool::new(false)),
            link: None,
        }
    }
}

/// The busy label while the browser sign-in waits.
pub fn waiting_label() -> &'static str {
    t("Waiting for the browser…", "กำลังรอเบราว์เซอร์…")
}

/// The busy label after the browser sign-in, while Keyman sets up or opens the vault.
pub fn signing_in_label() -> &'static str {
    t("Signing in…", "กำลังลงชื่อเข้าใช้…")
}

/// The fields of the question dialogs. They are secure fields (masked, no copy, wiped on
/// change and drop), and they are emptied when an answer is sent, when a question is dropped,
/// and when a new question comes. No password or code stays in the app after an answer.
pub struct Fields {
    pub first: TextEdit,
    pub second: TextEdit,
}

impl Default for Fields {
    fn default() -> Fields {
        Fields {
            first: TextEdit::new("").secure(),
            second: TextEdit::new("").secure(),
        }
    }
}

impl Fields {
    pub fn clear(&mut self) {
        self.first.clear();
        self.second.clear();
    }

    /// The first field's text, moved out so that no copy stays here; both fields are cleared.
    fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.first.text);
        self.clear();
        text
    }
}

/// A question from the library for the person. Not `Clone`: secrets are not copied.
#[derive(PartialEq)]
pub enum Ask {
    NewPassword,
    Password {
        tries_left: u8,
    },
    RecoveryCode,
    /// Show this code; the dialog answers with the code typed again (`Reply::Text`).
    ShowRecoveryCode(Zeroizing<String>),
    Resolve {
        project: String,
        conflicts: Vec<Conflict>,
    },
}

/// The answer from a dialog. Not `Clone`: secrets are not copied.
#[derive(PartialEq)]
pub enum Reply {
    Text(String),
    Confirmed,
    /// One choice per conflict; `None` removes the variable.
    Choices(Vec<Option<String>>),
    Cancel,
}

// Debug without the secrets: a password, a code or a value never goes to a log.
impl fmt::Debug for Ask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ask::NewPassword => write!(f, "NewPassword"),
            Ask::Password { tries_left } => write!(f, "Password {{ tries_left: {tries_left} }}"),
            Ask::RecoveryCode => write!(f, "RecoveryCode"),
            Ask::ShowRecoveryCode(_) => write!(f, "ShowRecoveryCode(..)"),
            Ask::Resolve { project, conflicts } => {
                write!(
                    f,
                    "Resolve {{ project: {project:?}, conflicts: {} }}",
                    conflicts.len()
                )
            }
        }
    }
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reply::Text(_) => write!(f, "Text(..)"),
            Reply::Confirmed => write!(f, "Confirmed"),
            Reply::Choices(choices) => write!(f, "Choices({})", choices.len()),
            Reply::Cancel => write!(f, "Cancel"),
        }
    }
}

/// A question that waits for its answer. Dropping it without an answer cancels it.
pub struct Question {
    pub ask: Ask,
    answer: mpsc::Sender<Reply>,
}

impl Question {
    pub fn answer(self, reply: Reply) {
        // The thread may have stopped already; then nobody needs the answer.
        let _ = self.answer.send(reply);
    }

    /// A question that no task waits for: for the headless scenes.
    pub fn unanswered(ask: Ask) -> Question {
        let (answer, _) = mpsc::channel();
        Question { ask, answer }
    }
}

/// Gives `ask` to `post` (which shows it) and waits for the answer. A question that is
/// dropped without an answer (the dialog closed, the window closed) gives `Reply::Cancel`.
pub fn ask_blocking(post: impl FnOnce(Question), ask: Ask) -> Reply {
    let (answer, reply) = mpsc::channel();
    post(Question { ask, answer });
    reply.recv().unwrap_or(Reply::Cancel)
}

/// Work for the UI thread, sent from a cloud task's thread.
pub type UiWork = Box<dyn FnOnce(&mut App) + Send>;

/// Where a cloud task reaches the app from its thread: the window, or a test.
pub trait Ui: Send + 'static {
    /// Runs `f` with the app on the UI thread, later.
    fn run(&self, f: UiWork);

    fn ask(&self, question: Question) {
        self.run(Box::new(move |s| post(s, question)));
    }

    fn notice(&self, text: String) {
        self.run(Box::new(move |s| s.fail(text)));
    }

    fn link(&self, url: String) {
        self.run(Box::new(move |s| show_link(s, url)));
    }
}

impl Ui for AppHandle<App> {
    fn run(&self, f: UiWork) {
        self.update(move |s, _| f(s));
    }
}

/// Shows a task's question, on the UI thread. A question that comes after Cancel, or with no
/// task running, is dropped, which answers it with `Cancel`: the thread may have read the
/// cancel flag just before the person clicked Cancel.
pub fn post(s: &mut App, question: Question) {
    if let Some(busy) = s
        .busy
        .as_mut()
        .filter(|busy| !busy.cancel.load(Ordering::SeqCst))
    {
        // A question after the browser wait: the browser part is done.
        if busy.link.take().is_some() {
            busy.label = signing_in_label();
        }
        s.cloud_fields.clear();
        s.cloud_picks = match &question.ask {
            Ask::Resolve { conflicts, .. } => Some(CloudPicks::new(conflicts)),
            _ => None,
        };
        s.question = Some(question);
    }
}

/// Shows the browser sign-in link on the Account page while the task waits for the browser.
fn show_link(s: &mut App, url: String) {
    if let Some(busy) = s
        .busy
        .as_mut()
        .filter(|busy| !busy.cancel.load(Ordering::SeqCst))
    {
        busy.link = Some(url);
        busy.label = waiting_label();
    }
}

/// Stops the running cloud task: at its next question, in the browser wait, or now when a
/// question is open. A running step (the key step, a server call) finishes first.
pub fn cancel(s: &mut App) {
    if let Some(busy) = &mut s.busy {
        busy.cancel.store(true, Ordering::SeqCst);
        busy.label = t("Cancelling…", "กำลังยกเลิก…");
    }
    // Dropped without an answer = Cancel for the waiting thread.
    s.question = None;
    s.cloud_picks = None;
}

/// The library's `Prompt` and `Resolver` for a cloud task: each question becomes a dialog.
pub struct Bridge<U: Ui = AppHandle<App>> {
    handle: U,
    cancel: Arc<AtomicBool>,
    /// How many times the task asked for the vault password.
    tries: u8,
    /// The code typed in the recovery code dialog: the answer to the next `recovery_code`.
    typed_code: Option<Zeroizing<String>>,
}

impl<U: Ui> Bridge<U> {
    pub fn new(handle: U, cancel: Arc<AtomicBool>) -> Bridge<U> {
        Bridge {
            handle,
            cancel,
            tries: 0,
            typed_code: None,
        }
    }

    /// Asks the person, unless they cancelled the task already.
    fn ask(&self, ask: Ask) -> Reply {
        if self.cancel.load(Ordering::SeqCst) {
            return Reply::Cancel;
        }
        ask_blocking(|question| self.handle.ask(question), ask)
    }

    fn text(&self, ask: Ask) -> Result<String, String> {
        match self.ask(ask) {
            Reply::Text(text) => Ok(text),
            _ => Err(CANCELLED.to_string()),
        }
    }
}

impl<U: Ui> Prompt for Bridge<U> {
    fn new_password(&mut self) -> Result<String, String> {
        self.text(Ask::NewPassword)
    }

    fn password(&mut self) -> Result<String, String> {
        let tries_left = PASSWORD_TRIES.saturating_sub(self.tries).max(1);
        self.tries = self.tries.saturating_add(1);
        self.text(Ask::Password { tries_left })
    }

    fn recovery_code(&mut self) -> Result<String, String> {
        match self.typed_code.take() {
            // Moved out; the empty `Zeroizing` left behind is dropped here.
            Some(mut typed) => Ok(std::mem::take(&mut *typed)),
            None => self.text(Ask::RecoveryCode),
        }
    }

    /// One dialog shows the code and takes it typed again; the typed text answers the
    /// library's next `recovery_code` call.
    fn show_recovery_code(&mut self, code: &str) -> Result<(), String> {
        match self.ask(Ask::ShowRecoveryCode(Zeroizing::new(code.to_owned()))) {
            Reply::Text(typed) => {
                self.typed_code = Some(Zeroizing::new(typed));
                Ok(())
            }
            Reply::Confirmed => Ok(()),
            _ => Err(CANCELLED.to_string()),
        }
    }

    fn notice(&mut self, text: &str) {
        self.handle.notice(text.to_owned());
    }

    fn sign_in_link(&mut self, url: &str) {
        self.handle.link(url.to_owned());
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

impl<U: Ui> CloudResolver for Bridge<U> {
    fn resolve(
        &mut self,
        project: &str,
        conflicts: &[Conflict],
    ) -> Result<Option<Vec<Option<String>>>, String> {
        let ask = Ask::Resolve {
            project: project.to_owned(),
            conflicts: conflicts.to_vec(),
        };
        match self.ask(ask) {
            Reply::Choices(choices) if choices.len() == conflicts.len() => Ok(Some(choices)),
            // Cancel, or an answer that does not fit: change nothing.
            _ => Ok(None),
        }
    }
}

/// What a finished cloud task asks the app to show.
pub enum Done {
    SignedIn {
        email: String,
    },
    SignedOut,
    Toast(String),
    SyncFinished(SyncSummary),
    /// The pull after **Merge** of `project`.
    Merged {
        project: String,
        summary: SyncSummary,
    },
    /// The person's groups, `(vault id, name)`, for the link picker.
    Groups(Vec<(String, String)>),
}

/// The result of **Sync** (pull, then push).
#[derive(Debug, Clone, PartialEq)]
pub struct SyncSummary {
    pub updated: usize,
    pub added: Vec<String>,
    pub sent: usize,
    pub attention: Vec<Attention>,
    /// `(group, email)` of the people who got access.
    pub given: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// A project that needs the person after a sync.
#[derive(Debug, Clone, PartialEq)]
pub enum Attention {
    Skipped { project: String, reason: String },
    Stopped { project: String, conflicts: usize },
    CloudIsNewer(String),
    CanMerge(String),
}

impl Attention {
    pub fn project(&self) -> &str {
        match self {
            Attention::Skipped { project, .. } | Attention::Stopped { project, .. } => project,
            Attention::CloudIsNewer(project) | Attention::CanMerge(project) => project,
        }
    }
}

/// The counts and the projects that need the person, from a pull and the push after it.
/// A project is listed once: a push result for a project the pull listed already is left out.
pub fn summarize(report: &PullReport, pushed: &[PushResult]) -> SyncSummary {
    let mut summary = SyncSummary {
        updated: 0,
        added: Vec::new(),
        sent: 0,
        attention: Vec::new(),
        given: report.given.clone(),
        warnings: report.warnings.clone(),
    };
    for result in &report.results {
        match result {
            PullResult::Updated { .. } => summary.updated += 1,
            PullResult::Added(project) => summary.added.push(project.clone()),
            PullResult::UpToDate(_) => {}
            PullResult::Stopped { project, conflicts } => {
                summary.attention.push(Attention::Stopped {
                    project: project.clone(),
                    conflicts: *conflicts,
                })
            }
            PullResult::Skipped { project, reason } => summary.attention.push(Attention::Skipped {
                project: project.clone(),
                reason: reason.clone(),
            }),
            PullResult::NameTaken { project } => {
                summary.attention.push(Attention::CanMerge(project.clone()))
            }
        }
    }
    for result in pushed {
        let attention = match result {
            PushResult::Pushed { .. } => {
                summary.sent += 1;
                continue;
            }
            PushResult::UpToDate(_) => continue,
            PushResult::CloudIsNewer(project) => Attention::CloudIsNewer(project.clone()),
            PushResult::Skipped { project, reason } => Attention::Skipped {
                project: project.clone(),
                reason: reason.clone(),
            },
        };
        if !summary
            .attention
            .iter()
            .any(|listed| listed.project() == attention.project())
        {
            summary.attention.push(attention);
        }
    }
    summary
}

/// Whether the **Sync results** dialog must open: a project needs the person, or a warning.
pub fn needs_person(summary: &SyncSummary) -> bool {
    !summary.attention.is_empty() || !summary.warnings.is_empty()
}

/// The toast after a sync, or `None` when the **Sync results** dialog must open.
pub fn toast_text(summary: &SyncSummary) -> Option<String> {
    toast_words(summary, crate::i18n::thai())
}

fn toast_words(summary: &SyncSummary, thai: bool) -> Option<String> {
    if needs_person(summary) {
        return None;
    }
    let mut text = counts_words(summary, thai);
    for (group, email) in &summary.given {
        text.push('\n');
        text.push_str(&given_words(group, email, thai));
    }
    Some(text)
}

/// "2 updated, 1 added, 1 sent." with the zero parts left out, or "All projects are up to date."
pub fn counts_text(summary: &SyncSummary) -> String {
    counts_words(summary, crate::i18n::thai())
}

fn counts_words(summary: &SyncSummary, thai: bool) -> String {
    let parts: Vec<String> = [
        (summary.updated, "updated", "อัปเดต"),
        (summary.added.len(), "added", "เพิ่ม"),
        (summary.sent, "sent", "ส่ง"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, en, th)| {
        if thai {
            format!("{th} {n} โปรเจกต์")
        } else {
            format!("{n} {en}")
        }
    })
    .collect();
    match (parts.is_empty(), thai) {
        (true, false) => "All projects are up to date.".to_owned(),
        (true, true) => "ทุกโปรเจกต์เป็นปัจจุบันแล้ว".to_owned(),
        (false, false) => format!("{}.", parts.join(", ")),
        (false, true) => parts.join(", "),
    }
}

/// The line for an invited person who got access to a group.
pub fn given_line(group: &str, email: &str) -> String {
    given_words(group, email, crate::i18n::thai())
}

fn given_words(group: &str, email: &str, thai: bool) -> String {
    if thai {
        format!("ให้ {email} เข้าถึง {group} แล้ว")
    } else {
        format!("Gave {email} access to {group}.")
    }
}

/// The open results after **Merge** of `project`: its row goes, and what the merge found is
/// added.
pub fn merge_into(mut results: SyncSummary, project: &str, merged: SyncSummary) -> SyncSummary {
    results
        .attention
        .retain(|listed| listed.project() != project);
    results.updated += merged.updated;
    results.sent += merged.sent;
    results.added.extend(merged.added);
    results.attention.extend(merged.attention);
    results.given.extend(merged.given);
    results.warnings.extend(merged.warnings);
    results
}

/// The text after a sign-in that the server does not accept now.
pub fn sign_in_rejected_text() -> &'static str {
    t(
        "Your sign-in is not valid now. Sign in again.",
        "การลงชื่อเข้าใช้นี้ใช้ไม่ได้แล้ว ลงชื่อเข้าใช้อีกครั้ง",
    )
}

/// A cloud task failed: back to idle, then tell the person. A sign-in that the server does
/// not accept signs the app out and opens Settings › Account. A cancel shows nothing.
pub fn after_failure(s: &mut App, error: &str) {
    s.busy = None;
    drop_question(s);
    if error == CANCELLED || error == account::SIGN_IN_CANCELLED {
        return;
    }
    if error == account::SIGN_IN_INVALID {
        s.cloud = CloudState::SignedOut;
        s.close_dialog();
        s.dialog = Some(Dialog::Settings(SettingsPage::Account));
        s.fail(sign_in_rejected_text());
    } else {
        s.fail(app_wording(error));
    }
}

/// Removes the open question (the waiting thread gets `Cancel`) and everything typed for it.
fn drop_question(s: &mut App) {
    s.question = None;
    s.cloud_fields.clear();
    s.cloud_picks = None;
}

/// The choices of the cloud conflict dialog. The values are secrets: they are wiped when the
/// picks are dropped (after the answer, a cancel, or the end of the task).
pub struct CloudPicks {
    /// `conflict.master` is the value on this computer, `conflict.env` the cloud value;
    /// `Pick::Master` keeps this computer's, `Pick::Env` the cloud's.
    pub items: Vec<ResolveItem>,
    pub reveal: bool,
}

impl CloudPicks {
    pub fn new(conflicts: &[Conflict]) -> CloudPicks {
        let items = conflicts
            .iter()
            .map(|conflict| {
                let start = conflict
                    .cloud
                    .as_deref()
                    .or(conflict.local.as_deref())
                    .unwrap_or_default();
                ResolveItem {
                    conflict: crate::core::Conflict {
                        name: conflict.name.clone(),
                        master: conflict.local.clone(),
                        env: conflict.cloud.clone(),
                    },
                    pick: Pick::Master,
                    edit: TextEdit::new(start).secure(),
                }
            })
            .collect();
        CloudPicks {
            items,
            reveal: false,
        }
    }

    /// One choice per conflict (`None` removes the variable). Typed text is moved out.
    fn choices(&mut self) -> Vec<Option<String>> {
        self.items
            .iter_mut()
            .map(|item| match item.pick {
                Pick::Master => item.conflict.master.clone(),
                Pick::Env => item.conflict.env.clone(),
                Pick::Edit => Some(std::mem::take(&mut item.edit.text)),
            })
            .collect()
    }
}

impl Drop for CloudPicks {
    fn drop(&mut self) {
        for item in &mut self.items {
            item.conflict.master.zeroize();
            item.conflict.env.zeroize();
            item.edit.clear();
        }
    }
}

/// The cloud state at app start: local data only, no network.
pub fn initial_state(home: &Path) -> CloudState {
    match account::signed_in_offline(home, &SystemStore::new()) {
        Ok(Some(email)) => CloudState::SignedIn { email },
        _ => CloudState::SignedOut,
    }
}

/// Only one cloud task runs at a time.
pub fn can_start(busy: &Option<Busy>) -> bool {
    busy.is_none()
}

/// Runs `work` on a background thread as the cloud task `label`. Returns `false`, and does
/// nothing, when a task runs already or the app has no window.
pub fn start(
    s: &mut App,
    label: &'static str,
    work: impl FnOnce(&mut Bridge, &Path) -> Result<Done, String> + Send + 'static,
) -> bool {
    let handle = s.handle.clone();
    start_with(s, handle, initial_state, label, work)
}

/// `start` with the window and the cloud state reader given, so tests need neither.
fn start_with<U: Ui + Clone>(
    s: &mut App,
    ui: Option<U>,
    refresh: fn(&Path) -> CloudState,
    label: &'static str,
    work: impl FnOnce(&mut Bridge<U>, &Path) -> Result<Done, String> + Send + 'static,
) -> bool {
    if !can_start(&s.busy) {
        return false;
    }
    let Some(handle) = ui else {
        return false;
    };
    let home = match s.home() {
        Ok(home) => home,
        Err(error) => {
            s.fail(error);
            return false;
        }
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let mut bridge = Bridge::new(handle.clone(), cancel.clone());
    let spawned = thread::Builder::new()
        .name("keyman-cloud".into())
        .spawn(move || {
            // A panic must not leave the app busy for ever.
            let result = panic::catch_unwind(AssertUnwindSafe(|| work(&mut bridge, &home)))
                .unwrap_or_else(|_| {
                    Err(t(
                        "The cloud action stopped because of an error in Keyman.",
                        "การทำงานกับคลาวด์หยุดลงเพราะเกิดข้อผิดพลาดใน Keyman",
                    )
                    .to_owned())
                });
            drop(bridge);
            // On a panic here, keep the state the app shows.
            let cloud = panic::catch_unwind(AssertUnwindSafe(|| refresh(&home))).ok();
            handle.run(Box::new(move |s| finish(s, cloud, result)));
        });
    match spawned {
        Ok(_) => {
            s.busy = Some(Busy {
                label,
                cancel,
                link: None,
            });
            true
        }
        Err(error) => {
            s.fail(error.to_string());
            false
        }
    }
}

/// The end of a cloud task, on the UI thread: back to idle, then show the result.
fn finish(s: &mut App, cloud: Option<CloudState>, result: Result<Done, String>) {
    s.busy = None;
    // Drops a question that is still open, so no dialog stays after its task.
    drop_question(s);
    if let Some(cloud) = cloud {
        s.cloud = cloud;
    }
    // The picker waits for its groups: when they did not come, it has nothing to show.
    let loaded = matches!(result, Ok(Done::Groups(_)));
    if !loaded
        && s.link_picker
            .as_ref()
            .is_some_and(|picker| picker.groups.is_none())
    {
        s.link_picker = None;
        if s.dialog == Some(Dialog::LinkPicker) {
            s.dialog = None;
        }
    }
    s.reload();
    match result {
        Ok(done) => apply(s, done),
        Err(error) => after_failure(s, &error),
    }
}

/// A library error in the app's words: the hint after the last wrong vault password names the
/// CLI command, and the app has the **Use recovery code** button instead.
pub fn app_wording(error: &str) -> String {
    match error.strip_suffix(account::RECOVER_HINT) {
        Some(head) => format!(
            "{head}{}",
            t(
                "Use 'Use recovery code' in Settings › Account.",
                "ใช้ปุ่ม \"ใช้รหัสกู้คืน\" ในการตั้งค่า › บัญชี",
            )
        ),
        None => error.to_owned(),
    }
}

fn apply(s: &mut App, done: Done) {
    match done {
        Done::SignedIn { email } => {
            // Signed in once: the first-use card does not come back after a later sign-out.
            s.update_settings(|st| st.cloud_prompt_done = true);
            s.ok(tf!("Signed in as {}.", "ลงชื่อเข้าใช้เป็น {} แล้ว", email));
            s.cloud = CloudState::SignedIn { email };
        }
        Done::SignedOut => {
            s.cloud = CloudState::SignedOut;
            s.ok(t(
                "Signed out of Keyman Cloud.",
                "ออกจากระบบ Keyman Cloud แล้ว",
            ));
        }
        Done::Toast(text) => s.ok(text),
        Done::SyncFinished(summary) => match toast_text(&summary) {
            Some(text) => s.ok(text),
            None => show_results(s, summary),
        },
        Done::Merged { project, summary } => merged(s, &project, summary),
        // The link picker was open while they loaded.
        Done::Groups(groups) => {
            if let Some(picker) = &mut s.link_picker {
                picker.groups = Some(groups);
            }
        }
    }
}

/// Opens the **Sync results** dialog.
fn show_results(s: &mut App, summary: SyncSummary) {
    s.close_dialog();
    s.sync_results = Some(summary);
    s.dialog = Some(Dialog::SyncResults);
}

/// After **Merge**: the open results lose the project's row and get what the merge found.
/// When nothing more needs the person, the dialog closes.
fn merged(s: &mut App, project: &str, mut summary: SyncSummary) {
    let open = if s.dialog == Some(Dialog::SyncResults) {
        s.sync_results.take()
    } else {
        None
    };
    // A clean merge says so in a toast, with the access it gave; else the dialog lists it.
    let clean = !needs_person(&summary);
    let given = if clean {
        std::mem::take(&mut summary.given)
    } else {
        Vec::new()
    };
    let next = match open {
        Some(results) => merge_into(results, project, summary),
        None => summary,
    };
    if needs_person(&next) {
        s.sync_results = Some(next);
        s.dialog = Some(Dialog::SyncResults);
    } else if s.dialog == Some(Dialog::SyncResults) {
        s.close_dialog();
    }
    if clean {
        let mut text = tf!(
            "Keyman merged {} with the cloud copy.",
            "Keyman รวม {} กับสำเนาบนคลาวด์แล้ว",
            project
        );
        for (group, email) in &given {
            text.push('\n');
            text.push_str(&given_line(group, email));
        }
        s.ok(text);
    }
}

/// The busy label while **Sync** runs.
pub fn syncing_label() -> &'static str {
    t("Syncing…", "กำลังซิงก์…")
}

/// The email of the sign-in that this computer saved, as the task's result.
fn signed_in_as(home: &Path, store: &dyn Store) -> Result<Done, String> {
    let email = account::signed_in_offline(home, store)?.unwrap_or_default();
    Ok(Done::SignedIn { email })
}

/// Whether this computer has a credential store. Without one, the library asks for the vault
/// password at each cloud task. A local read, no network.
pub fn store_available() -> bool {
    keystore::system_available()
}

// ── App actions ─────────────────────────────────────────────────────────────

impl App {
    /// Signs in with the browser (`provider` is "github" or "google"), then sets up the vault
    /// (a new account) or opens it (an old one).
    pub fn cloud_sign_in(&mut self, provider: &'static str) {
        self.recover_pick = false;
        start(self, waiting_label(), move |bridge, home| {
            let store = SystemStore::new();
            account::login(home, &store, provider, bridge)?;
            signed_in_as(home, &store)
        });
    }

    /// Sets a new vault password with the recovery code. With no saved sign-in, it signs in
    /// with `provider` first.
    pub fn cloud_recover(&mut self, provider: &'static str) {
        self.recover_task(Some(provider));
    }

    /// **Use recovery code** while signed in: the saved sign-in is used, so no provider is
    /// asked. If the server no longer accepts it, the task ends as a sign-in that is not valid.
    pub fn cloud_recover_signed_in(&mut self) {
        self.recover_task(None);
    }

    fn recover_task(&mut self, provider: Option<&'static str>) {
        self.recover_pick = false;
        start(self, signing_in_label(), move |bridge, home| {
            let mut provider = || provider.ok_or_else(|| account::SIGN_IN_INVALID.to_owned());
            account::recover(home, &SystemStore::new(), &mut provider, bridge)?;
            Ok(Done::Toast(
                t("The new vault password is set.", "ตั้งรหัสผ่านตู้นิรภัยใหม่แล้ว").to_owned(),
            ))
        });
    }

    pub fn cloud_change_password(&mut self) {
        let label = t("Changing the vault password…", "กำลังเปลี่ยนรหัสผ่านตู้นิรภัย…");
        start(self, label, |bridge, home| {
            account::change_password(home, &SystemStore::new(), bridge)?;
            Ok(Done::Toast(
                t("The vault password is changed.", "เปลี่ยนรหัสผ่านตู้นิรภัยแล้ว").to_owned(),
            ))
        });
    }

    /// Signs out (the Account page asks first). Links stay.
    pub fn cloud_sign_out(&mut self) {
        start(
            self,
            t("Signing out…", "กำลังออกจากระบบ…"),
            |_, home| {
                account::logout(home, &SystemStore::new())?;
                Ok(Done::SignedOut)
            },
        );
    }

    /// **Link to cloud**: opens the picker and loads the person's groups (a cloud task).
    pub fn open_link_picker(&mut self, project: &str) {
        if !can_start(&self.busy) {
            return;
        }
        self.project_menu = None;
        self.link_picker = Some(LinkPicker {
            project: project.to_owned(),
            groups: None,
            choice: None,
        });
        self.dialog = Some(Dialog::LinkPicker);
        let label = t("Loading groups…", "กำลังโหลดกลุ่ม…");
        if !start(self, label, |bridge, home| {
            let u = account::session(home, &SystemStore::new(), bridge)?;
            let vaults = Vaults::load(&u)?;
            Ok(Done::Groups(groups::group_names(&vaults)))
        }) {
            self.link_picker = None;
            self.dialog = None;
        }
    }

    /// Links `project` to the personal vault (`None`) or to the group with this name.
    pub fn cloud_link(&mut self, project: &str, group: Option<String>) {
        self.close_dialog();
        let project = project.to_owned();
        let label = t("Linking to the cloud…", "กำลังเชื่อมกับคลาวด์…");
        start(self, label, move |bridge, home| {
            let u = account::session(home, &SystemStore::new(), bridge)?;
            sync::link(home, &u, &project, group.as_deref())?;
            Ok(Done::Toast(tf!(
                "Keyman linked {} to the cloud.",
                "Keyman เชื่อม {} กับคลาวด์แล้ว",
                project
            )))
        });
    }

    /// Removes the cloud link on this computer. The cloud copy stays. Local, so no task and no
    /// session, but not while a cloud task runs: both write the same state file.
    pub fn cloud_unlink(&mut self, project: &str) {
        self.project_menu = None;
        if !cloud_views::can_unlink(&self.busy) {
            return;
        }
        let result = self.home().and_then(|home| sync::unlink(&home, project));
        self.refresh_link_status();
        match result {
            Ok(()) => self.ok(tf!(
                "Keyman removed the cloud link of {} on this computer.",
                "Keyman ลบการเชื่อมคลาวด์ของ {} บนเครื่องนี้แล้ว",
                project
            )),
            Err(error) => self.fail(error),
        }
    }

    /// Asks for the project name before **Delete from cloud**.
    pub fn ask_cloud_delete(&mut self, project: &str) {
        self.project_menu = None;
        self.delete_typed.replace_text("");
        self.delete_confirm = Some(project.to_owned());
        self.dialog = Some(Dialog::ConfirmDelete);
    }

    /// Deletes `project` from the cloud for all members. It runs only after the typed name
    /// equals the project name.
    pub fn cloud_delete(&mut self, project: &str) {
        let confirmed = self.delete_confirm.as_deref() == Some(project)
            && cloud_views::delete_confirmed(&self.delete_typed.text, project);
        if !confirmed || !can_start(&self.busy) {
            return;
        }
        self.close_dialog();
        let project = project.to_owned();
        let label = t("Deleting from the cloud…", "กำลังลบออกจากคลาวด์…");
        start(self, label, move |bridge, home| {
            let u = account::session(home, &SystemStore::new(), bridge)?;
            sync::delete_cloud(home, &u, &project)?;
            Ok(Done::Toast(tf!(
                "Keyman deleted {} from the cloud.",
                "Keyman ลบ {} ออกจากคลาวด์แล้ว",
                project
            )))
        });
    }

    /// **Sync**: pull every project (this adds new cloud projects), then push.
    pub fn cloud_sync(&mut self) {
        if !matches!(self.cloud, CloudState::SignedIn { .. }) {
            return;
        }
        start(self, syncing_label(), |bridge, home| {
            let u = account::session(home, &SystemStore::new(), bridge)?;
            let report = sync::pull(home, &u, None, None, bridge)?;
            // Cancel was clicked during the pull: do not send anything.
            if bridge.cancelled() {
                return Err(CANCELLED.to_owned());
            }
            let pushed = sync::push(home, &u, None)?;
            Ok(Done::SyncFinished(summarize(&report, &pushed)))
        });
    }

    /// **Merge** in the Sync results: pull the cloud project into the local project with the
    /// same name (`keyman pull <project>`).
    pub fn cloud_merge(&mut self, project: &str) {
        let project = project.to_owned();
        let label = t("Merging…", "กำลังรวม…");
        start(self, label, move |bridge, home| {
            let u = account::session(home, &SystemStore::new(), bridge)?;
            let report = sync::pull(home, &u, Some(&project), None, bridge)?;
            Ok(Done::Merged {
                summary: summarize(&report, &[]),
                project,
            })
        });
    }

    /// **Apply** in the cloud conflict dialog: one choice per conflict. The picks are wiped.
    pub fn cloud_resolve_apply(&mut self) {
        let Some(mut picks) = self.cloud_picks.take() else {
            return;
        };
        if matches!(
            self.question.as_ref().map(|q| &q.ask),
            Some(Ask::Resolve { .. })
        ) && let Some(question) = self.question.take()
        {
            question.answer(Reply::Choices(picks.choices()));
        }
    }

    /// **Cancel** in the cloud conflict dialog: Keyman changes nothing for this project, and
    /// the sync goes on with the others.
    pub fn cloud_resolve_cancel(&mut self) {
        self.cloud_picks = None;
        if matches!(
            self.question.as_ref().map(|q| &q.ask),
            Some(Ask::Resolve { .. })
        ) && let Some(question) = self.question.take()
        {
            question.answer(Reply::Cancel);
        }
    }

    /// The typed value of conflict `i` in the cloud conflict dialog.
    pub fn cloud_pick_edit(&mut self, i: usize) -> &mut TextEdit {
        if self.cloud_picks.as_ref().is_some_and(|p| i < p.items.len()) {
            &mut self.cloud_picks.as_mut().expect("checked above").items[i].edit
        } else {
            &mut self.scratch
        }
    }

    /// Stops the running cloud task; an open question gets `Cancel`.
    pub fn cloud_cancel(&mut self) {
        cancel(self);
        self.cloud_fields.clear();
    }

    /// Sends the open question's answer from the dialog fields, when they are complete, then
    /// empties the fields.
    pub fn cloud_answer(&mut self) {
        let ready = self.question.as_ref().is_some_and(|question| {
            cloud_views::can_answer(
                &question.ask,
                &self.cloud_fields.first.text,
                &self.cloud_fields.second.text,
            )
        });
        if ready && let Some(question) = self.question.take() {
            question.answer(Reply::Text(self.cloud_fields.take()));
        }
    }

    /// The first-use card's **Skip**: it does not show again.
    pub fn skip_cloud_prompt(&mut self) {
        self.update_settings(|st| st.cloud_prompt_done = true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A window for tests: it keeps each question and answers it at once with `answer`.
    struct TestUi {
        asked: Arc<Mutex<Vec<Ask>>>,
        answer: fn(&Ask) -> Reply,
    }

    impl Ui for TestUi {
        fn run(&self, _: UiWork) {
            unreachable!("the bridge tests answer in `ask`");
        }

        fn ask(&self, question: Question) {
            let Question { ask, answer } = question;
            let _ = answer.send((self.answer)(&ask));
            self.asked.lock().unwrap().push(ask);
        }

        fn notice(&self, _: String) {}
    }

    /// A window for tests that queues the UI-thread work; the test runs it on an `App`.
    #[derive(Clone)]
    struct QueueUi(mpsc::Sender<UiWork>);

    impl Ui for QueueUi {
        fn run(&self, f: UiWork) {
            let _ = self.0.send(f);
        }
    }

    fn bridge(answer: fn(&Ask) -> Reply) -> (Bridge<TestUi>, Arc<Mutex<Vec<Ask>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let ui = TestUi {
            asked: asked.clone(),
            answer,
        };
        (Bridge::new(ui, Arc::new(AtomicBool::new(false))), asked)
    }

    fn busy(label: &'static str) -> Option<Busy> {
        Some(Busy::new(label))
    }

    fn conflict() -> Conflict {
        Conflict {
            name: "KEY".into(),
            local: Some("a".into()),
            cloud: Some("b".into()),
        }
    }

    /// A thread that waits on `ask`; the test gets its question.
    fn waiting_thread(ask: Ask) -> (thread::JoinHandle<Reply>, Question) {
        let (tx_q, rx_q) = mpsc::channel();
        let waiting = thread::spawn(move || {
            ask_blocking(
                |q| {
                    let _ = tx_q.send(q);
                },
                ask,
            )
        });
        (waiting, rx_q.recv().unwrap())
    }

    /// A home that does not exist: nothing on disk is read or written.
    fn no_home() -> Option<std::path::PathBuf> {
        Some(std::env::temp_dir().join("keyman-app-test-no-home"))
    }

    #[test]
    fn an_answer_reaches_the_waiting_thread() {
        let (waiting, question) = waiting_thread(Ask::NewPassword);
        assert_eq!(question.ask, Ask::NewPassword);
        question.answer(Reply::Text("p".into()));
        assert_eq!(waiting.join().unwrap(), Reply::Text("p".into()));
    }

    #[test]
    fn dropped_answer_cancels_the_question() {
        let (waiting, question) = waiting_thread(Ask::RecoveryCode);
        drop(question);
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);
    }

    #[test]
    fn bridge_cancel_is_an_error_for_the_library() {
        let (mut b, _) = bridge(|_| Reply::Cancel);
        assert_eq!(b.new_password(), Err(CANCELLED.to_string()));
        assert_eq!(b.password(), Err(CANCELLED.to_string()));
        assert_eq!(b.recovery_code(), Err(CANCELLED.to_string()));
        assert_eq!(b.show_recovery_code("CODE"), Err(CANCELLED.to_string()));
        assert_eq!(b.resolve("app", &[conflict()]), Ok(None));

        // A dropped question is a cancel too.
        struct DropUi;
        impl Ui for DropUi {
            fn run(&self, _: UiWork) {}
            fn ask(&self, question: Question) {
                drop(question);
            }
        }
        let mut dropping = Bridge::new(DropUi, Arc::new(AtomicBool::new(false)));
        assert_eq!(dropping.password(), Err(CANCELLED.to_string()));
        assert_eq!(dropping.resolve("app", &[conflict()]), Ok(None));

        // After Cancel was clicked, the bridge asks nothing more.
        let (mut stopped, asked) = bridge(|_| Reply::Text("x".into()));
        stopped.cancel.store(true, Ordering::SeqCst);
        assert!(stopped.cancelled());
        assert_eq!(stopped.password(), Err(CANCELLED.to_string()));
        assert!(asked.lock().unwrap().is_empty());
    }

    #[test]
    fn a_question_after_cancel_is_not_shown() {
        let mut app = App::for_tests();
        app.busy = busy("Signing in…");
        // The thread read the flag before the click, so its question is on the way.
        let (waiting, question) = waiting_thread(Ask::NewPassword);
        cancel(&mut app);
        let busy_now = app.busy.as_ref().unwrap();
        assert!(busy_now.cancel.load(Ordering::SeqCst));
        // Either language: another test may switch it meanwhile.
        assert!(["Cancelling…", "กำลังยกเลิก…"].contains(&busy_now.label));
        post(&mut app, question);
        assert!(app.question.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);

        // Cancel with a question open answers it.
        app.busy = busy("Signing in…");
        let (waiting, question) = waiting_thread(Ask::RecoveryCode);
        post(&mut app, question);
        assert!(app.question.is_some());
        cancel(&mut app);
        assert!(app.question.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);

        // No task running: a question is not shown.
        app.busy = None;
        let (waiting, question) = waiting_thread(Ask::NewPassword);
        post(&mut app, question);
        assert!(app.question.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);
    }

    #[test]
    fn bridge_answers_reach_the_library() {
        let (mut b, asked) = bridge(|ask| match ask {
            Ask::Resolve { .. } => Reply::Choices(vec![Some("b".into())]),
            _ => Reply::Text("secret".into()),
        });
        assert_eq!(b.new_password(), Ok("secret".into()));
        assert_eq!(
            b.resolve("app", &[conflict()]),
            Ok(Some(vec![Some("b".into())]))
        );
        assert_eq!(
            asked.lock().unwrap()[1],
            Ask::Resolve {
                project: "app".into(),
                conflicts: vec![conflict()]
            }
        );

        // A wrong number of choices changes nothing.
        let (mut short, _) = bridge(|_| Reply::Choices(Vec::new()));
        assert_eq!(short.resolve("app", &[conflict()]), Ok(None));
    }

    #[test]
    fn password_tries_count_down() {
        let (mut b, asked) = bridge(|_| Reply::Text("pw".into()));
        for _ in 0..3 {
            assert_eq!(b.password(), Ok("pw".into()));
        }
        assert_eq!(
            *asked.lock().unwrap(),
            vec![
                Ask::Password { tries_left: 3 },
                Ask::Password { tries_left: 2 },
                Ask::Password { tries_left: 1 },
            ]
        );
    }

    #[test]
    fn recovery_code_confirmation_is_answered_once() {
        let (mut b, asked) = bridge(|ask| match ask {
            Ask::ShowRecoveryCode(code) => Reply::Text(code.as_str().to_owned()),
            _ => Reply::Text("again".into()),
        });
        assert_eq!(b.show_recovery_code("ABCD-EFGH"), Ok(()));
        // The typed code answers the next call without a second dialog.
        assert_eq!(b.recovery_code(), Ok("ABCD-EFGH".into()));
        assert_eq!(
            *asked.lock().unwrap(),
            vec![Ask::ShowRecoveryCode(Zeroizing::new("ABCD-EFGH".into()))]
        );
        // It is used once: a later call asks again.
        assert_eq!(b.recovery_code(), Ok("again".into()));
        assert!(b.typed_code.is_none());
        assert_eq!(asked.lock().unwrap().len(), 2);
    }

    #[test]
    fn secrets_are_not_in_debug_text() {
        let text = format!(
            "{:?} {:?}",
            Ask::ShowRecoveryCode(Zeroizing::new("ABCD-EFGH".into())),
            Reply::Text("hunter2-password".into())
        );
        assert!(
            !text.contains("ABCD") && !text.contains("hunter2"),
            "{text}"
        );
    }

    #[test]
    fn only_one_task_runs() {
        assert!(can_start(&None));
        assert!(!can_start(&busy("Syncing…")));

        let mut app = App::for_tests();
        app.home = no_home();
        let (tx, rx) = mpsc::channel();
        let ui = Some(QueueUi(tx));
        let refresh: fn(&Path) -> CloudState = |_| CloudState::SignedOut;
        let work = |_: &mut Bridge<QueueUi>, _: &Path| Ok(Done::Toast("done".into()));

        // With a window, a running task still blocks a second one.
        app.busy = busy("Syncing…");
        assert!(!start_with(
            &mut app,
            ui.clone(),
            refresh,
            "Signing in…",
            work
        ));
        assert_eq!(app.busy.as_ref().map(|b| b.label), Some("Syncing…"));
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

        // No window: nothing to show the questions, so nothing starts.
        app.busy = None;
        assert!(!start_with(
            &mut app,
            None::<QueueUi>,
            refresh,
            "Signing in…",
            work
        ));
        assert!(app.busy.is_none());

        // One task starts; a second waits for its end.
        assert!(start_with(
            &mut app,
            ui.clone(),
            refresh,
            "Signing in…",
            work
        ));
        assert_eq!(app.busy.as_ref().map(|b| b.label), Some("Signing in…"));
        assert!(!start_with(&mut app, ui.clone(), refresh, "Syncing…", work));
        let end = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        end(&mut app);
        assert!(app.busy.is_none());
        assert!(app.toast.as_ref().is_some_and(|toast| toast.text == "done"));
        assert!(start_with(&mut app, ui, refresh, "Syncing…", work));
        rx.recv_timeout(Duration::from_secs(10)).unwrap()(&mut app);
        assert!(app.busy.is_none());
    }

    #[test]
    fn a_panic_in_a_task_returns_to_idle() {
        let mut app = App::for_tests();
        app.home = no_home();
        let signed_in = CloudState::SignedIn {
            email: "a@b.c".into(),
        };
        app.cloud = signed_in.clone();
        let (tx, rx) = mpsc::channel();
        let refresh: fn(&Path) -> CloudState = |_| panic!("refresh");
        let work = |_: &mut Bridge<QueueUi>, _: &Path| -> Result<Done, String> { panic!("work") };
        assert!(start_with(
            &mut app,
            Some(QueueUi(tx)),
            refresh,
            "Syncing…",
            work
        ));
        rx.recv_timeout(Duration::from_secs(10)).unwrap()(&mut app);
        assert!(app.busy.is_none());
        // The shown state stays when the refresh fails.
        assert_eq!(app.cloud, signed_in);
        assert!(app.toast.as_ref().is_some_and(|toast| !toast.ok));
    }

    #[test]
    fn the_end_of_a_task_releases_an_open_question() {
        let mut app = App::for_tests();
        let (waiting, question) = waiting_thread(Ask::NewPassword);
        app.question = Some(question);
        app.busy = busy("Signing in…");
        finish(&mut app, Some(CloudState::SignedOut), Err(CANCELLED.into()));
        assert!(app.busy.is_none() && app.question.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);
        // A cancel is not an error for the person.
        assert!(app.toast.is_none());

        finish(&mut app, None, Err("boom".into()));
        assert!(
            app.toast
                .as_ref()
                .is_some_and(|toast| !toast.ok && toast.text == "boom")
        );
    }

    #[test]
    fn the_recovery_hint_is_in_app_words() {
        let error = format!(
            "The vault password is not correct.\n\n{}",
            account::RECOVER_HINT
        );
        let shown = app_wording(&error);
        assert!(shown.starts_with("The vault password is not correct.\n\n"));
        assert!(!shown.contains("keyman recover"), "{shown}");
        assert!(
            shown.ends_with("Use 'Use recovery code' in Settings › Account.")
                || shown.ends_with("ในการตั้งค่า › บัญชี"),
            "{shown}"
        );
        // Other errors stay as they are.
        assert_eq!(app_wording("boom"), "boom");

        // `finish` shows the app words.
        let mut app = App::for_tests();
        app.busy = busy("Signing in…");
        finish(&mut app, None, Err(error));
        let toast = app.toast.as_ref().unwrap();
        assert!(!toast.ok && !toast.text.contains("keyman recover"));
    }

    #[test]
    fn a_sign_in_closes_the_first_use_card_for_good() {
        let mut app = App::for_tests();
        assert!(!app.settings.cloud_prompt_done);
        app.busy = busy("Signing in…");
        let done = Done::SignedIn {
            email: "a@b.c".into(),
        };
        finish(&mut app, None, Ok(done));
        assert!(app.settings.cloud_prompt_done);
        // Saved, so a sign-out and a new start keep it closed.
        assert!(crate::settings::Settings::load(&app.prefs).cloud_prompt_done);
    }

    #[test]
    fn closing_the_settings_resets_the_recovery_picker() {
        let mut app = App::for_tests();
        app.dialog = Some(crate::Dialog::Settings(crate::SettingsPage::Account));
        app.recover_pick = true;
        app.close_dialog();
        assert!(app.dialog.is_none() && !app.recover_pick);
    }

    #[test]
    fn the_sign_in_link_shows_while_the_browser_waits() {
        let mut app = App::for_tests();
        app.busy = busy("Signing in…");
        let (tx, rx) = mpsc::channel();
        let mut b = Bridge::new(QueueUi(tx), Arc::new(AtomicBool::new(false)));
        b.sign_in_link("https://example.test/authorize?x=1");
        rx.recv_timeout(Duration::from_secs(1)).unwrap()(&mut app);
        let now = app.busy.as_ref().unwrap();
        assert_eq!(
            now.link.as_deref(),
            Some("https://example.test/authorize?x=1")
        );
        assert_eq!(now.label, waiting_label());

        // The first question ends the browser part.
        let (waiting, question) = waiting_thread(Ask::NewPassword);
        post(&mut app, question);
        let now = app.busy.as_ref().unwrap();
        assert!(now.link.is_none());
        assert_eq!(now.label, signing_in_label());
        cancel(&mut app);
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);

        // After Cancel, a late link is not shown.
        b.sign_in_link("https://example.test/late");
        rx.recv_timeout(Duration::from_secs(1)).unwrap()(&mut app);
        assert!(app.busy.as_ref().unwrap().link.is_none());
    }

    #[test]
    fn an_answer_empties_the_fields() {
        let mut app = App::for_tests();
        app.busy = busy("Signing in…");
        let (waiting, question) = waiting_thread(Ask::NewPassword);
        post(&mut app, question);
        app.cloud_fields.first.replace_text("abcdefghij");
        app.cloud_fields.second.replace_text("abcdefghik");
        // Not the same: nothing is sent.
        app.cloud_answer();
        assert!(app.question.is_some());
        app.cloud_fields.second.replace_text("abcdefghij");
        app.cloud_answer();
        assert!(app.question.is_none());
        assert!(app.cloud_fields.first.text.is_empty() && app.cloud_fields.second.text.is_empty());
        assert_eq!(waiting.join().unwrap(), Reply::Text("abcdefghij".into()));

        // Cancel empties them too, and a new question starts with empty fields.
        let (waiting, question) = waiting_thread(Ask::Password { tries_left: 3 });
        post(&mut app, question);
        app.cloud_fields.first.replace_text("typed");
        app.cloud_cancel();
        assert!(app.cloud_fields.first.text.is_empty());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);

        app.busy = busy("Signing in…");
        app.cloud_fields.first.replace_text("left over");
        let (waiting, question) = waiting_thread(Ask::RecoveryCode);
        post(&mut app, question);
        assert!(app.cloud_fields.first.text.is_empty());
        app.cloud_fields.first.replace_text("ABCD-EFGH");
        app.cloud_answer();
        assert_eq!(waiting.join().unwrap(), Reply::Text("ABCD-EFGH".into()));
    }

    #[test]
    fn the_recovery_code_dialog_answers_with_the_typed_code() {
        let mut app = App::for_tests();
        app.busy = busy("Signing in…");
        let code = "ABCD-EFGH-JKMN-PQRS";
        let (waiting, question) =
            waiting_thread(Ask::ShowRecoveryCode(Zeroizing::new(code.into())));
        post(&mut app, question);
        app.cloud_fields.first.replace_text("abcd efgh jkmn pqr");
        app.cloud_answer();
        assert!(
            app.question.is_some(),
            "a wrong code does not close the dialog"
        );
        app.cloud_fields.first.replace_text("abcd-efgh jkmn pqrs");
        app.cloud_answer();
        assert!(app.question.is_none());
        assert_eq!(
            waiting.join().unwrap(),
            Reply::Text("abcd-efgh jkmn pqrs".into())
        );
    }

    #[test]
    fn delete_waits_for_the_exact_name() {
        let mut app = App::for_tests();
        app.ask_cloud_delete("my-app");
        assert_eq!(app.dialog, Some(Dialog::ConfirmDelete));
        app.delete_typed.replace_text("My-App");
        app.cloud_delete("my-app");
        // No match: the dialog stays and no task starts.
        assert_eq!(app.dialog, Some(Dialog::ConfirmDelete));
        assert!(app.busy.is_none() && app.toast.is_none());
    }

    fn summary(updated: usize, added: &[&str], sent: usize) -> SyncSummary {
        SyncSummary {
            updated,
            added: added.iter().map(|name| (*name).to_owned()).collect(),
            sent,
            attention: Vec::new(),
            given: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn summarize_counts_and_attention() {
        let report = PullReport {
            results: vec![
                PullResult::Updated {
                    project: "a".into(),
                    changes: 2,
                },
                PullResult::Added("b".into()),
                PullResult::UpToDate("c".into()),
                PullResult::Stopped {
                    project: "d".into(),
                    conflicts: 1,
                },
                PullResult::Skipped {
                    project: "e".into(),
                    reason: "why".into(),
                },
                PullResult::NameTaken {
                    project: "f".into(),
                },
            ],
            given: vec![("team".into(), "x@y.z".into())],
            warnings: vec!["warn".into()],
        };
        let pushed = [
            PushResult::Pushed {
                project: "a".into(),
                version: 3,
            },
            PushResult::CloudIsNewer("g".into()),
        ];
        let got = summarize(&report, &pushed);
        assert_eq!(got.updated, 1);
        assert_eq!(got.added, vec!["b".to_string()]);
        assert_eq!(got.sent, 1);
        assert_eq!(
            got.attention,
            vec![
                Attention::Stopped {
                    project: "d".into(),
                    conflicts: 1
                },
                Attention::Skipped {
                    project: "e".into(),
                    reason: "why".into()
                },
                Attention::CanMerge("f".into()),
                Attention::CloudIsNewer("g".into()),
            ]
        );
        assert_eq!(got.given, report.given);
        assert_eq!(got.warnings, report.warnings);

        // A project the pull stopped is listed once, not again for the push.
        let stopped = PullReport {
            results: vec![PullResult::Stopped {
                project: "d".into(),
                conflicts: 2,
            }],
            given: Vec::new(),
            warnings: Vec::new(),
        };
        let again = summarize(&stopped, &[PushResult::CloudIsNewer("d".into())]);
        assert_eq!(again.attention.len(), 1);
    }

    #[test]
    fn toast_text_rules() {
        assert_eq!(
            toast_words(&summary(0, &[], 0), false).as_deref(),
            Some("All projects are up to date.")
        );
        assert_eq!(
            toast_words(&summary(2, &["n"], 1), false).as_deref(),
            Some("2 updated, 1 added, 1 sent.")
        );
        // Zero parts are left out.
        assert_eq!(
            toast_words(&summary(0, &["n", "m"], 0), false).as_deref(),
            Some("2 added.")
        );
        assert_eq!(
            toast_words(&summary(1, &[], 3), false).as_deref(),
            Some("1 updated, 3 sent.")
        );
        assert_eq!(
            toast_words(&summary(0, &[], 0), true).as_deref(),
            Some("ทุกโปรเจกต์เป็นปัจจุบันแล้ว")
        );
        assert_eq!(
            toast_words(&summary(2, &["n"], 1), true).as_deref(),
            Some("อัปเดต 2 โปรเจกต์, เพิ่ม 1 โปรเจกต์, ส่ง 1 โปรเจกต์")
        );
        // Access given to invited people goes in the toast too.
        let mut given = summary(0, &[], 0);
        given.given.push(("team".into(), "x@y.z".into()));
        assert_eq!(
            toast_words(&given, false).as_deref(),
            Some("All projects are up to date.\nGave x@y.z access to team.")
        );
        // Anything that needs the person opens the dialog instead.
        let mut attention = summary(2, &["n"], 1);
        attention
            .attention
            .push(Attention::CloudIsNewer("g".into()));
        assert_eq!(toast_words(&attention, false), None);
        let mut warned = summary(0, &[], 0);
        warned.warnings.push("warn".into());
        assert_eq!(toast_words(&warned, false), None);
    }

    const REJECTED: [&str; 2] = [
        "Your sign-in is not valid now. Sign in again.",
        "การลงชื่อเข้าใช้นี้ใช้ไม่ได้แล้ว ลงชื่อเข้าใช้อีกครั้ง",
    ];

    #[test]
    fn rejected_sign_in_signs_out() {
        let mut app = App::for_tests();
        app.cloud = CloudState::SignedIn {
            email: "a@b.c".into(),
        };
        app.busy = busy("Syncing…");
        let (waiting, question) = waiting_thread(Ask::Password { tries_left: 3 });
        app.question = Some(question);
        after_failure(&mut app, account::SIGN_IN_INVALID);
        assert_eq!(app.cloud, CloudState::SignedOut);
        assert_eq!(
            app.dialog,
            Some(Dialog::Settings(crate::SettingsPage::Account))
        );
        let toast = app.toast.as_ref().unwrap();
        assert!(REJECTED.contains(&toast.text.as_str()), "{}", toast.text);
        assert!(app.busy.is_none() && app.question.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);

        // `finish` takes the same way for every task.
        let mut app = App::for_tests();
        app.cloud = CloudState::SignedIn {
            email: "a@b.c".into(),
        };
        app.busy = busy("Linking…");
        finish(&mut app, None, Err(account::SIGN_IN_INVALID.into()));
        assert_eq!(app.cloud, CloudState::SignedOut);
        assert_eq!(
            app.dialog,
            Some(Dialog::Settings(crate::SettingsPage::Account))
        );
    }

    #[test]
    fn a_failed_task_returns_to_idle() {
        let mut app = App::for_tests();
        let signed_in = CloudState::SignedIn {
            email: "a@b.c".into(),
        };
        app.cloud = signed_in.clone();
        app.busy = busy("Syncing…");
        let (waiting, question) = waiting_thread(Ask::Password { tries_left: 3 });
        app.question = Some(question);
        let error = "Keyman cannot connect to Keyman Cloud.";
        after_failure(&mut app, error);
        assert!(app.busy.is_none() && app.question.is_none());
        assert_eq!(app.cloud, signed_in);
        assert!(app.dialog.is_none());
        let toast = app.toast.as_ref().unwrap();
        assert!(!toast.ok && toast.text == error);
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);

        // A cancel shows no error.
        app.toast = None;
        app.busy = busy("Syncing…");
        after_failure(&mut app, CANCELLED);
        assert!(app.busy.is_none() && app.toast.is_none());
    }

    fn resolve_ask() -> Ask {
        Ask::Resolve {
            project: "app".into(),
            conflicts: vec![
                conflict(),
                Conflict {
                    name: "GONE".into(),
                    local: Some("old".into()),
                    cloud: None,
                },
                Conflict {
                    name: "TYPED".into(),
                    local: Some("x".into()),
                    cloud: Some("y".into()),
                },
            ],
        }
    }

    #[test]
    fn a_cloud_choice_answers_and_wipes_the_picks() {
        let mut app = App::for_tests();
        app.busy = busy("Syncing…");
        let (waiting, question) = waiting_thread(resolve_ask());
        post(&mut app, question);
        let picks = app.cloud_picks.as_mut().expect("picks for a conflict");
        assert_eq!(picks.items.len(), 3);
        assert!(!picks.reveal, "values are hidden until revealed");
        picks.items[0].pick = Pick::Env;
        picks.items[1].pick = Pick::Env;
        picks.items[2].pick = Pick::Edit;
        app.cloud_pick_edit(2).replace_text("typed");
        app.cloud_resolve_apply();
        assert!(app.question.is_none() && app.cloud_picks.is_none());
        assert_eq!(
            waiting.join().unwrap(),
            Reply::Choices(vec![Some("b".into()), None, Some("typed".into())])
        );
        // The answer does not stop the task.
        assert!(!app.busy.as_ref().unwrap().cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_in_the_cloud_resolver_stops_only_that_project() {
        let mut app = App::for_tests();
        app.busy = busy("Syncing…");
        let (waiting, question) = waiting_thread(resolve_ask());
        post(&mut app, question);
        app.cloud_picks.as_mut().unwrap().items[0].pick = Pick::Edit;
        app.cloud_pick_edit(0).replace_text("typed");
        app.cloud_resolve_cancel();
        assert!(app.question.is_none() && app.cloud_picks.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);
        assert!(!app.busy.as_ref().unwrap().cancel.load(Ordering::SeqCst));

        // The task's Cancel and its end also drop the picks.
        let (waiting, question) = waiting_thread(resolve_ask());
        post(&mut app, question);
        app.cloud_cancel();
        assert!(app.cloud_picks.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);
        app.busy = busy("Syncing…");
        let (waiting, question) = waiting_thread(resolve_ask());
        post(&mut app, question);
        finish(&mut app, None, Err("boom".into()));
        assert!(app.cloud_picks.is_none());
        assert_eq!(waiting.join().unwrap(), Reply::Cancel);
    }

    #[test]
    fn a_sync_opens_the_results_only_when_needed() {
        let mut app = App::for_tests();
        app.busy = busy("Syncing…");
        finish(&mut app, None, Ok(Done::SyncFinished(summary(1, &[], 0))));
        assert!(app.dialog.is_none() && app.sync_results.is_none());
        assert!(app.toast.as_ref().is_some_and(|toast| toast.ok));

        let mut needs = summary(0, &["new"], 0);
        needs.attention.push(Attention::CanMerge("f".into()));
        needs.given.push(("team".into(), "x@y.z".into()));
        app.toast = None;
        app.busy = busy("Syncing…");
        finish(&mut app, None, Ok(Done::SyncFinished(needs.clone())));
        assert_eq!(app.dialog, Some(Dialog::SyncResults));
        assert_eq!(app.sync_results.as_ref(), Some(&needs));
        // The invite lines are in the dialog, not also in a toast.
        assert!(app.toast.is_none());
    }

    #[test]
    fn merge_updates_or_closes_the_results() {
        let mut open = summary(0, &[], 0);
        open.attention = vec![
            Attention::CanMerge("f".into()),
            Attention::CloudIsNewer("g".into()),
        ];
        let mut stopped = summary(0, &[], 0);
        stopped.attention.push(Attention::Stopped {
            project: "f".into(),
            conflicts: 1,
        });
        let next = merge_into(open.clone(), "f", stopped);
        assert_eq!(
            next.attention,
            vec![
                Attention::CloudIsNewer("g".into()),
                Attention::Stopped {
                    project: "f".into(),
                    conflicts: 1
                },
            ]
        );

        // A clean merge with another row left: the row goes, the dialog stays.
        let mut app = App::for_tests();
        app.sync_results = Some(open.clone());
        app.dialog = Some(Dialog::SyncResults);
        app.busy = busy("Merging…");
        let done = Done::Merged {
            project: "f".into(),
            summary: summary(1, &[], 0),
        };
        finish(&mut app, None, Ok(done));
        assert_eq!(app.dialog, Some(Dialog::SyncResults));
        assert_eq!(
            app.sync_results.as_ref().unwrap().attention,
            vec![Attention::CloudIsNewer("g".into())]
        );
        assert!(app.toast.as_ref().is_some_and(|toast| toast.ok));

        // The last row: the dialog closes.
        let mut only = summary(0, &[], 0);
        only.attention.push(Attention::CanMerge("f".into()));
        app.sync_results = Some(only);
        app.busy = busy("Merging…");
        let done = Done::Merged {
            project: "f".into(),
            summary: summary(1, &[], 0),
        };
        finish(&mut app, None, Ok(done));
        assert!(app.dialog.is_none() && app.sync_results.is_none());
    }

    #[test]
    fn unlink_does_nothing_while_busy() {
        let mut app = App::for_tests();
        app.busy = Some(Busy::new("Syncing…"));
        app.cloud_unlink("my-app");
        assert!(app.toast.is_none());
    }
}
