//! Supabase client: Auth (PKCE), PostgREST tables and RPC.

use super::config::Config;
use super::crypto::KdfParams;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const CONNECT_ERROR: &str =
    "Keyman cannot connect to the Keyman server. Check the network connection, then try again.";
const DEADLOCK: &str = "40P01";
const BAD_REPLY: &str = "The Keyman server gave a reply that Keyman cannot use.";
const BAD_SIGN_IN: &str = "The sign-in data from the server is not complete.";
pub const PROFILE_EXISTS: &str =
    "Another computer set up your account now. Run 'keyman login' again.";
const PROFILE_NOT_SAVED: &str = "Keyman cannot save the new key data on the server. Try again.";

pub const INVITE_GONE: &str = "The invite does not exist now.";
pub const GROUP_KEY_CHANGED: &str = "The group key changed. Run the command again.";
const PERSON_CHANGED: &str =
    "A person joined or left the group during the removal. Run the command again.";

/// Server error texts that Keyman knows, as short sentences. The first match wins.
const KNOWN_ERRORS: &[(&str, &str)] = &[
    ("the invite does not exist", INVITE_GONE),
    ("the group key changed", GROUP_KEY_CHANGED),
    (
        "a project changed during the removal",
        "A member changed a project during the removal. Run the command again.",
    ),
    (
        "a new key is necessary for each remaining member",
        PERSON_CHANGED,
    ),
    ("a key is for a user who is not a member", PERSON_CHANGED),
    ("every remaining member must get a new key", PERSON_CHANGED),
    ("the user is not a member", PERSON_CHANGED),
    (
        "all projects of the vault must be encrypted again",
        "A member added or removed a project during the removal. Run the command again.",
    ),
    (
        "the key generation is out of date",
        "The group key changed. Run 'keyman pull' again.",
    ),
    (
        "not a member of this vault",
        "You are not a member of this vault now. Run 'keyman pull'.",
    ),
    (
        "only the owner can remove a member",
        "Only the owner of the group can remove persons.",
    ),
    (
        "the owner cannot be removed",
        "The owner cannot be removed. Delete the group instead.",
    ),
    (
        "only a member can accept an invite",
        "Only a member of the group can give access.",
    ),
    (
        "the user does not match the invite email",
        "The invite is for a different email address.",
    ),
    (
        "only a group accepts invites",
        "Only a group accepts invites.",
    ),
    (
        "the public key cannot change",
        "The key on the server cannot change. Keyman stops.",
    ),
    (
        "a project cannot move to a different vault",
        "A project cannot move to a different vault.",
    ),
    (
        "sign in first",
        "You are not signed in. Run 'keyman login' first.",
    ),
    (
        "JWT expired",
        "Your sign-in is not valid now. Run 'keyman login'.",
    ),
];

/// The most characters of an unknown server text that Keyman shows.
const MAX_SERVER_TEXT: usize = 200;

/// A server error as text that is safe and short for the terminal. Known
/// errors become fixed sentences. Other text loses its control and format
/// characters and is cut to `MAX_SERVER_TEXT` characters.
pub fn server_message(code: &str, message: &str) -> String {
    if let Some((_, known)) = KNOWN_ERRORS
        .iter()
        .find(|(server, _)| message.contains(server))
    {
        return known.to_string();
    }
    match code {
        "42501" => return "The server does not allow this change.".to_string(),
        "PGRST301" => return "Your sign-in is not valid now. Run 'keyman login'.".to_string(),
        "23505" => return "The cloud has this item already.".to_string(),
        _ => {}
    }
    let spaced: String = message
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() {
                Some(' ')
            } else if crate::ui::is_hidden(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let clean = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return "The Keyman server gave an error.".to_string();
    }
    if clean.chars().count() > MAX_SERVER_TEXT {
        let cut: String = clean.chars().take(MAX_SERVER_TEXT).collect();
        return format!("The Keyman server gave an error: {cut}...");
    }
    let end = if clean.ends_with('.') { "" } else { "." };
    format!("The Keyman server gave an error: {clean}{end}")
}

