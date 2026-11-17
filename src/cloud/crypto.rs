//! Crypto primitives for cloud sync. Everything the server stores is sealed
//! here first, so the server never sees a key or a value.

use crate::master::Variables;
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use crypto_box::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const NONCE_LEN: usize = 24;
const DECRYPT_ERROR: &str = "The data does not decrypt: wrong key, or the data changed.";
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn fill_random(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("the system random source failed");
}

/// A 32-byte secret key. Memory is wiped on drop.
pub struct Key(Zeroizing<[u8; 32]>);

impl Key {
    pub fn random() -> Key {
        let mut bytes = [0u8; 32];
        fill_random(&mut bytes);
        let key = Key::from_bytes(bytes);
        bytes.zeroize();
        key
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Key {
        Key(Zeroizing::new(bytes))
    }

    pub fn to_base64(&self) -> String {
        STANDARD.encode(*self.0)
    }

    pub fn from_base64(text: &str) -> Result<Key, String> {
        let bytes = Zeroizing::new(
            STANDARD
                .decode(text.trim())
                .map_err(|_| "The key is not valid base64.".to_string())?,
        );
        let array: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| "The key must have 32 bytes.".to_string())?;
        Ok(Key::from_bytes(array))
    }
}

/// An X25519 key pair. The secret half is a `Key`.
pub struct KeyPair {
    pub public: [u8; 32],
    pub secret: Key,
}

impl KeyPair {
    pub fn generate() -> KeyPair {
        KeyPair::from_secret(Key::random())
    }

    pub fn from_secret(secret: Key) -> KeyPair {
        let public = SecretKey::from_bytes(*secret.0).public_key();
        KeyPair {
            public: *public.as_bytes(),
            secret,
        }
    }
}

/// Argon2id cost settings.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)]
pub struct KdfParams {
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    pub const PRODUCTION: KdfParams = KdfParams {
        m: 65536,
        t: 3,
        p: 1,
    };

    /// The server stores these settings, so they are not trusted. Refuse
    /// values that are too weak (easy to attack) or too heavy (hang the
    /// computer).
    pub fn check(&self) -> Result<(), String> {
        if self.m < 19456
            || self.m > 1048576
            || self.t < 2
            || self.t > 10
            || self.p < 1
            || self.p > 4
        {
            return Err("The key settings from the server are not safe. Keyman stops.".to_string());
        }
        Ok(())
    }
}

pub fn new_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    fill_random(&mut salt);
    salt
}

pub fn derive_key(secret: &str, salt: &[u8; 16], params: KdfParams) -> Result<Key, String> {
    let argon_params = Params::new(params.m, params.t, params.p, Some(32))
        .map_err(|_| "The key settings are not valid.".to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut out = [0u8; 32];
    let result = argon.hash_password_into(secret.as_bytes(), salt, &mut out);
    let key = Key::from_bytes(out);
    out.zeroize();
    result.map_err(|_| "Keyman cannot make the key.".to_string())?;
    Ok(key)
}

/// Encrypt with XChaCha20-Poly1305. Output is base64 of `nonce || ciphertext`.
pub fn seal(key: &Key, aad: &str, plaintext: &[u8]) -> String {
    let cipher = XChaCha20Poly1305::new(key.0.as_slice().into());
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce);
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .expect("encryption failed");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ciphertext);
    STANDARD.encode(out)
}

pub fn open(key: &Key, aad: &str, sealed: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let raw = STANDARD
        .decode(sealed.trim())
        .map_err(|_| DECRYPT_ERROR.to_string())?;
    if raw.len() < NONCE_LEN {
        return Err(DECRYPT_ERROR.to_string());
    }
    let (nonce, ciphertext) = raw.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.0.as_slice().into());
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| DECRYPT_ERROR.to_string())
}

/// The text in front of a wrapped vault key. It binds the key to one vault
/// and one key generation, so the server cannot give the key of one vault
/// (for example a group) to another vault (for example the personal vault).
fn vault_key_binding(vault_id: &str, generation: i64) -> String {
    format!("keyman/v1|vault-key|{vault_id}|{generation}|")
}

pub const UNWRAP_ERROR: &str =
    "The vault key does not open: wrong key pair, wrong vault, or the data changed.";

/// Seal a vault key for one member with their public key (sealed box). The
/// plaintext is the binding for `vault_id` and `generation`, then the key.
pub fn wrap_for(public: &[u8; 32], key: &Key, vault_id: &str, generation: i64) -> String {
    let mut plain = Zeroizing::new(vault_key_binding(vault_id, generation).into_bytes());
    plain.extend_from_slice(key.0.as_slice());
    let sealed = PublicKey::from(*public)
        .seal(&mut crypto_box::aead::OsRng, plain.as_slice())
        .expect("sealing the key failed");
    STANDARD.encode(sealed)
}

