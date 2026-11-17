//! End-to-end tests of Keyman Cloud against a local Supabase stack.
//!
//! They need Docker, so each test returns at once unless `KEYMAN_E2E=1`.
//! The local stack must allow email and password sign-in for these tests
//! (the hosted project does not). Run from the repository root:
//!
//! ```sh
//! SUPABASE_AUTH_EMAIL_ENABLED=true supabase start
//! set -a; eval "$(supabase status -o env)"; set +a   # API_URL, ANON_KEY, SERVICE_ROLE_KEY
//! KEYMAN_E2E=1 cargo test --test cloud_e2e -- --test-threads=1
//! ```
//!
//! Users are made with the admin API (service role key) and sign in with a
//! password, so no browser is necessary.

use keyman_cli::cloud::account::{self, LoginOutcome, Prompt, Unlocked};
use keyman_cli::cloud::api::{self, Session};
use keyman_cli::cloud::crypto::KdfParams;
use keyman_cli::cloud::groups::{self, Removal};
use keyman_cli::cloud::keystore::MemoryStore;
use keyman_cli::cloud::merge::{self, Conflict, Keep};
use keyman_cli::cloud::sync::{self, PullResult, PushResult, Resolver, Vaults};
use keyman_cli::master::{self, Variables};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PASSWORD: &str = "e2e vault password";
const NEW_PASSWORD: &str = "e2e new vault password";
const AUTH_PASSWORD: &str = "e2e-sign-in-password-1";
/// The lowest settings that `KdfParams::check` accepts, so unlock is fast.
const PARAMS: KdfParams = KdfParams {
    m: 19456,
    t: 2,
    p: 1,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn unique() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!(
        "{nanos}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The local stack. Making it also points Keyman at the stack.
struct Local {
    url: String,
    anon: String,
    service: String,
}

/// `None` (skip the test) unless `KEYMAN_E2E=1`.
fn local() -> Option<Local> {
    if std::env::var("KEYMAN_E2E").as_deref() != Ok("1") {
        return None;
    }
    let var = |name: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("set {name} (run: eval \"$(supabase status -o env)\")"))
    };
    let url = std::env::var("API_URL").unwrap_or_else(|_| "http://127.0.0.1:54321".to_string());
    let local = Local {
        url: url.trim_end_matches('/').to_string(),
        anon: var("ANON_KEY"),
        service: var("SERVICE_ROLE_KEY"),
    };
    std::env::set_var("KEYMAN_SUPABASE_URL", &local.url);
    std::env::set_var("KEYMAN_SUPABASE_KEY", &local.anon);
    Some(local)
}

macro_rules! local_or_skip {
    () => {
        match local() {
            Some(local) => local,
            None => return,
        }
    };
}

fn call(
    method: &str,
    url: &str,
    key: &str,
    bearer: Option<&str>,
    body: Option<&Value>,
) -> (u16, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build(),
    );
    let mut req = ureq::http::Request::builder()
        .method(method)
        .uri(url)
        .header("apikey", key);
    if let Some(token) = bearer {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let payload = match body {
        Some(body) => {
            req = req.header("Content-Type", "application/json");
            body.to_string()
        }
        None => String::new(),
    };
    let mut resp = agent
        .run(req.body(payload).unwrap())
        .expect("the local Supabase stack must run");
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap();
    (status, text)
}

impl Local {
    /// A legacy service key is a JWT and also goes in `Authorization`.
    fn service_bearer(&self) -> Option<&str> {
        self.service
            .starts_with("eyJ")
            .then_some(self.service.as_str())
    }

    /// Read rows as the service role (row level security does not apply).
    fn service_get(&self, path: &str) -> String {
        let url = format!("{}{path}", self.url);
        let (status, body) = call("GET", &url, &self.service, self.service_bearer(), None);
        assert!(status < 300, "GET {path}: {status} {body}");
        body
    }

    /// A new user with a confirmed email (the RLS policies require one).
    fn new_user(&self, tag: &str) -> String {
        let email = format!("{tag}-{}@example.com", unique());
        let url = format!("{}/auth/v1/admin/users", self.url);
        let body = json!({"email": email, "password": AUTH_PASSWORD, "email_confirm": true});
        let (status, text) = call(
            "POST",
            &url,
            &self.service,
            self.service_bearer(),
            Some(&body),
        );
        assert!(status < 300, "admin user create: {status} {text}");
        email
    }