/// A second insert of one row: HTTP 409, or the Postgres unique violation.
pub(crate) fn is_conflict(status: u16, code: &str) -> bool {
    status == 409 || code == "23505"
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub user_id: String,
    pub email: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub user_id: String,
    pub email: String,
    pub public_key: String,
    pub kdf_salt: String,
    pub kdf_params: KdfParams,
    pub private_key_pw: String,
    pub private_key_recovery: String,
}

/// The columns that other users may read.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PublicProfile {
    pub user_id: String,
    pub email: String,
    pub public_key: String,
}

#[derive(Clone, Deserialize, Debug)]
pub struct VaultRow {
    pub kind: String,
    pub owner_id: String,
    pub name_sealed: Option<String>,
    pub key_generation: i64,
}

#[derive(Clone, Deserialize, Debug)]
pub struct Membership {
    pub vault_id: String,
    pub user_id: String,
    pub role: String,
    pub wrapped_key: String,
    pub key_generation: i64,
    pub vaults: VaultRow,
}

#[derive(Clone, Deserialize, Debug)]
pub struct CloudProject {
    pub id: String,
    pub vault_id: String,
    pub version: i64,
    pub key_generation: i64,
    pub sealed: String,
    pub updated_at: String,
}

#[derive(Clone, Deserialize, Debug)]
pub struct Invite {
    pub id: String,
    pub vault_id: String,
    pub email: String,
}

/// Percent-encode a string for a URL query value.
pub(crate) fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Read the caller's profile from the `my_profile` RPC reply.
/// A caller with no profile gets `null` or an object of null fields.
pub fn parse_my_profile(json: &str) -> Result<Option<Profile>, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| BAD_REPLY.to_string())?;
    if v.get("user_id").is_none_or(Value::is_null) {
        return Ok(None);
    }
    serde_json::from_value(v)
        .map(Some)
        .map_err(|_| BAD_REPLY.to_string())
}

/// Read a token response. `now` is the current Unix time in seconds.
pub fn parse_session(json: &str, now: i64) -> Result<Session, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| BAD_REPLY.to_string())?;
    let text = |x: Option<&Value>| x.and_then(Value::as_str).map(str::to_string);
    let bad = || BAD_SIGN_IN.to_string();
    let expires_at = v
        .get("expires_at")
        .and_then(Value::as_i64)
        .or_else(|| v.get("expires_in").and_then(Value::as_i64).map(|n| now + n))
        .ok_or_else(bad)?;
    Ok(Session {
        access_token: text(v.get("access_token")).ok_or_else(bad)?,
        refresh_token: text(v.get("refresh_token")).ok_or_else(bad)?,
        expires_at,
        user_id: text(v.pointer("/user/id")).ok_or_else(bad)?,
        email: text(v.pointer("/user/email"))
            .ok_or_else(bad)?
            .to_lowercase(),
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .build(),
    )
}

struct Reply {
    status: u16,
    body: String,
}

fn send_once(
    cfg: &Config,
    token: Option<&str>,
    method: &str,
    path: &str,
    prefer: Option<&str>,
    body: Option<&Value>,
) -> Result<Reply, String> {
    let mut req = ureq::http::Request::builder()
        .method(method)
        .uri(format!("{}{}", cfg.url, path))
        .header("apikey", &cfg.key);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if let Some(p) = prefer {
        req = req.header("Prefer", p);
    }
    let payload = match body {
        Some(b) => {
            req = req.header("Content-Type", "application/json");
            b.to_string()
        }
        None => String::new(),
    };
    let req = req
        .body(payload)
        .map_err(|_| "Keyman cannot make the request.".to_string())?;
    let mut resp = agent().run(req).map_err(|_| CONNECT_ERROR.to_string())?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|_| CONNECT_ERROR.to_string())?;
    Ok(Reply { status, body })
}

fn error_fields(body: &str) -> (String, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let pick = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let message = ["message", "msg", "error_description", "error"]
        .iter()
        .find_map(|k| pick(k))
        .unwrap_or_default();
    let code = pick("code").or_else(|| pick("error_code"));
    (code.unwrap_or_default(), message)
}

/// A request that did not work. `status` is 0 when no reply came.
struct Failure {
    status: u16,
    code: String,
    /// Safe to show (see `server_message`).
    message: String,
}

