//! Supabase client: Auth (PKCE), PostgREST tables and RPC.

use super::config::Config;
use super::crypto::KdfParams;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

const CONNECT_ERROR: &str =
    "cannot connect to the Keyman server. Check the network connection, then try again.";
const DEADLOCK: &str = "40P01";

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub user_id: String,
    pub email: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
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
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad server reply: {e}"))?;
    if v.get("user_id").is_none_or(Value::is_null) {
        return Ok(None);
    }
    serde_json::from_value(v)
        .map(Some)
        .map_err(|e| format!("bad server reply: {e}"))
}

/// Read a token response. `now` is the current Unix time in seconds.
pub fn parse_session(json: &str, now: i64) -> Result<Session, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad server reply: {e}"))?;
    let text = |x: Option<&Value>| x.and_then(Value::as_str).map(str::to_string);
    let bad = || "bad server reply: the sign-in data is not complete".to_string();
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
        .header("apikey", &cfg.key)
        .header(
            "Authorization",
            format!("Bearer {}", token.unwrap_or(&cfg.key)),
        );
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
    let req = req.body(payload).map_err(|e| format!("bad request: {e}"))?;
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
        .unwrap_or_else(|| "the Keyman server returned an error".to_string());
    (pick("code").unwrap_or_default(), message)
}

/// The one place that sends requests. It retries once after a Postgres deadlock.
fn send(
    cfg: &Config,
    token: Option<&str>,
    method: &str,
    path: &str,
    prefer: Option<&str>,
    body: Option<&Value>,
) -> Result<String, String> {
    let mut retried = false;
    loop {
        let r = send_once(cfg, token, method, path, prefer, body)?;
        if r.status < 400 {
            return Ok(r.body);
        }
        let (code, message) = error_fields(&r.body);
        if code == DEADLOCK && !retried {
            retried = true;
            continue;
        }
        return Err(message);
    }
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

pub fn refresh(cfg: &Config, refresh_token: &str) -> Result<Session, String> {
    token_request(
        cfg,
        "/auth/v1/token?grant_type=refresh_token",
        json!({"refresh_token": refresh_token}),
    )
}

pub fn sign_out(cfg: &Config, session: &Session) -> Result<(), String> {
    send(
        cfg,
        Some(&session.access_token),
        "POST",
        "/auth/v1/logout",
        None,
        None,
    )
    .map(|_| ())
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(text).map_err(|e| format!("bad server reply: {e}"))
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
fn new_uuid() -> String {
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

    pub fn put_profile(&self, p: &Profile) -> Result<(), String> {
        let mut row = p.clone();
        row.email = row.email.to_lowercase();
        let body = serde_json::to_value(&row).map_err(|e| e.to_string())?;
        self.call(
            "POST",
            "/rest/v1/profiles?on_conflict=user_id",
            Some("resolution=merge-duplicates,return=minimal"),
            Some(&body),
        )
        .map(|_| ())
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

    pub fn create_vault(
        &self,
        kind: &str,
        name_sealed: Option<&str>,
        wrapped_key: &str,
    ) -> Result<String, String> {
        let text = self.rpc(
            "create_vault",
            json!({"p_kind": kind, "p_name_sealed": name_sealed, "p_wrapped_key": wrapped_key}),
        )?;
        parse(&text)
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
        rows.into_iter()
            .next()
            .ok_or_else(|| "bad server reply: no project returned".to_string())
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
    }
}
