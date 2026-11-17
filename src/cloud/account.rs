//! The cloud account: sign in, set up keys, unlock, recover, change password.

use super::api::{self, Api, Profile, Session};
use super::auth;
use super::config::config;
use super::crypto::{self, KdfParams, Key, KeyPair};
use super::keystore::Store;
use super::state;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::Zeroizing;

const WRONG_PASSWORD: &str = "The vault password is not correct.";
const WRONG_RECOVERY: &str = "The recovery code is not correct.";
const NOT_SIGNED_IN: &str = "You are not signed in. Run 'keyman login' first.";
const NO_PROFILE: &str = "This account has no vault password yet. Run 'keyman login' first.";
const BAD_PROFILE: &str = "The key data on the server is not valid. Keyman stops.";
const KEY_MISMATCH: &str = "The key on the server does not match. Keyman stops.";
const BAD_LOCAL_KEY: &str = "The key on this computer is not valid. Run 'keyman login' again.";
const CODE_MISMATCH: &str =
    "The code is not the same. Nothing is saved. Run 'keyman login' to start again.";
const PRIVATE_KEY: &str = "private_key";
const SESSION: &str = "session";
const PASSWORD_TRIES: usize = 3;
const CODE_TRIES: usize = 3;

pub struct Unlocked {
    pub api: Api,
    pub pair: KeyPair,
    pub user_id: String,
}

/// The questions Keyman asks. The terminal and tests implement it.
pub trait Prompt {
    fn new_password(&mut self) -> Result<String, String>;
    fn password(&mut self) -> Result<String, String>;
    fn recovery_code(&mut self) -> Result<String, String>;
    fn show_recovery_code(&mut self, code: &str) -> Result<(), String>;
}

#[derive(Debug, PartialEq)]
pub enum LoginOutcome {
    Created,
    Unlocked,
}

/// What the credential store keeps for the session. The access token is not
/// kept: it is large, and a refresh gives a new one.
#[derive(Serialize, Deserialize)]
struct StoredSession {
    refresh_token: String,
    user_id: String,
    email: String,
}

fn decode_salt(profile: &Profile) -> Result<[u8; 16], String> {
    STANDARD
        .decode(&profile.kdf_salt)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| BAD_PROFILE.to_string())
}

fn seal_private_key(key: &Key, user_id: &str, pair: &KeyPair) -> String {
    let text = Zeroizing::new(pair.secret.to_base64());
    crypto::seal(key, &crypto::private_key_aad(user_id), text.as_bytes())
}

/// Open one sealed copy of the private key and check it against the public key.
fn open_private_key(
    key: &Key,
    profile: &Profile,
    sealed: &str,
    wrong: &str,
) -> Result<KeyPair, String> {
    let plain = crypto::open(key, &crypto::private_key_aad(&profile.user_id), sealed)
        .map_err(|_| wrong.to_string())?;
    let text = std::str::from_utf8(&plain).map_err(|_| BAD_PROFILE.to_string())?;
    let pair = KeyPair::from_secret(Key::from_base64(text).map_err(|_| BAD_PROFILE.to_string())?);
    let expected = STANDARD
        .decode(&profile.public_key)
        .map_err(|_| BAD_PROFILE.to_string())?;
    if expected != pair.public {
        return Err(KEY_MISMATCH.to_string());
    }
    Ok(pair)
}

/// Make the profile values for a new account (spec 7.2, steps 1 to 6).
pub fn new_profile_values(
    user_id: &str,
    email: &str,
    password: &str,
    recovery: &str,
    params: KdfParams,
) -> Result<(Profile, KeyPair), String> {
    let pair = KeyPair::generate();
    let salt = crypto::new_salt();
    let password_key = crypto::derive_key(password, &salt, params)?;
    let recovery_key =
        crypto::derive_key(&crypto::normalize_recovery_code(recovery), &salt, params)?;
    let profile = Profile {
        user_id: user_id.to_string(),
        email: email.to_lowercase(),
        public_key: STANDARD.encode(pair.public),
        kdf_salt: STANDARD.encode(salt),
        kdf_params: params,
        private_key_pw: seal_private_key(&password_key, user_id, &pair),
        private_key_recovery: seal_private_key(&recovery_key, user_id, &pair),
    };
    Ok((profile, pair))
}