/// The one place that sends requests. It retries once after a Postgres deadlock.
fn request(
    cfg: &Config,
    token: Option<&str>,
    method: &str,
    path: &str,
    prefer: Option<&str>,
    body: Option<&Value>,
) -> Result<String, Failure> {
    let mut retried = false;
    loop {
        let r = send_once(cfg, token, method, path, prefer, body).map_err(|message| Failure {
            status: 0,
            code: String::new(),
            message,
        })?;
        if r.status < 400 {
            return Ok(r.body);
        }
        let (code, message) = error_fields(&r.body);
        if code == DEADLOCK && !retried {
            retried = true;
            continue;
        }
        return Err(Failure {
            status: r.status,
            message: server_message(&code, &message),
            code,
        });
    }
}

fn send(
    cfg: &Config,
    token: Option<&str>,
    method: &str,
    path: &str,
    prefer: Option<&str>,
    body: Option<&Value>,
) -> Result<String, String> {
    request(cfg, token, method, path, prefer, body).map_err(|failure| failure.message)
}

fn token_request(cfg: &Config, path: &str, body: Value) -> Result<Session, String> {
    let text = send(cfg, None, "POST", path, None, Some(&body))?;
    parse_session(&text, now())
}

pub fn exchange_code(cfg: &Config, code: &str, verifier: &str) -> Result<Session, String> {
    token_request(
        cfg,
        "/auth/v1/token?grant_type=pkce",
        json!({"auth_code": code, "code_verifier": verifier}),
    )
}

/// Why a refresh did not work.
#[derive(Debug, PartialEq)]
pub enum RefreshError {
    /// The server does not accept the refresh token. The sign-in is over.
    Rejected,
    /// A network problem, a server problem, or a reply Keyman cannot read.
    /// The refresh token may still be good.
    Unavailable(String),
}

const UNAVAILABLE: &str = "The Keyman server is not available now. Try again later.";

/// Sort a failed refresh reply. Only a clear "bad refresh token" answer
/// (HTTP 400, 401 or 403 with a known error code) is `Rejected`.
pub fn classify_refresh_failure(status: u16, body: &str) -> RefreshError {
    const REJECTED: [&str; 7] = [
        "invalid_grant",
        "refresh_token_not_found",
        "refresh_token_already_used",
        "session_expired",
        "session_not_found",
        "user_not_found",
        "user_banned",
    ];
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let rejected = [400, 401, 403].contains(&status)
        && ["error", "error_code", "code"].iter().any(|key| {
            v.get(key)
                .and_then(Value::as_str)
                .is_some_and(|code| REJECTED.contains(&code))
        });
    if rejected {
        RefreshError::Rejected
    } else {
        RefreshError::Unavailable(UNAVAILABLE.to_string())
    }
}

pub fn refresh(cfg: &Config, refresh_token: &str) -> Result<Session, RefreshError> {
    let body = json!({"refresh_token": refresh_token});
    let reply = send_once(
        cfg,
        None,
        "POST",
        "/auth/v1/token?grant_type=refresh_token",
        None,
        Some(&body),
    )
    .map_err(RefreshError::Unavailable)?;
    if reply.status >= 400 {
        return Err(classify_refresh_failure(reply.status, &reply.body));
    }
    parse_session(&reply.body, now())
        .map_err(|_| RefreshError::Unavailable(UNAVAILABLE.to_string()))
}

pub fn sign_out(cfg: &Config, session: &Session) -> Result<(), String> {
    send(
        cfg,
        Some(&session.access_token),
        "POST",
        "/auth/v1/logout?scope=local",
        None,
        None,
    )
    .map(|_| ())
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|_| BAD_REPLY.to_string())
}

fn in_list(ids: &[String]) -> String {
    let joined: Vec<String> = ids.iter().map(|i| urlenc(i)).collect();
    format!("in.({})", joined.join(","))
}

const MEMBER_SELECT: &str = "select=vault_id,user_id,role,wrapped_key,key_generation,vaults(kind,owner_id,name_sealed,key_generation)";
const PUBLIC_SELECT: &str = "select=user_id,email,public_key";
const PROJECT_SELECT: &str = "select=id,vault_id,version,key_generation,sealed,updated_at";
const INVITE_SELECT: &str = "select=id,vault_id,email";

/// A random version 4 UUID.
pub fn new_uuid() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("system random source failed");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

pub struct Api {
    cfg: Config,
    session: Session,
}