    fn sign_in(&self, email: &str) -> Session {
        let url = format!("{}/auth/v1/token?grant_type=password", self.url);
        let body = json!({"email": email, "password": AUTH_PASSWORD});
        let (status, text) = call("POST", &url, &self.anon, None, Some(&body));
        assert!(status < 300, "password sign-in: {status} {text}");
        api::parse_session(&text, now()).unwrap()
    }
}

/// Answers for the account questions.
struct Answers {
    password: String,
    new_password: String,
    code: String,
}

impl Answers {
    fn password(password: &str) -> Answers {
        Answers {
            password: password.to_string(),
            new_password: password.to_string(),
            code: String::new(),
        }
    }
}

impl Prompt for Answers {
    fn new_password(&mut self) -> Result<String, String> {
        Ok(self.new_password.clone())
    }
    fn password(&mut self) -> Result<String, String> {
        Ok(self.password.clone())
    }
    fn recovery_code(&mut self) -> Result<String, String> {
        Ok(self.code.clone())
    }
    fn show_recovery_code(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A conflict must not occur.
struct NoConflicts;

impl Resolver for NoConflicts {
    fn resolve(
        &mut self,
        project: &str,
        _: &[Conflict],
    ) -> Result<Option<Vec<Option<String>>>, String> {
        panic!("unexpected conflict in {project}");
    }
}

struct KeepCloud;

impl Resolver for KeepCloud {
    fn resolve(
        &mut self,
        _: &str,
        conflicts: &[Conflict],
    ) -> Result<Option<Vec<Option<String>>>, String> {
        Ok(Some(merge::choose_all(conflicts, Keep::Cloud)))
    }
}

/// One computer: a home folder with a master file, and a credential store.
struct Computer {
    home: PathBuf,
    store: MemoryStore,
}

impl Computer {
    fn new(name: &str, master_text: &str) -> Computer {
        let home = std::env::temp_dir().join(format!("keyman-e2e-{name}-{}", unique()));
        fs::create_dir_all(&home).unwrap();
        let computer = Computer {
            home,
            store: MemoryStore::new(),
        };
        fs::write(
            computer.home.join(".keyman_config"),
            computer.master_path().to_string_lossy().as_ref(),
        )
        .unwrap();
        computer.write_master(master_text);
        computer
    }

    fn master_path(&self) -> PathBuf {
        self.home.join("master.yml")
    }

    fn write_master(&self, text: &str) {
        fs::write(self.master_path(), text).unwrap();
    }

    /// The variables of a project, sorted by name.
    fn vars(&self, project: &str) -> Variables {
        let (_, yaml) = master::read_master(&self.home).unwrap();
        let mut vars = master::project_variables(&yaml, project).unwrap();
        vars.sort();
        vars
    }

    /// A new account on this computer. Returns the email and the recovery code.
    fn set_up(&self, local: &Local, tag: &str) -> (String, String) {
        let email = local.new_user(tag);
        let code = self.set_up_as(local, &email);
        (email, code)
    }

    /// Set up an account for a user that exists. Returns the recovery code.
    fn set_up_as(&self, local: &Local, email: &str) -> String {
        let session = local.sign_in(email);
        let (_, code) =
            account::setup_with(&self.home, &self.store, &session, PASSWORD, PARAMS).unwrap();
        code
    }

    /// Sign in on this computer to an account that was set up on another one.
    fn log_in(&self, local: &Local, email: &str) {
        let session = local.sign_in(email);
        let outcome = account::login_with_session(
            &self.home,
            &self.store,
            &session,
            &mut Answers::password(PASSWORD),
        )
        .unwrap();
        assert_eq!(outcome, LoginOutcome::Unlocked);
    }

    fn unlocked(&self) -> Unlocked {
        account::session(&self.home, &self.store, &mut Answers::password(PASSWORD)).unwrap()
    }

    fn push(&self) -> Vec<PushResult> {
        sync::push(&self.home, &self.unlocked(), None).unwrap()
    }

    fn pull(&self, resolver: &mut dyn Resolver) -> Vec<PullResult> {
        sync::pull(&self.home, &self.unlocked(), None, None, resolver).unwrap()
    }
}

fn var(vars: &Variables, name: &str) -> Option<String> {
    vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
}

fn no_skips(results: &[PullResult]) {
    for result in results {
        assert!(
            !matches!(result, PullResult::Skipped { .. }),
            "skipped: {result:?}"
        );
    }
}

const APP: &str = "app:\n  API_KEY: \"k1\"\n  DB_URL: \"postgres://db\"\n";
const OTHER: &str = "other:\n  X: \"1\"\n";

/// Computer 1 sets up and links `app`; computer 2 (same user) pulls it.
fn two_linked_computers(local: &Local, tag: &str) -> (Computer, Computer) {
    let c1 = Computer::new(&format!("{tag}-1"), APP);
    let (email, _) = c1.set_up(local, tag);
    sync::link(&c1.home, &c1.unlocked(), "app", None).unwrap();
    let c2 = Computer::new(&format!("{tag}-2"), OTHER);
    c2.log_in(local, &email);
    assert_eq!(
        c2.pull(&mut NoConflicts),
        vec![PullResult::Added("app".to_string())]
    );
    (c1, c2)
}

#[test]
fn two_computers_push_and_pull() {
    let local = local_or_skip!();
    let (c1, c2) = two_linked_computers(&local, "two");
    assert_eq!(c2.vars("app"), c1.vars("app"));

    c1.write_master("app:\n  API_KEY: \"k2\"\n  DB_URL: \"postgres://db\"\n  NEW: \"n\"\n");
    assert!(matches!(c1.push()[..], [PushResult::Pushed { .. }]));
    let results = c2.pull(&mut NoConflicts);
    assert!(
        matches!(results[..], [PullResult::Updated { .. }]),
        "{results:?}"
    );
    assert_eq!(c2.vars("app"), c1.vars("app"));
}

#[test]
fn conflict_is_resolved_with_keep_cloud() {
    let local = local_or_skip!();
    let (c1, c2) = two_linked_computers(&local, "conflict");
    c1.write_master("app:\n  API_KEY: \"one\"\n  DB_URL: \"postgres://db\"\n");
    assert!(matches!(c1.push()[..], [PushResult::Pushed { .. }]));
    c2.write_master(&format!(
        "{OTHER}app:\n  API_KEY: \"two\"\n  DB_URL: \"postgres://db\"\n"
    ));
    let results = c2.pull(&mut KeepCloud);
    assert!(
        matches!(results[..], [PullResult::Updated { .. }]),
        "{results:?}"
    );
    assert_eq!(var(&c2.vars("app"), "API_KEY").as_deref(), Some("one"));
}

#[test]
fn push_refused_when_cloud_is_newer() {
    let local = local_or_skip!();
    let (c1, c2) = two_linked_computers(&local, "newer");
    c1.write_master("app:\n  API_KEY: \"from-1\"\n  DB_URL: \"postgres://db\"\n");
    assert!(matches!(c1.push()[..], [PushResult::Pushed { .. }]));
    c2.write_master(&format!(
        "{OTHER}app:\n  API_KEY: \"k1\"\n  DB_URL: \"postgres://other\"\n"
    ));
    assert_eq!(c2.push(), vec![PushResult::CloudIsNewer("app".to_string())]);
}

#[test]
fn server_rows_hold_no_plain_text() {
    let local = local_or_skip!();
    let c1 = Computer::new(
        "plain",
        "plainprojectname:\n  PLAINVARNAME: \"plainvaluetext\"\nplaingroupproject:\n  PLAINGROUPVAR: \"plaingroupvalue\"\n",
    );
    c1.set_up(&local, "plain");
    let u = c1.unlocked();
    sync::link(&c1.home, &u, "plainprojectname", None).unwrap();
    groups::create(&u, "plaingroupname").unwrap();
    sync::link(&c1.home, &u, "plaingroupproject", Some("plaingroupname")).unwrap();

    let projects = local.service_get("/rest/v1/projects?select=sealed");
    let vaults = local.service_get("/rest/v1/vaults?select=name_sealed");
    assert!(
        projects.contains("sealed"),
        "the service role reads the rows"
    );
    for needle in [
        "plainprojectname",
        "PLAINVARNAME",
        "plainvaluetext",
        "plaingroupproject",
        "PLAINGROUPVAR",
        "plaingroupvalue",
        "plaingroupname",
    ] {
        assert!(!projects.contains(needle), "projects hold '{needle}'");
        assert!(!vaults.contains(needle), "vaults hold '{needle}'");
    }
}

/// A owns group `team` with the project `shared`.
fn owner_with_group(local: &Local, tag: &str) -> (Computer, String) {
    let ca = Computer::new(&format!("{tag}-a"), "shared:\n  TOKEN: \"t1\"\n");
    ca.set_up(local, &format!("{tag}-a"));
    let u = ca.unlocked();
    let group_id = groups::create(&u, "team").unwrap();
    sync::link(&ca.home, &u, "shared", Some("team")).unwrap();
    (ca, group_id)
}

/// A invites the user, the user sets up on a new computer, A pulls (that
/// gives access) and the user pulls the group project.
fn join_group(local: &Local, owner: &Computer, tag: &str) -> (Computer, String) {
    let email = local.new_user(tag);
    groups::invite(&owner.unlocked(), "team", &email).unwrap();
    let c = Computer::new(tag, OTHER);
    c.set_up_as(local, &email);
    assert_eq!(groups::waiting(&c.unlocked()).unwrap(), 1);
    no_skips(&owner.pull(&mut NoConflicts));
    let results = c.pull(&mut NoConflicts);
    assert!(
        results.contains(&PullResult::Added("shared".to_string())),
        "{results:?}"
    );
    (c, email)
}

#[test]
fn invite_then_accept_on_pull() {
    let local = local_or_skip!();
    let (ca, _) = owner_with_group(&local, "invite");
    let (cb, _) = join_group(&local, &ca, "invite-b");
    assert_eq!(cb.vars("shared"), ca.vars("shared"));
    assert_eq!(groups::waiting(&cb.unlocked()).unwrap(), 0);
}

#[test]
fn remove_rotates_key() {
    let local = local_or_skip!();
    let (ca, group_id) = owner_with_group(&local, "remove");
    let (cb, b_email) = join_group(&local, &ca, "remove-b");
    let (cc, _) = join_group(&local, &ca, "remove-c");

    let ua = ca.unlocked();
    let vaults = Vaults::load(&ua).unwrap();
    assert_eq!(
        groups::remove(&ua, &vaults, "team", &b_email).unwrap(),
        Removal::Member { projects: 1 }
    );

    let ub = cb.unlocked();
    assert!(ub
        .api
        .projects(std::slice::from_ref(&group_id))
        .unwrap()
        .is_empty());
    no_skips(&ca.pull(&mut NoConflicts));
    no_skips(&cc.pull(&mut NoConflicts));
    assert_eq!(cc.vars("shared"), ca.vars("shared"));

    let vaults = Vaults::load(&ca.unlocked()).unwrap();
    let group = vaults
        .memberships()
        .iter()
        .find(|member| member.vault_id == group_id)
        .unwrap();
    assert_eq!(group.vaults.key_generation, 2);
}

#[test]
fn recover_sets_new_password() {
    let local = local_or_skip!();
    let c1 = Computer::new("recover", APP);
    let (_, code) = c1.set_up(&local, "recover");
    let mut answers = Answers {
        password: String::new(),
        new_password: NEW_PASSWORD.to_string(),
        code,
    };
    account::recover(
        &c1.home,
        &c1.store,
        &mut || Err("no browser in the tests".to_string()),
        &mut answers,
    )
    .unwrap();

    let profile = c1.unlocked().api.my_profile().unwrap().unwrap();
    assert!(account::unlock_with_password(&profile, NEW_PASSWORD).is_ok());
    assert!(account::unlock_with_password(&profile, PASSWORD).is_err());
}

#[test]
fn expired_access_token_is_refreshed() {
    let local = local_or_skip!();
    let c1 = Computer::new("expired", APP);
    let email = local.new_user("expired");
    let mut session = local.sign_in(&email);
    session.expires_at = now() - 3600;
    account::setup_with(&c1.home, &c1.store, &session, PASSWORD, PARAMS).unwrap();

    let u = c1.unlocked();
    assert_eq!(u.user_id, session.user_id);
    assert!(u.api.my_profile().unwrap().is_some());
}
