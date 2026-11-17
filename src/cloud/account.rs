//! The cloud account: sign in, set up keys, unlock, recover, change password.

use super::api::{self, Api, Profile, Session};
use super::auth;
use super::config::config;
use super::crypto::{self, KdfParams, Key, KeyPair};
use super::keystore::Store;
use super::state::{self, State};
use super::sync;
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
    "The code is not the same. Keyman did not save anything. Run 'keyman login' to start again.";
const SIGN_IN_INVALID: &str = "Your sign-in is not valid now. Run 'keyman login'.";
const WRONG_OWNER: &str = "The profile on the server is for a different user. Keyman stops.";
const SESSION: &str = "session";
const LEGACY_KEY: &str = "private_key";
const NO_CLOUD: &str = "You do not have Keyman Cloud yet. Run 'keyman login' to set it up.";
const RECOVER_HINT: &str = "Run 'keyman recover' to use your recovery code.";
const USER_SWITCHED: &str = "You signed in as a different user. Keyman removed the cloud links of the old user. Your local projects did not change.";
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

/// The store entry for the private key of one user.
fn key_entry(user_id: &str) -> String {
    format!("{user_id}/private-key")
}

fn remember_key(store: &dyn Store, user_id: &str, pair: &KeyPair) -> Result<(), String> {
    if store.available() {
        let text = Zeroizing::new(pair.secret.to_base64());
        store.set(&key_entry(user_id), &text)?;
    }
    Ok(())
}

/// The key that this computer keeps for this user, if any.
fn stored_key(store: &dyn Store, user_id: &str) -> Result<Option<KeyPair>, String> {
    match store.get(&key_entry(user_id))? {
        Some(text) => {
            let text = Zeroizing::new(text);
            let key = Key::from_base64(&text).map_err(|_| BAD_LOCAL_KEY.to_string())?;
            Ok(Some(KeyPair::from_secret(key)))
        }
        None => Ok(None),
    }
}

/// A profile must belong to the signed-in user.
fn check_owner(profile: &Profile, user_id: &str) -> Result<(), String> {
    if profile.user_id == user_id {
        Ok(())
    } else {
        Err(WRONG_OWNER.to_string())
    }
}

/// Makes the personal vault and returns its ID. The ID is made here, so the
/// wrapped key is bound to it (generation 1).
fn create_personal_vault(api: &Api, pair: &KeyPair) -> Result<String, String> {
    let id = api::new_uuid();
    let vault_key = Key::random();
    api.create_vault(
        &id,
        "personal",
        None,
        &crypto::wrap_for(&pair.public, &vault_key, &id, 1),
    )?;
    Ok(id)
}

const OTHER_USER: &str = "You signed in as a different user. Run 'keyman login' again.";

/// The personal vault that this computer saved for this user, if any.
pub(crate) fn pinned_vault(st: &State, user_id: &str) -> Option<String> {
    if st.user_id.as_deref() == Some(user_id) {
        st.personal_vault.clone()
    } else {
        None
    }
}

/// Saves the personal vault ID the first time. After that, a different ID
/// is refused: the server must not swap the personal vault.
pub(crate) fn pin_personal_vault(home: &Path, user_id: &str, vault_id: &str) -> Result<(), String> {
    let mut st = state::load(home)?;
    if st.user_id.as_deref() != Some(user_id) {
        return Err(OTHER_USER.to_string());
    }
    match st.personal_vault.as_deref() {
        Some(pinned) if pinned == vault_id => Ok(()),
        Some(_) => Err(sync::PERSONAL_CHANGED.to_string()),
        None => {
            st.personal_vault = Some(vault_id.to_string());
            state::save(home, &st)
        }
    }
}

