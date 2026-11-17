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
use keyman_cli::cloud::sync::{self, Resolver as CloudResolver, Vaults};
use rune::{AppHandle, TextEdit};
use zeroize::Zeroizing;

use crate::cloud_views;
use crate::i18n::t;
use crate::{App, Dialog, LinkPicker};

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
    #[expect(dead_code, reason = "the Sync action (a later step) makes it")]
    SyncFinished(SyncSummary),
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
#[expect(dead_code, reason = "the Sync action (a later step) makes these")]
pub enum Attention {
    Skipped { project: String, reason: String },
    Stopped { project: String, conflicts: usize },
    CloudIsNewer(String),
    CanMerge(String),
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
    s.question = None;
    s.cloud_fields.clear();
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
        // The person stopped the task: no error to show.
        Err(error) if error == CANCELLED || error == account::SIGN_IN_CANCELLED => {}
        Err(error) => s.fail(app_wording(&error)),
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
        Done::SyncFinished(summary) => s.ok(tf!(
            "Sync finished: {} updated, {} added, {} sent.",
            "ซิงก์เสร็จแล้ว: อัปเดต {} เพิ่ม {} ส่ง {}",
            summary.updated,
            summary.added.len(),
            summary.sent
        )),
        // The link picker was open while they loaded.
        Done::Groups(groups) => {
            if let Some(picker) = &mut s.link_picker {
                picker.groups = Some(groups);
            }
        }
    }
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

    #[test]
    fn unlink_does_nothing_while_busy() {
        let mut app = App::for_tests();
        app.busy = Some(Busy::new("Syncing…"));
        app.cloud_unlink("my-app");
        assert!(app.toast.is_none());
    }
}