impl Api {
    pub fn new(cfg: Config, session: Session) -> Api {
        Api { cfg, session }
    }

    pub fn user_id(&self) -> &str {
        &self.session.user_id
    }

    pub fn email(&self) -> &str {
        &self.session.email
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        prefer: Option<&str>,
        body: Option<&Value>,
    ) -> Result<String, String> {
        send(
            &self.cfg,
            Some(&self.session.access_token),
            method,
            path,
            prefer,
            body,
        )
    }

    fn get(&self, path: &str) -> Result<String, String> {
        self.call("GET", path, None, None)
    }

    fn rpc(&self, name: &str, args: Value) -> Result<String, String> {
        self.call("POST", &format!("/rest/v1/rpc/{name}"), None, Some(&args))
    }

    pub fn my_profile(&self) -> Result<Option<Profile>, String> {
        parse_my_profile(&self.rpc("my_profile", json!({}))?)
    }

    pub fn find_profile_by_email(&self, email: &str) -> Result<Option<PublicProfile>, String> {
        let path = format!(
            "/rest/v1/profiles?{PUBLIC_SELECT}&email=eq.{}",
            urlenc(&email.to_lowercase())
        );
        let rows: Vec<PublicProfile> = parse(&self.get(&path)?)?;
        Ok(rows.into_iter().next())
    }