/// First sign-in: make the keys, the profile and the personal vault.
/// The caller saves the session first, then the private key.
/// Returns the key pair and the ID of the personal vault.
pub fn setup_profile(
    api: &Api,
    prompt: &mut dyn Prompt,
    params: KdfParams,
) -> Result<(KeyPair, String), String> {
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
        crate::ui::warning("The code is not the same. Try again.");
    }
    if !confirmed {
        return Err(CODE_MISMATCH.to_string());
    }
    api.put_profile(&profile)?;
    let vault_id = create_personal_vault(api, &pair)?;
    Ok((pair, vault_id))
}

/// Record the signed-in user. When it is a different user than before, the
/// old links go away (they belong to the old user). Returns true then.
fn switch_user(st: &mut State, user_id: &str) -> bool {
    let switched = st.user_id.as_deref().is_some_and(|old| old != user_id);
    if switched {
        st.links.clear();
        st.personal_vault = None;
    }
    st.user_id = Some(user_id.to_string());
    switched
}

/// True when there is no saved session, so a browser sign-in must come first.
fn needs_sign_in(store: &dyn Store, st: &State) -> Result<bool, String> {
    Ok(saved_refresh_token(store, st)?.is_none())
}

/// Save the refresh token (store, or the state file when there is no store)
/// and the account name.
fn store_session(home: &Path, store: &dyn Store, session: &Session) -> Result<(), String> {
    let mut st = state::load(home)?;
    if switch_user(&mut st, &session.user_id) {
        crate::ui::warning(USER_SWITCHED);
    }
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
    check_owner(&profile, api.user_id())?;
    let password = Zeroizing::new(prompt.password()?);
    unlock_with_password(&profile, &password)
}

/// Finds the personal vault and checks that its key opens with this key
/// pair. A setup that stopped half way can leave a profile with no vault:
/// then Keyman makes it. Returns the vault ID.
fn ensure_personal_vault(home: &Path, api: &Api, pair: &KeyPair) -> Result<String, String> {
    let memberships = api.my_memberships()?;
    let pinned = pinned_vault(&state::load(home)?, api.user_id());
    match sync::personal_in(&memberships, api.user_id(), pinned.as_deref()) {
        Ok(id) => {
            let member = memberships
                .iter()
                .find(|member| member.vault_id == id)
                .ok_or_else(|| sync::PERSONAL_CHANGED.to_string())?;
            crypto::unwrap(pair, &member.wrapped_key, &id, member.key_generation)?;
            Ok(id)
        }
        Err(error) if error == sync::NO_PERSONAL => create_personal_vault(api, pair),
        Err(error) => Err(error),
    }
}

/// Sign in with the browser. Then set up a new account or unlock an old one.
/// Nothing is saved on this computer until the keys are ready. If a step
/// fails, Keyman ends the server session.
pub fn login(
    home: &Path,
    store: &dyn Store,
    provider: &str,
    prompt: &mut dyn Prompt,
) -> Result<LoginOutcome, String> {
    let session = auth::sign_in_with_browser(&config(), provider)?;
    login_with_session(home, store, &session, prompt)
}

/// `login` after the sign-in step: the caller already has a session (the
/// end-to-end tests sign in with a password).
pub fn login_with_session(
    home: &Path,
    store: &dyn Store,
    session: &Session,
    prompt: &mut dyn Prompt,
) -> Result<LoginOutcome, String> {
    let cfg = config();
    let api = Api::new(cfg.clone(), session.clone());
    let result = finish_login(home, store, &api, session, prompt);
    if result.is_err() {
        let _ = api::sign_out(&cfg, session);
    }
    result
}

/// Answers the setup questions with one password and confirms the recovery
/// code that setup shows.
struct SetupAnswers {
    password: Zeroizing<String>,
    code: Option<Zeroizing<String>>,
}

impl Prompt for SetupAnswers {
    fn new_password(&mut self) -> Result<String, String> {
        Ok(self.password.to_string())
    }
    fn password(&mut self) -> Result<String, String> {
        Ok(self.password.to_string())
    }
    fn recovery_code(&mut self) -> Result<String, String> {
        self.code
            .as_ref()
            .map(|code| code.to_string())
            .ok_or_else(|| CODE_MISMATCH.to_string())
    }
    fn show_recovery_code(&mut self, code: &str) -> Result<(), String> {
        self.code = Some(Zeroizing::new(code.to_string()));
        Ok(())
    }
}