/// Seal the password copy again with the same salt. The recovery copy does
/// not change. The settings in `profile` must be checked before this call.
pub fn reseal_password(
    profile: &Profile,
    pair: &KeyPair,
    new_password: &str,
) -> Result<Profile, String> {
    let salt = decode_salt(profile)?;
    let key = crypto::derive_key(new_password, &salt, profile.kdf_params)?;
    let mut updated = profile.clone();
    updated.private_key_pw = seal_private_key(&key, &profile.user_id, pair);
    Ok(updated)
}

/// Seal both copies again with a new salt (used after recovery).
fn reseal_all(
    profile: &Profile,
    pair: &KeyPair,
    new_password: &str,
    code: &str,
) -> Result<Profile, String> {
    let salt = crypto::new_salt();
    let params = profile.kdf_params;
    let password_key = crypto::derive_key(new_password, &salt, params)?;
    let recovery_key = crypto::derive_key(&crypto::normalize_recovery_code(code), &salt, params)?;
    let mut updated = profile.clone();
    updated.kdf_salt = STANDARD.encode(salt);
    updated.private_key_pw = seal_private_key(&password_key, &profile.user_id, pair);
    updated.private_key_recovery = seal_private_key(&recovery_key, &profile.user_id, pair);
    Ok(updated)
}

/// Unlock without the settings check. The public functions check first.
fn open_with_password(profile: &Profile, password: &str) -> Result<KeyPair, String> {
    let salt = decode_salt(profile)?;
    let key = crypto::derive_key(password, &salt, profile.kdf_params)?;
    open_private_key(&key, profile, &profile.private_key_pw, WRONG_PASSWORD)
}

fn open_with_recovery(profile: &Profile, code: &str) -> Result<KeyPair, String> {
    let salt = decode_salt(profile)?;
    let key = crypto::derive_key(
        &crypto::normalize_recovery_code(code),
        &salt,
        profile.kdf_params,
    )?;
    open_private_key(&key, profile, &profile.private_key_recovery, WRONG_RECOVERY)
}

pub fn unlock_with_password(profile: &Profile, password: &str) -> Result<KeyPair, String> {
    profile.kdf_params.check()?;
    open_with_password(profile, password)
}

pub fn unlock_with_recovery(profile: &Profile, code: &str) -> Result<KeyPair, String> {
    profile.kdf_params.check()?;
    open_with_recovery(profile, code)
}

fn remember_key(store: &dyn Store, pair: &KeyPair) -> Result<(), String> {
    if store.available() {
        let text = Zeroizing::new(pair.secret.to_base64());
        store.set(PRIVATE_KEY, &text)?;
    }
    Ok(())
}

fn create_personal_vault(api: &Api, pair: &KeyPair) -> Result<(), String> {
    let vault_key = Key::random();
    api.create_vault(
        "personal",
        None,
        &crypto::wrap_for(&pair.public, &vault_key),
    )?;
    Ok(())
}

/// First sign-in: make the keys, the profile and the personal vault.
pub fn setup_profile(
    api: &Api,
    store: &dyn Store,
    prompt: &mut dyn Prompt,
    params: KdfParams,
) -> Result<KeyPair, String> {
    let password = Zeroizing::new(prompt.new_password()?);
    let code = Zeroizing::new(crypto::recovery_code());
    let (profile, pair) = new_profile_values(api.user_id(), api.email(), &password, &code, params)?;
    prompt.show_recovery_code(&code)?;
    let wanted = crypto::normalize_recovery_code(&code);
    let mut confirmed = false;
    for _ in 0..CODE_TRIES {
        let typed = Zeroizing::new(prompt.recovery_code()?);
        if crypto::normalize_recovery_code(&typed) == wanted {
            confirmed = true;
            break;
        }
    }
    if !confirmed {
        return Err(CODE_MISMATCH.to_string());
    }
    api.put_profile(&profile)?;
    create_personal_vault(api, &pair)?;
    remember_key(store, &pair)?;
    Ok(pair)
}