    pub fn public_profiles(&self, user_ids: &[String]) -> Result<Vec<PublicProfile>, String> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let path = format!(
            "/rest/v1/profiles?{PUBLIC_SELECT}&user_id={}",
            in_list(user_ids)
        );
        parse(&self.get(&path)?)
    }

    /// Adds the profile of a new account. It never replaces a profile: when
    /// another computer was first, the answer is `PROFILE_EXISTS`.
    pub fn insert_profile(&self, p: &Profile) -> Result<(), String> {
        let mut row = p.clone();
        row.email = row.email.to_lowercase();
        let body = serde_json::to_value(&row).map_err(|_| BAD_REPLY.to_string())?;
        request(
            &self.cfg,
            Some(&self.session.access_token),
            "POST",
            "/rest/v1/profiles",
            Some("return=minimal"),
            Some(&body),
        )
        .map(|_| ())
        .map_err(|failure| {
            if is_conflict(failure.status, &failure.code) {
                PROFILE_EXISTS.to_string()
            } else {
                failure.message
            }
        })
    }

    /// Saves new sealed copies of the private key (after a recovery or a
    /// password change). The public key and the email do not change. The
    /// server ignores an update it does not allow, so the profile is read
    /// again to check it.
    pub fn update_profile_keys(&self, p: &Profile) -> Result<(), String> {
        let path = format!(
            "/rest/v1/profiles?user_id=eq.{}",
            urlenc(&self.session.user_id)
        );
        let body = json!({
            "kdf_salt": p.kdf_salt,
            "kdf_params": p.kdf_params,
            "private_key_pw": p.private_key_pw,
            "private_key_recovery": p.private_key_recovery,
        });
        self.call("PATCH", &path, Some("return=minimal"), Some(&body))?;
        let saved = self
            .my_profile()?
            .ok_or_else(|| PROFILE_NOT_SAVED.to_string())?;
        let same = saved.kdf_salt == p.kdf_salt
            && saved.kdf_params == p.kdf_params
            && saved.private_key_pw == p.private_key_pw
            && saved.private_key_recovery == p.private_key_recovery;
        if same {
            Ok(())
        } else {
            Err(PROFILE_NOT_SAVED.to_string())
        }
    }

    pub fn my_memberships(&self) -> Result<Vec<Membership>, String> {
        let path = format!(
            "/rest/v1/vault_members?{MEMBER_SELECT}&user_id=eq.{}",
            urlenc(&self.session.user_id)
        );
        parse(&self.get(&path)?)
    }

    pub fn vault_members(&self, vault_id: &str) -> Result<Vec<Membership>, String> {
        let path = format!(
            "/rest/v1/vault_members?{MEMBER_SELECT}&vault_id=eq.{}",
            urlenc(vault_id)
        );
        parse(&self.get(&path)?)
    }

    /// Makes a vault with the ID `id` (made on the client, so the wrapped
    /// key can be bound to it before the call).
    pub fn create_vault(
        &self,
        id: &str,
        kind: &str,
        name_sealed: Option<&str>,
        wrapped_key: &str,
    ) -> Result<(), String> {
        let text = self.rpc(
            "create_vault",
            json!({
                "p_id": id,
                "p_kind": kind,
                "p_name_sealed": name_sealed,
                "p_wrapped_key": wrapped_key,
            }),
        )?;
        let made: String = parse(&text)?;
        if made != id {
            return Err(BAD_REPLY.to_string());
        }
        Ok(())
    }

    /// Members of several vaults in one request.
    pub fn members_for_vaults(&self, vault_ids: &[String]) -> Result<Vec<Membership>, String> {
        if vault_ids.is_empty() {
            return Ok(Vec::new());
        }
        let path = format!(
            "/rest/v1/vault_members?{MEMBER_SELECT}&vault_id={}",
            in_list(vault_ids)
        );
        parse(&self.get(&path)?)
    }

    /// Only the owner of a group can set the name. The server ignores an
    /// update that it does not allow, so no row back is an error.
    pub fn set_vault_name(&self, vault_id: &str, name_sealed: &str) -> Result<(), String> {
        let path = format!("/rest/v1/vaults?id=eq.{}&select=id", urlenc(vault_id));
        let body = json!({"name_sealed": name_sealed});
        let text = self.call("PATCH", &path, Some("return=representation"), Some(&body))?;
        let rows: Vec<Value> = parse(&text)?;
        if rows.is_empty() {
            return Err("Keyman cannot save the name of the group.".to_string());
        }
        Ok(())
    }

    pub fn delete_vault(&self, vault_id: &str) -> Result<(), String> {
        let path = format!("/rest/v1/vaults?id=eq.{}", urlenc(vault_id));
        self.call("DELETE", &path, None, None).map(|_| ())
    }

    pub fn projects(&self, vault_ids: &[String]) -> Result<Vec<CloudProject>, String> {
        if vault_ids.is_empty() {
            return Ok(Vec::new());
        }
        let path = format!(
            "/rest/v1/projects?{PROJECT_SELECT}&vault_id={}",
            in_list(vault_ids)
        );
        parse(&self.get(&path)?)
    }

    pub fn project(&self, id: &str) -> Result<Option<CloudProject>, String> {
        let path = format!("/rest/v1/projects?{PROJECT_SELECT}&id=eq.{}", urlenc(id));
        let rows: Vec<CloudProject> = parse(&self.get(&path)?)?;
        Ok(rows.into_iter().next())
    }

    /// Create the project ID on the client, so the sealed text can include it.
    pub fn insert_project(
        &self,
        vault_id: &str,
        key_generation: i64,
        sealed_for: impl Fn(&str) -> String,
    ) -> Result<CloudProject, String> {
        let id = new_uuid();
        let body = json!({
            "id": id,
            "vault_id": vault_id,
            "key_generation": key_generation,
            "sealed": sealed_for(&id),
        });
        let text = self.call(
            "POST",
            "/rest/v1/projects",
            Some("return=representation"),
            Some(&body),
        )?;
        let rows: Vec<CloudProject> = parse(&text)?;
        rows.into_iter().next().ok_or_else(|| BAD_REPLY.to_string())
    }

    /// `Ok(None)` means the cloud copy is newer than `base_version`.
    pub fn update_project(
        &self,
        id: &str,
        base_version: i64,
        key_generation: i64,
        sealed: &str,
    ) -> Result<Option<CloudProject>, String> {
        let path = format!(
            "/rest/v1/projects?id=eq.{}&version=eq.{base_version}",
            urlenc(id)
        );
        let body = json!({"key_generation": key_generation, "sealed": sealed});
        let text = self.call("PATCH", &path, Some("return=representation"), Some(&body))?;
        let rows: Vec<CloudProject> = parse(&text)?;
        Ok(rows.into_iter().next())
    }

    /// Only the owner of the group can delete an invite. The server ignores
    /// a delete that it does not allow, so the caller must read again.
    pub fn delete_invite(&self, invite_id: &str) -> Result<(), String> {
        let path = format!("/rest/v1/invites?id=eq.{}", urlenc(invite_id));
        self.call("DELETE", &path, None, None).map(|_| ())
    }

    pub fn delete_project(&self, id: &str) -> Result<(), String> {
        let path = format!("/rest/v1/projects?id=eq.{}", urlenc(id));
        self.call("DELETE", &path, None, None).map(|_| ())
    }

    pub fn invites_for_vaults(&self, vault_ids: &[String]) -> Result<Vec<Invite>, String> {
        if vault_ids.is_empty() {
            return Ok(Vec::new());
        }
        let path = format!(
            "/rest/v1/invites?{INVITE_SELECT}&vault_id={}",
            in_list(vault_ids)
        );
        parse(&self.get(&path)?)
    }

    pub fn my_invites(&self) -> Result<Vec<Invite>, String> {
        let path = format!(
            "/rest/v1/invites?{INVITE_SELECT}&email=eq.{}",
            urlenc(&self.session.email.to_lowercase())
        );
        parse(&self.get(&path)?)
    }

    pub fn create_invite(&self, vault_id: &str, email: &str) -> Result<(), String> {
        let body = json!({
            "vault_id": vault_id,
            "email": email.to_lowercase(),
            "invited_by": self.session.user_id,
        });
        self.call(
            "POST",
            "/rest/v1/invites",
            Some("return=minimal"),
            Some(&body),
        )
        .map(|_| ())
    }

    pub fn accept_invite(
        &self,
        invite_id: &str,
        user_id: &str,
        wrapped_key: &str,
        generation: i64,
    ) -> Result<(), String> {
        self.rpc(
            "accept_invite",
            json!({
                "p_invite": invite_id,
                "p_user": user_id,
                "p_wrapped_key": wrapped_key,
                "p_generation": generation,
            }),
        )
        .map(|_| ())
    }

    /// Remove a member and re-key the vault. `keys` are `(user_id, wrapped_key)`;
    /// `projects` are `(id, base_version, sealed)`. Returns the new key generation.
    pub fn rotate_vault(
        &self,
        vault_id: &str,
        remove: &str,
        keys: &[(String, String)],
        projects: &[(String, i64, String)],
        name_sealed: Option<&str>,
    ) -> Result<i64, String> {
        let keys: Vec<Value> = keys
            .iter()
            .map(|(u, w)| json!({"user_id": u, "wrapped_key": w}))
            .collect();
        let projects: Vec<Value> = projects
            .iter()
            .map(|(id, v, s)| json!({"id": id, "version": v, "sealed": s}))
            .collect();
        let text = self.rpc(
            "rotate_vault",
            json!({
                "p_vault": vault_id,
                "p_remove": remove,
                "p_keys": keys,
                "p_projects": projects,
                "p_name_sealed": name_sealed,
            }),
        )?;
        parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{"user_id":"u1","email":"a@b.c","public_key":"pk","kdf_salt":"s",
        "kdf_params":{"m":65536,"t":3,"p":1},"private_key_pw":"x","private_key_recovery":"y",
        "created_at":"2026-01-01T00:00:00Z"}"#;

    #[test]
    fn my_profile_full_row() {
        let p = parse_my_profile(FULL).unwrap().unwrap();
        assert_eq!(p.user_id, "u1");
        assert_eq!(p.kdf_params.m, 65536);
    }

    #[test]
    fn my_profile_all_null_is_none() {
        let j = r#"{"user_id":null,"email":null,"public_key":null,"kdf_salt":null,
            "kdf_params":null,"private_key_pw":null,"private_key_recovery":null,"created_at":null}"#;
        assert!(parse_my_profile(j).unwrap().is_none());
    }

    #[test]
    fn my_profile_null_is_none() {
        assert!(parse_my_profile("null").unwrap().is_none());
    }

    #[test]
    fn session_uses_expires_at_when_present() {
        let j = r#"{"access_token":"a","refresh_token":"r","expires_in":3600,"expires_at":5000,
            "user":{"id":"u","email":"A@B.C"}}"#;
        let s = parse_session(j, 100).unwrap();
        assert_eq!(s.expires_at, 5000);
        assert_eq!(s.user_id, "u");
        assert_eq!(s.email, "a@b.c");
    }

    #[test]
    fn session_falls_back_to_expires_in() {
        let j = r#"{"access_token":"a","refresh_token":"r","expires_in":3600,
            "user":{"id":"u","email":"a@b.c"}}"#;
        assert_eq!(parse_session(j, 100).unwrap().expires_at, 3700);
    }

    #[test]
    fn uuid_is_v4_format() {
        let u = new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!("89ab".contains(&u[19..20]));
    }

    #[test]
    fn error_fields_reads_code_and_message() {
        let (c, m) = error_fields(r#"{"code":"40P01","message":"deadlock detected"}"#);
        assert_eq!((c.as_str(), m.as_str()), ("40P01", "deadlock detected"));
        assert_eq!(error_fields(r#"{"code":400,"msg":"bad"}"#).1, "bad");
        assert_eq!(
            error_fields(r#"{"code":400,"error_code":"user_banned","msg":"x"}"#).0,
            "user_banned"
        );
    }

    #[test]
    fn known_server_errors_become_short_sentences() {
        assert_eq!(server_message("", "the invite does not exist"), INVITE_GONE);
        assert_eq!(
            server_message("P0001", "the group key changed; run the command again"),
            GROUP_KEY_CHANGED
        );
        assert_eq!(
            server_message("", "a project changed during the removal; try again"),
            "A member changed a project during the removal. Run the command again."
        );
        for message in [
            "a new key is necessary for each remaining member",
            "a key is for a user who is not a member",
            "every remaining member must get a new key",
            "the user is not a member",
        ] {
            assert_eq!(
                server_message("P0001", message),
                "A person joined or left the group during the removal. Run the command again."
            );
        }
        assert_eq!(
            server_message("", "all projects of the vault must be encrypted again"),
            "A member added or removed a project during the removal. Run the command again."
        );
        assert_eq!(
            server_message("42501", "new row violates row-level security policy"),
            "The server does not allow this change."
        );
        assert_eq!(
            server_message("PGRST301", "JWT expired"),
            "Your sign-in is not valid now. Run 'keyman login'."
        );
    }

    #[test]
    fn unknown_server_errors_are_cleaned() {
        assert_eq!(
            server_message("", "boom\u{1b}[2J\u{202E}  now"),
            "The Keyman server gave an error: boom[2J now."
        );
        let long = "x".repeat(500);
        let shown = server_message("", &long);
        assert!(shown.chars().count() < 260, "{shown}");
        assert!(shown.ends_with("..."), "{shown}");
        assert_eq!(server_message("", ""), "The Keyman server gave an error.");
    }

    #[test]
    fn a_second_profile_insert_is_a_conflict() {
        assert!(is_conflict(409, ""));
        assert!(is_conflict(400, "23505"));
        assert!(!is_conflict(403, "42501"));
        assert!(!is_conflict(500, ""));
    }

    #[test]
    fn refresh_failures_are_classified() {
        let c = classify_refresh_failure;
        let keep = |e: RefreshError| matches!(e, RefreshError::Unavailable(_));
        assert_eq!(
            c(400, r#"{"error":"invalid_grant"}"#),
            RefreshError::Rejected
        );
        assert_eq!(
            c(
                400,
                r#"{"code":400,"error_code":"refresh_token_already_used","msg":"x"}"#
            ),
            RefreshError::Rejected
        );
        assert_eq!(
            c(401, r#"{"error_code":"refresh_token_not_found"}"#),
            RefreshError::Rejected
        );
        for code in [
            "session_expired",
            "session_not_found",
            "user_not_found",
            "user_banned",
        ] {
            for status in [400, 401, 403] {
                assert_eq!(
                    c(
                        status,
                        &format!(r#"{{"code":{status},"error_code":"{code}","msg":"x"}}"#)
                    ),
                    RefreshError::Rejected,
                    "{code} {status}"
                );
            }
            assert!(keep(c(503, &format!(r#"{{"error_code":"{code}"}}"#))));
            assert!(keep(c(429, &format!(r#"{{"error_code":"{code}"}}"#))));
        }
        assert!(keep(c(503, "<html>down</html>")));
        assert!(keep(c(503, r#"{"error":"invalid_grant"}"#)));
        assert!(keep(c(429, r#"{"error_code":"over_request_rate_limit"}"#)));
        assert!(keep(c(500, "")));
        assert!(keep(c(400, r#"{"error":"other"}"#)));
    }
}