/// Set up a new account with no questions, for the end-to-end tests. It
/// saves the session and the key like `login`. Returns the key pair and the
/// recovery code.
pub fn setup_with(
    home: &Path,
    store: &dyn Store,
    session: &Session,
    password: &str,
    params: KdfParams,
) -> Result<(KeyPair, String), String> {
    let api = Api::new(config(), session.clone());
    let mut answers = SetupAnswers {
        password: Zeroizing::new(password.to_string()),
        code: None,
    };
    let (pair, vault_id) = setup_profile(&api, &mut answers, params)?;
    store_session(home, store, session)?;
    remember_key(store, &session.user_id, &pair)?;
    pin_personal_vault(home, &session.user_id, &vault_id)?;
    let code = answers
        .code
        .map(|code| code.to_string())
        .unwrap_or_default();
    Ok((pair, code))
}

fn finish_login(
    home: &Path,
    store: &dyn Store,
    api: &Api,
    session: &Session,
    prompt: &mut dyn Prompt,
) -> Result<LoginOutcome, String> {
    let (pair, vault_id, outcome) = match api.my_profile()? {
        Some(profile) => {
            check_owner(&profile, api.user_id())?;
            let mut attempt = 1;
            let pair = loop {
                let password = Zeroizing::new(prompt.password()?);
                match unlock_with_password(&profile, &password) {
                    Ok(pair) => break pair,
                    Err(error) if error == WRONG_PASSWORD && attempt < PASSWORD_TRIES => {
                        crate::ui::warning(&error);
                        attempt += 1;
                    }
                    Err(error) if error == WRONG_PASSWORD => {
                        return Err(format!("{error}\n\n{RECOVER_HINT}"))
                    }
                    Err(error) => return Err(error),
                }
            };
            let vault_id = ensure_personal_vault(home, api, &pair)?;
            (pair, vault_id, LoginOutcome::Unlocked)
        }
        None => {
            let (pair, vault_id) = setup_profile(api, prompt, KdfParams::PRODUCTION)?;
            (pair, vault_id, LoginOutcome::Created)
        }
    };
    // The session entry comes first, then the key.
    store_session(home, store, session)?;
    remember_key(store, &session.user_id, &pair)?;
    pin_personal_vault(home, &session.user_id, &vault_id)?;
    if store.available() {
        forget_legacy_key(store);
    }
    Ok(outcome)
}

/// Remove a saved session that the server does not accept.
fn drop_session(home: &Path, store: &dyn Store) {
    if store.available() {
        let _ = store.delete(SESSION);
    }
    if let Ok(mut st) = state::load(home) {
        if st.refresh_token.take().is_some() {
            let _ = state::save(home, &st);
        }
    }
}

/// Use the saved refresh token to get a new session, and save the new token.
/// The server gives a new refresh token each time, so the save comes first.
fn signed_in(home: &Path, store: &dyn Store) -> Result<Api, String> {
    let st = state::load(home)?;
    let token = saved_refresh_token(store, &st)?.ok_or_else(|| NOT_SIGNED_IN.to_string())?;
    let cfg = config();
    let session = match api::refresh(&cfg, &token) {
        Ok(session) => session,
        // Only a clear "bad token" answer ends the sign-in.
        Err(api::RefreshError::Rejected) => {
            drop_session(home, store);
            return Err(SIGN_IN_INVALID.to_string());
        }
        Err(api::RefreshError::Unavailable(message)) => return Err(message),
    };
    store_session(home, store, &session)?;
    Ok(Api::new(cfg, session))
}

