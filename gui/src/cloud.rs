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
use keyman_cli::cloud::keystore::SystemStore;
use keyman_cli::cloud::merge::Conflict;
use keyman_cli::cloud::sync::Resolver as CloudResolver;
use rune::AppHandle;
use zeroize::Zeroizing;

use crate::App;
use crate::i18n::t;

/// What a bridge question gives the library when the person cancels it.
pub const CANCELLED: &str = "cancelled";

/// The library asks for the vault password at most this many times in a row.
const PASSWORD_TRIES: u8 = 3;

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
    if s.busy
        .as_ref()
        .is_some_and(|busy| !busy.cancel.load(Ordering::SeqCst))
    {
        s.question = Some(question);
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
            s.busy = Some(Busy { label, cancel });
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
    if let Some(cloud) = cloud {
        s.cloud = cloud;
    }
    s.reload();
    match result {
        Ok(done) => apply(s, done),
        // The person stopped the task: no error to show.
        Err(error) if error == CANCELLED || error == account::SIGN_IN_CANCELLED => {}
        Err(error) => s.fail(error),
    }
}

fn apply(s: &mut App, done: Done) {
    match done {
        Done::SignedIn { email } => {
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
        // The link picker shows them (a later step).
        Done::Groups(_) => {}
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
        Some(Busy {
            label,
            cancel: Arc::new(AtomicBool::new(false)),
        })
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
}