/// Opens a wrapped vault key. The binding must be for `vault_id` and
/// `generation`; every failure gives the same error.
pub fn unwrap(
    pair: &KeyPair,
    wrapped: &str,
    vault_id: &str,
    generation: i64,
) -> Result<Key, String> {
    let fail = || UNWRAP_ERROR.to_string();
    let raw = STANDARD.decode(wrapped.trim()).map_err(|_| fail())?;
    let plain = Zeroizing::new(
        SecretKey::from_bytes(*pair.secret.0)
            .unseal(&raw)
            .map_err(|_| fail())?,
    );
    let binding = vault_key_binding(vault_id, generation);
    if plain.len() != binding.len() + 32 || !plain.starts_with(binding.as_bytes()) {
        return Err(fail());
    }
    let array: [u8; 32] = plain[binding.len()..].try_into().map_err(|_| fail())?;
    Ok(Key::from_bytes(array))
}

/// A recovery code: 20 random bytes as Crockford base32, 8 groups of 4.
pub fn recovery_code() -> String {
    let mut bytes = [0u8; 20];
    fill_random(&mut bytes);
    let mut chars = Vec::with_capacity(32);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for byte in bytes {
        acc = (acc << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            chars.push(CROCKFORD[((acc >> bits) & 31) as usize] as char);
        }
        acc &= (1 << bits) - 1;
    }
    bytes.zeroize();
    chars
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn normalize_recovery_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| *c != ' ' && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

pub fn project_aad(project_id: &str, version: i64) -> String {
    format!("keyman/v1|project|{project_id}|{version}")
}

pub fn private_key_aad(user_id: &str) -> String {
    format!("keyman/v1|private-key|{user_id}")
}

pub fn vault_name_aad(vault_id: &str) -> String {
    format!("keyman/v1|vault-name|{vault_id}")
}

#[derive(Serialize, Deserialize)]
struct ProjectPayload {
    name: String,
    variables: Vec<(String, String)>,
}

pub fn seal_project(
    key: &Key,
    project_id: &str,
    version: i64,
    name: &str,
    variables: &[(String, String)],
) -> String {
    let payload = Zeroizing::new(
        serde_json::to_vec(&ProjectPayload {
            name: name.to_owned(),
            variables: variables.to_vec(),
        })
        .expect("project data always serializes"),
    );
    seal(key, &project_aad(project_id, version), &payload)
}

pub fn open_project(
    key: &Key,
    project_id: &str,
    version: i64,
    sealed: &str,
) -> Result<(String, Variables), String> {
    let plain = open(key, &project_aad(project_id, version), sealed)?;
    let payload: ProjectPayload =
        serde_json::from_slice(&plain).map_err(|_| "The project data is not valid.".to_string())?;
    Ok((payload.name, payload.variables))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams { m: 256, t: 1, p: 1 };

    fn sample_vars() -> Variables {
        vec![("A".into(), "1".into()), ("B".into(), "two".into())]
    }

    #[test]
    fn seal_open_round_trip() {
        let k = Key::random();
        let s = seal(&k, "a", b"hi");
        assert_eq!(&*open(&k, "a", &s).unwrap(), b"hi");
    }

    #[test]
    fn wrong_key_fails() {
        let s = seal(&Key::random(), "a", b"hi");
        assert!(open(&Key::random(), "a", &s).is_err());
    }

    #[test]
    fn wrong_aad_fails() {
        let k = Key::random();
        let s = seal(&k, "a", b"hi");
        assert!(open(&k, "b", &s).is_err());
    }

    #[test]
    fn changed_ciphertext_fails() {
        let k = Key::random();
        let s = seal(&k, "a", b"hi");
        let mut raw = STANDARD.decode(&s).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        let tampered = STANDARD.encode(raw);
        assert_eq!(
            open(&k, "a", &tampered).unwrap_err(),
            "The data does not decrypt: wrong key, or the data changed."
        );
    }

    #[test]
    fn same_plaintext_gives_different_ciphertext() {
        let k = Key::random();
        assert_ne!(seal(&k, "a", b"x"), seal(&k, "a", b"x"));
    }

    #[test]
    fn derive_key_is_stable_and_salted() {
        let s1 = [1u8; 16];
        let s2 = [2u8; 16];
        assert_eq!(
            derive_key("pw", &s1, FAST).unwrap().to_base64(),
            derive_key("pw", &s1, FAST).unwrap().to_base64()
        );
        assert_ne!(
            derive_key("pw", &s1, FAST).unwrap().to_base64(),
            derive_key("pw", &s2, FAST).unwrap().to_base64()
        );
    }

    #[test]
    fn kdf_params_check_bounds() {
        assert!(KdfParams::PRODUCTION.check().is_ok());
        let bad = |m, t, p| KdfParams { m, t, p }.check().is_err();
        assert!(bad(8, 3, 1));
        assert!(bad(19455, 3, 1));
        assert!(bad(1048577, 3, 1));
        assert!(bad(65536, 1, 1));
        assert!(bad(65536, 11, 1));
        assert!(bad(65536, 3, 0));
        assert!(bad(65536, 3, 5));
        assert!(!bad(19456, 2, 1));
        assert!(!bad(1048576, 10, 4));
    }

    #[test]
    fn derive_key_errors_are_fixed_text() {
        let error = derive_key("pw", &[0u8; 16], KdfParams { m: 1, t: 1, p: 1 })
            .err()
            .unwrap();
        assert_eq!(error, "The key settings are not valid.");
    }

    #[test]
    fn wrap_unwrap_round_trip() {
        let p = KeyPair::generate();
        let k = Key::random();
        let wrapped = wrap_for(&p.public, &k, "v1", 1);
        assert_eq!(
            unwrap(&p, &wrapped, "v1", 1).unwrap().to_base64(),
            k.to_base64()
        );
    }

    #[test]
    fn unwrap_with_other_pair_fails() {
        let p = KeyPair::generate();
        let k = Key::random();
        let wrapped = wrap_for(&p.public, &k, "v1", 1);
        assert_eq!(
            unwrap(&KeyPair::generate(), &wrapped, "v1", 1)
                .err()
                .unwrap(),
            UNWRAP_ERROR
        );
    }

    #[test]
    fn wrapped_key_for_another_vault_is_refused() {
        let p = KeyPair::generate();
        let wrapped = wrap_for(&p.public, &Key::random(), "group-vault", 1);
        assert_eq!(
            unwrap(&p, &wrapped, "personal-vault", 1).err().unwrap(),
            UNWRAP_ERROR
        );
    }

    #[test]
    fn wrapped_key_for_another_generation_is_refused() {
        let p = KeyPair::generate();
        let wrapped = wrap_for(&p.public, &Key::random(), "v1", 2);
        assert_eq!(unwrap(&p, &wrapped, "v1", 1).err().unwrap(), UNWRAP_ERROR);
        assert_eq!(unwrap(&p, &wrapped, "v1", 3).err().unwrap(), UNWRAP_ERROR);
        assert!(unwrap(&p, &wrapped, "v1", 2).is_ok());
    }

    #[test]
    fn a_bare_key_with_no_binding_is_refused() {
        // The old format: the sealed box held only the 32 key bytes.
        let p = KeyPair::generate();
        let k = Key::random();
        let sealed = PublicKey::from(p.public)
            .seal(&mut crypto_box::aead::OsRng, k.0.as_slice())
            .unwrap();
        let wrapped = STANDARD.encode(sealed);
        assert_eq!(unwrap(&p, &wrapped, "v1", 1).err().unwrap(), UNWRAP_ERROR);
    }

    #[test]
    fn a_vault_id_cannot_end_the_binding_early() {
        // "v|1" with generation 2 must not match "v" with generation "1|2...".
        let p = KeyPair::generate();
        let wrapped = wrap_for(&p.public, &Key::random(), "v|1", 2);
        assert!(unwrap(&p, &wrapped, "v", 1).is_err());
        assert!(unwrap(&p, &wrapped, "v|1", 2).is_ok());
    }

    #[test]
    fn key_pair_from_secret_gives_same_public() {
        let p = KeyPair::generate();
        assert_eq!(
            KeyPair::from_secret(Key::from_base64(&p.secret.to_base64()).unwrap()).public,
            p.public
        );
    }

    #[test]
    fn recovery_code_shape() {
        let c = recovery_code();
        assert_eq!(c.len(), 39);
        assert_eq!(c.split('-').count(), 8);
        assert_eq!(
            normalize_recovery_code(&c.to_lowercase().replace('-', " ")),
            c.replace('-', "")
        );
    }

    #[test]
    fn project_moved_to_other_id_fails() {
        let k = Key::random();
        let s = seal_project(&k, "p1", 2, "app", &sample_vars());
        assert!(open_project(&k, "p2", 2, &s).is_err());
        assert!(open_project(&k, "p1", 3, &s).is_err());
    }

    #[test]
    fn round_trip_keeps_any_text() {
        let vars: Variables = vec![
            ("MULTI".into(), "line1\nline2".into()),
            ("QUOTE".into(), "a\"b'c".into()),
            ("HASH".into(), "x # y".into()),
            ("THAI".into(), "รหัสผ่าน".into()),
            ("EMPTY".into(), "".into()),
        ];
        let k = Key::random();
        let s = seal_project(&k, "p1", 1, "แอป", &vars);
        assert_eq!(
            open_project(&k, "p1", 1, &s).unwrap(),
            ("แอป".to_owned(), vars)
        );
    }
}