pub fn session(
    home: &Path,
    store: &dyn Store,
    prompt: &mut dyn Prompt,
) -> Result<Unlocked, String> {
    let api = signed_in(home, store)?;
    let user_id = api.user_id().to_string();
    let pair = if store.available() {
        match stored_key(store, &user_id)? {
            Some(pair) => pair,
            None => {
                let pair = unlock_from_server(&api, prompt)?;
                remember_key(store, &user_id, &pair)?;
                pair
            }
        }
    } else {
        unlock_from_server(&api, prompt)?
    };
    Ok(Unlocked { api, pair, user_id })
}

/// Set a new vault password with the recovery code. With no saved session,
/// `provider` is called to pick a provider, and Keyman signs in first.
pub fn recover(
    home: &Path,
    store: &dyn Store,
    provider: &mut dyn FnMut() -> Result<&'static str, String>,
    prompt: &mut dyn Prompt,
) -> Result<(), String> {
    let st = state::load(home)?;
    let saved = if needs_sign_in(store, &st)? {
        None
    } else {
        match signed_in(home, store) {
            Ok(api) => Some(api),
            // The saved sign-in is old: sign in again with the browser.
            Err(error) if error == SIGN_IN_INVALID => None,
            Err(error) => return Err(error),
        }
    };
    let (api, fresh) = match saved {
        Some(api) => (api, None),
        None => {
            let cfg = config();
            let session = auth::sign_in_with_browser(&cfg, provider()?)?;
            (Api::new(cfg, session.clone()), Some(session))
        }
    };
    let result = finish_recover(home, store, &api, fresh.as_ref(), prompt);
    if let (Err(_), Some(session)) = (&result, &fresh) {
        let _ = api::sign_out(&config(), session);
    }
    result
}

fn finish_recover(
    home: &Path,
    store: &dyn Store,
    api: &Api,
    fresh: Option<&Session>,
    prompt: &mut dyn Prompt,
) -> Result<(), String> {
    let profile = api.my_profile()?.ok_or_else(|| NO_CLOUD.to_string())?;
    check_owner(&profile, api.user_id())?;
    let code = Zeroizing::new(prompt.recovery_code()?);
    let pair = unlock_with_recovery(&profile, &code)?;
    let password = Zeroizing::new(prompt.new_password()?);
    let updated = reseal_all(&profile, &pair, &password, &code)?;
    api.put_profile(&updated)?;
    // The session entry comes first, then the key.
    if let Some(session) = fresh {
        store_session(home, store, session)?;
    }
    remember_key(store, api.user_id(), &pair)
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
    check_owner(&profile, api.user_id())?;
    let current = Zeroizing::new(prompt.password()?);
    let pair = unlock_with_password(&profile, &current)?;
    let password = Zeroizing::new(prompt.new_password()?);
    let updated = reseal_password(&profile, &pair, &password)?;
    api.put_profile(&updated)
}

/// Sign out on the server (network errors are ignored), then remove the
/// keys and the token from this computer. Links stay.
pub fn logout(home: &Path, store: &dyn Store) -> Result<(), String> {
    let st = state::load(home)?;
    if let Some(token) = saved_refresh_token(store, &st).ok().flatten() {
        let cfg = config();
        if let Ok(session) = api::refresh(&cfg, &token) {
            let _ = api::sign_out(&cfg, &session);
        }
    }
    forget_local(home, store)
}

/// Remove the session, the keys and the email from this computer.
fn forget_local(home: &Path, store: &dyn Store) -> Result<(), String> {
    let mut st = state::load(home)?;
    let mut failure = None;
    if store.available() {
        // The key belongs to the user of the saved session. The user in the
        // state file is the fallback. Remove both entries if they differ.
        let from_session = store
            .get(SESSION)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<StoredSession>(&json).ok())
            .map(|stored| stored.user_id);
        let mut names: Vec<String> = [from_session, st.user_id.clone()]
            .into_iter()
            .flatten()
            .map(|id| key_entry(&id))
            .collect();
        names.dedup();
        names.push(SESSION.to_string());
        for name in &names {
            if let Err(error) = store.delete(name) {
                failure.get_or_insert(error);
            }
        }
        forget_legacy_key(store);
    }
    // The user id stays (it is not secret). It lets the next sign-in see
    // a different user and remove the old links.
    st.email = None;
    st.refresh_token = None;
    state::save(home, &st)?;
    failure.map_or(Ok(()), Err)
}