/// Save the refresh token (store, or the state file when there is no store)
/// and the account name.
fn store_session(home: &Path, store: &dyn Store, session: &Session) -> Result<(), String> {
    let mut st = state::load(home)?;
    st.user_id = Some(session.user_id.clone());
    st.email = Some(session.email.clone());
    if store.available() {
        let stored = StoredSession {
            refresh_token: session.refresh_token.clone(),
            user_id: session.user_id.clone(),
            email: session.email.clone(),
        };
        let json = Zeroizing::new(serde_json::to_string(&stored).map_err(|e| e.to_string())?);
        store.set(SESSION, &json)?;
        st.refresh_token = None;
    } else {
        st.refresh_token = Some(session.refresh_token.clone());
    }
    state::save(home, &st)
}

fn saved_refresh_token(store: &dyn Store, st: &state::State) -> Result<Option<String>, String> {
    if store.available() {
        Ok(store
            .get(SESSION)?
            .and_then(|json| serde_json::from_str::<StoredSession>(&json).ok())
            .map(|stored| stored.refresh_token))
    } else {
        Ok(st.refresh_token.clone())
    }
}

fn unlock_from_server(api: &Api, prompt: &mut dyn Prompt) -> Result<KeyPair, String> {
    let profile = api.my_profile()?.ok_or_else(|| NO_PROFILE.to_string())?;
    let password = Zeroizing::new(prompt.password()?);
    unlock_with_password(&profile, &password)
}

/// A setup that stopped half way can leave a profile with no vault.
fn ensure_personal_vault(api: &Api, pair: &KeyPair) -> Result<(), String> {
    let has_vault = api
        .my_memberships()?
        .iter()
        .any(|member| member.vaults.kind == "personal");
    if has_vault {
        Ok(())
    } else {
        create_personal_vault(api, pair)
    }
}

/// Sign in with the browser. Then set up a new account or unlock an old one.
/// Nothing is saved on this computer until the keys are ready.
pub fn login(
    home: &Path,
    store: &dyn Store,
    provider: &str,
    prompt: &mut dyn Prompt,
) -> Result<LoginOutcome, String> {
    let cfg = config();
    let session = auth::sign_in_with_browser(&cfg, provider)?;
    let api = Api::new(cfg, session.clone());
    let (pair, outcome) = match api.my_profile()? {
        Some(profile) => {
            let mut attempt = 1;
            let pair = loop {
                let password = Zeroizing::new(prompt.password()?);
                match unlock_with_password(&profile, &password) {
                    Ok(pair) => break pair,
                    Err(error) if error == WRONG_PASSWORD && attempt < PASSWORD_TRIES => {
                        crate::ui::warning(&error);
                        attempt += 1;
                    }
                    Err(error) => return Err(error),
                }
            };
            ensure_personal_vault(&api, &pair)?;
            (pair, LoginOutcome::Unlocked)
        }
        None => (
            setup_profile(&api, store, prompt, KdfParams::PRODUCTION)?,
            LoginOutcome::Created,
        ),
    };
    remember_key(store, &pair)?;
    store_session(home, store, &session)?;
    Ok(outcome)
}

/// Use the saved refresh token to get a new session, and save the new token.
/// The server gives a new refresh token each time, so the save comes first.
fn signed_in(home: &Path, store: &dyn Store) -> Result<Api, String> {
    let st = state::load(home)?;
    let token = saved_refresh_token(store, &st)?.ok_or_else(|| NOT_SIGNED_IN.to_string())?;
    let cfg = config();
    let session = api::refresh(&cfg, &token)?;
    store_session(home, store, &session)?;
    Ok(Api::new(cfg, session))
}

pub fn session(
    home: &Path,
    store: &dyn Store,
    prompt: &mut dyn Prompt,
) -> Result<Unlocked, String> {
    let api = signed_in(home, store)?;
    let pair = if store.available() {
        match store.get(PRIVATE_KEY)? {
            Some(text) => {
                let text = Zeroizing::new(text);
                KeyPair::from_secret(
                    Key::from_base64(&text).map_err(|_| BAD_LOCAL_KEY.to_string())?,
                )
            }
            None => {
                let pair = unlock_from_server(&api, prompt)?;
                remember_key(store, &pair)?;
                pair
            }
        }
    } else {
        unlock_from_server(&api, prompt)?
    };
    let user_id = api.user_id().to_string();
    Ok(Unlocked { api, pair, user_id })
}