/// Old versions used one global key entry. Remove it (errors are ignored).
fn forget_legacy_key(store: &dyn Store) {
    let _ = store.delete(LEGACY_KEY);
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
        assert!(remember_key(&NoStore, "user-1", &KeyPair::generate()).is_ok());
    }

    #[test]
    fn missing_token_means_not_signed_in() {
        let home = temp_home("none");
        let store = crate::cloud::keystore::MemoryStore::new();
        let error = signed_in(&home, &store).err().unwrap();
        assert_eq!(error, "You are not signed in. Run 'keyman login' first.");
    }

    #[test]
    fn no_session_means_sign_in_first() {
        let home = temp_home("needs");
        let store = crate::cloud::keystore::MemoryStore::new();
        let st = state::load(&home).unwrap();
        assert!(needs_sign_in(&store, &st).unwrap());
        store_session(&home, &store, &sample_session()).unwrap();
        let st = state::load(&home).unwrap();
        assert!(!needs_sign_in(&store, &st).unwrap());
        let mut st = state::State::default();
        assert!(needs_sign_in(&NoStore, &st).unwrap());
        st.refresh_token = Some("t".into());
        assert!(!needs_sign_in(&NoStore, &st).unwrap());
    }

    #[test]
    fn a_different_user_loses_the_old_links() {
        let home = temp_home("switch");
        let store = crate::cloud::keystore::MemoryStore::new();
        let mut st = state::State {
            user_id: Some("user-A".into()),
            ..Default::default()
        };
        st.set_link(state::Link {
            project: "app".into(),
            cloud_id: "c1".into(),
            vault_id: "v1".into(),
            version: 1,
            base: vec![("K".into(), "v".into())],
            cloud_name: "app".into(),
        });
        state::save(&home, &st).unwrap();
        // The same user keeps the links.
        let mut same = sample_session();
        same.user_id = "user-A".into();
        store_session(&home, &store, &same).unwrap();
        assert_eq!(state::load(&home).unwrap().links.len(), 1);
        // A different user does not.
        let mut other = sample_session();
        other.user_id = "user-B".into();
        store_session(&home, &store, &other).unwrap();
        let after = state::load(&home).unwrap();
        assert!(after.links.is_empty());
        assert_eq!(after.user_id.as_deref(), Some("user-B"));
    }

    #[test]
    fn the_personal_vault_is_saved_then_checked() {
        let home = temp_home("pin");
        let store = crate::cloud::keystore::MemoryStore::new();
        store_session(&home, &store, &sample_session()).unwrap();
        pin_personal_vault(&home, "user-1", "vault-1").unwrap();
        let st = state::load(&home).unwrap();
        assert_eq!(st.personal_vault.as_deref(), Some("vault-1"));
        assert_eq!(pinned_vault(&st, "user-1").as_deref(), Some("vault-1"));
        assert_eq!(pinned_vault(&st, "user-2"), None);
        assert!(pin_personal_vault(&home, "user-1", "vault-1").is_ok());
        assert_eq!(
            pin_personal_vault(&home, "user-1", "vault-2").unwrap_err(),
            "The server changed your personal vault. Keyman stops."
        );
        assert_eq!(
            state::load(&home).unwrap().personal_vault.as_deref(),
            Some("vault-1")
        );
        // Another user does not get the saved vault of the old user.
        let mut other = sample_session();
        other.user_id = "user-2".into();
        store_session(&home, &store, &other).unwrap();
        assert_eq!(state::load(&home).unwrap().personal_vault, None);
        pin_personal_vault(&home, "user-2", "vault-9").unwrap();
        assert!(pin_personal_vault(&home, "user-1", "vault-1").is_err());
    }

    #[test]
    fn first_user_keeps_links() {
        let mut st = state::State::default();
        assert!(!switch_user(&mut st, "user-A"));
        assert_eq!(st.user_id.as_deref(), Some("user-A"));
    }

    #[test]
    fn key_of_another_user_is_not_used() {
        let store = crate::cloud::keystore::MemoryStore::new();
        let b = KeyPair::generate();
        remember_key(&store, "user-B", &b).unwrap();
        assert!(stored_key(&store, "user-A").unwrap().is_none());
        assert_eq!(
            stored_key(&store, "user-B").unwrap().unwrap().public,
            b.public
        );
        assert_eq!(key_entry("user-A"), "user-A/private-key");
    }

    #[test]
    fn profile_of_another_user_is_refused() {
        let (profile, _) = values("correct horse", "ABCD-EFGH");
        assert!(check_owner(&profile, "user-1").is_ok());
        assert!(check_owner(&profile, "user-2").is_err());
    }

    #[test]
    fn stale_session_is_dropped() {
        let home = temp_home("stale");
        let store = crate::cloud::keystore::MemoryStore::new();
        store_session(&home, &store, &sample_session()).unwrap();
        drop_session(&home, &store);
        let st = state::load(&home).unwrap();
        assert!(needs_sign_in(&store, &st).unwrap());
        assert_eq!(st.user_id.as_deref(), Some("user-1"));
        let mut st = state::State {
            refresh_token: Some("t".into()),
            ..Default::default()
        };
        state::save(&home, &st).unwrap();
        drop_session(&home, &NoStore);
        st = state::load(&home).unwrap();
        assert!(needs_sign_in(&NoStore, &st).unwrap());
    }

    #[test]
    fn logout_keeps_the_user_so_a_switch_is_seen() {
        let home = temp_home("logout");
        let store = crate::cloud::keystore::MemoryStore::new();
        let mut st = state::State {
            user_id: Some("user-A".into()),
            email: Some("a@example.com".into()),
            ..Default::default()
        };
        st.set_link(state::Link {
            project: "app".into(),
            cloud_id: "c1".into(),
            vault_id: "v1".into(),
            version: 1,
            base: vec![],
            cloud_name: "app".into(),
        });
        state::save(&home, &st).unwrap();
        remember_key(&store, "user-A", &KeyPair::generate()).unwrap();
        logout(&home, &store).unwrap();
        let after = state::load(&home).unwrap();
        assert_eq!(after.user_id.as_deref(), Some("user-A"));
        assert_eq!(after.email, None);
        assert_eq!(after.links.len(), 1);
        assert!(stored_key(&store, "user-A").unwrap().is_none());
        let mut other = sample_session();
        other.user_id = "user-B".into();
        store_session(&home, &store, &other).unwrap();
        let switched = state::load(&home).unwrap();
        assert!(switched.links.is_empty());
        assert_eq!(switched.user_id.as_deref(), Some("user-B"));
    }

    #[test]
    fn logout_removes_the_key_of_the_session_user() {
        let home = temp_home("forget");
        let store = crate::cloud::keystore::MemoryStore::new();
        let mut other = sample_session();
        other.user_id = "user-B".into();
        store_session(&home, &store, &other).unwrap();
        remember_key(&store, "user-B", &KeyPair::generate()).unwrap();
        store.set(LEGACY_KEY, "old").unwrap();
        // The state file has no user.
        let mut st = state::load(&home).unwrap();
        st.user_id = None;
        state::save(&home, &st).unwrap();
        forget_local(&home, &store).unwrap();
        assert!(stored_key(&store, "user-B").unwrap().is_none());
        assert_eq!(store.get(SESSION).unwrap(), None);
        assert_eq!(store.get(LEGACY_KEY).unwrap(), None);
    }
}