/// Set a new vault password with the recovery code.
pub fn recover(home: &Path, store: &dyn Store, prompt: &mut dyn Prompt) -> Result<(), String> {
    let api = signed_in(home, store)?;
    let profile = api.my_profile()?.ok_or_else(|| NO_PROFILE.to_string())?;
    let code = Zeroizing::new(prompt.recovery_code()?);
    let pair = unlock_with_recovery(&profile, &code)?;
    let password = Zeroizing::new(prompt.new_password()?);
    let updated = reseal_all(&profile, &pair, &password, &code)?;
    api.put_profile(&updated)?;
    remember_key(store, &pair)
}

/// Change the vault password. The recovery copy stays as it is, so the
/// salt stays the same.
pub fn change_password(
    home: &Path,
    store: &dyn Store,
    prompt: &mut dyn Prompt,
) -> Result<(), String> {
    let api = signed_in(home, store)?;
    let profile = api.my_profile()?.ok_or_else(|| NO_PROFILE.to_string())?;
    let current = Zeroizing::new(prompt.password()?);
    let pair = unlock_with_password(&profile, &current)?;
    let password = Zeroizing::new(prompt.new_password()?);
    let updated = reseal_password(&profile, &pair, &password)?;
    api.put_profile(&updated)
}

/// Sign out on the server (network errors are ignored), then remove the
/// keys and the token from this computer. Links stay.
pub fn logout(home: &Path, store: &dyn Store) -> Result<(), String> {
    let mut st = state::load(home)?;
    let token = saved_refresh_token(store, &st).ok().flatten();
    if let Some(token) = token {
        let cfg = config();
        if let Ok(session) = api::refresh(&cfg, &token) {
            let _ = api::sign_out(&cfg, &session);
        }
    }
    let mut failure = None;
    if store.available() {
        for name in [PRIVATE_KEY, SESSION] {
            if let Err(error) = store.delete(name) {
                failure.get_or_insert(error);
            }
        }
    }
    st.user_id = None;
    st.email = None;
    st.refresh_token = None;
    state::save(home, &st)?;
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams { m: 256, t: 1, p: 1 };

    fn values(password: &str, code: &str) -> (Profile, KeyPair) {
        new_profile_values("user-1", "a@example.com", password, code, FAST).unwrap()
    }

    #[test]
    fn setup_values_unlock_with_password() {
        let (profile, pair) = values("correct horse", "ABCD-EFGH");
        let unlocked = open_with_password(&profile, "correct horse").unwrap();
        assert_eq!(unlocked.public, pair.public);
        assert_eq!(
            STANDARD.decode(&profile.public_key).unwrap(),
            pair.public.to_vec()
        );
    }

    #[test]
    fn setup_values_unlock_with_recovery() {
        let code = crypto::recovery_code();
        let (profile, pair) = values("correct horse", &code);
        assert_eq!(
            open_with_recovery(&profile, &code).unwrap().public,
            pair.public
        );
        let sloppy = code.to_lowercase().replace('-', " ");
        assert_eq!(
            open_with_recovery(&profile, &sloppy).unwrap().public,
            pair.public
        );
    }

    #[test]
    fn wrong_password_is_reported() {
        let (profile, _) = values("correct horse", "ABCD-EFGH");
        let error = open_with_password(&profile, "wrong").err().unwrap();
        assert_eq!(error, "The vault password is not correct.");
    }

    #[test]
    fn wrong_recovery_code_is_reported() {
        let (profile, _) = values("correct horse", "ABCD-EFGH");
        let error = open_with_recovery(&profile, "AAAA-BBBB").err().unwrap();
        assert_eq!(error, "The recovery code is not correct.");
    }

    #[test]
    fn changed_password_keeps_recovery_code_valid() {
        let code = crypto::recovery_code();
        let (profile, pair) = values("old password", &code);
        let changed = reseal_password(&profile, &pair, "new password").unwrap();
        assert_eq!(changed.kdf_salt, profile.kdf_salt);
        assert_eq!(changed.public_key, profile.public_key);
        assert_eq!(changed.private_key_recovery, profile.private_key_recovery);
        assert_eq!(
            open_with_password(&changed, "new password").unwrap().public,
            pair.public
        );
        assert!(open_with_password(&changed, "old password").is_err());
        assert_eq!(
            open_with_recovery(&changed, &code).unwrap().public,
            pair.public
        );
    }

    #[test]
    fn recover_reseals_both_copies_with_a_new_salt() {
        let code = crypto::recovery_code();
        let (profile, pair) = values("old password", &code);
        let changed = reseal_all(&profile, &pair, "new password", &code).unwrap();
        assert_ne!(changed.kdf_salt, profile.kdf_salt);
        assert_eq!(
            open_with_password(&changed, "new password").unwrap().public,
            pair.public
        );
        assert_eq!(
            open_with_recovery(&changed, &code).unwrap().public,
            pair.public
        );
        assert!(open_with_password(&changed, "old password").is_err());
    }

    #[test]
    fn public_unlock_refuses_weak_server_settings() {
        let (mut profile, _) = values("correct horse", "ABCD-EFGH");
        profile.kdf_params = KdfParams { m: 8, t: 1, p: 1 };
        let expected = "The key settings from the server are not safe. Keyman stops.";
        assert_eq!(
            unlock_with_password(&profile, "correct horse")
                .err()
                .unwrap(),
            expected
        );
        assert_eq!(
            unlock_with_recovery(&profile, "ABCD-EFGH").err().unwrap(),
            expected
        );
    }

    #[test]
    fn production_settings_pass_the_public_check() {
        let (profile, pair) = new_profile_values(
            "user-1",
            "a@example.com",
            "correct horse",
            "ABCD-EFGH",
            KdfParams::PRODUCTION,
        )
        .unwrap();
        assert_eq!(
            unlock_with_password(&profile, "correct horse")
                .unwrap()
                .public,
            pair.public
        );
    }

    #[test]
    fn mismatched_public_key_is_refused() {
        let (mut profile, _) = values("correct horse", "ABCD-EFGH");
        profile.public_key = STANDARD.encode(KeyPair::generate().public);
        assert!(open_with_password(&profile, "correct horse").is_err());
    }

    struct NoStore;

    impl Store for NoStore {
        fn get(&self, _: &str) -> Result<Option<String>, String> {
            Err("no store".to_string())
        }
        fn set(&self, _: &str, _: &str) -> Result<(), String> {
            Err("no store".to_string())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Err("no store".to_string())
        }
        fn available(&self) -> bool {
            false
        }
    }

    fn sample_session() -> Session {
        Session {
            access_token: "access".into(),
            refresh_token: "refresh-1".into(),
            expires_at: 0,
            user_id: "user-1".into(),
            email: "a@example.com".into(),
        }
    }

    fn temp_home(name: &str) -> std::path::PathBuf {
        let home =
            std::env::temp_dir().join(format!("keyman-account-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn token_goes_to_the_store_when_there_is_one() {
        let home = temp_home("store");
        let store = crate::cloud::keystore::MemoryStore::new();
        store_session(&home, &store, &sample_session()).unwrap();
        let st = state::load(&home).unwrap();
        assert_eq!(st.refresh_token, None);
        assert_eq!(st.email.as_deref(), Some("a@example.com"));
        assert_eq!(
            saved_refresh_token(&store, &st).unwrap().as_deref(),
            Some("refresh-1")
        );
        let stored = store.get(SESSION).unwrap().unwrap();
        assert!(!stored.contains("access"));
    }

    #[test]
    fn token_goes_to_the_state_file_without_a_store() {
        let home = temp_home("nostore");
        store_session(&home, &NoStore, &sample_session()).unwrap();
        let st = state::load(&home).unwrap();
        assert_eq!(st.refresh_token.as_deref(), Some("refresh-1"));
        assert_eq!(
            saved_refresh_token(&NoStore, &st).unwrap().as_deref(),
            Some("refresh-1")
        );
        // The private key is never kept without a store.
        assert!(remember_key(&NoStore, &KeyPair::generate()).is_ok());
    }

    #[test]
    fn missing_token_means_not_signed_in() {
        let home = temp_home("none");
        let store = crate::cloud::keystore::MemoryStore::new();
        let error = signed_in(&home, &store).err().unwrap();
        assert_eq!(error, "You are not signed in. Run 'keyman login' first.");
    }
}
