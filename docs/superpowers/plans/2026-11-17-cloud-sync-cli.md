# Keyman Cloud Sync (Library and CLI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add optional, end-to-end encrypted cloud sync and share groups to the Keyman shared library and CLI, with a Supabase back end.

**Architecture:** A new `src/cloud/` module in the shared library holds the crypto, the three-way merge, the local sync state, the Supabase client, the credential store and the sync operations. The CLI gets new commands that call it. The server keeps only ciphertext; Row Level Security and a few `security definer` Postgres functions guard every table. The desktop app screens are a separate plan (Plan 2), written after this plan lands.

**Tech Stack:** Rust 2021 (stable), `ureq` 3 + `serde_json`, `argon2`, `chacha20poly1305`, `crypto_box` (sealed box), `keyring` 3, `rpassword`, `webbrowser`, `base64`, `sha2`, `getrandom`, `zeroize`. Supabase (Postgres, Auth with PKCE, PostgREST). Supabase CLI for local tests.

**Spec:** `docs/superpowers/specs/2026-11-17-cloud-sync-design.md`

## Global Constraints

- All new user-facing text (CLI output, errors, help) is in ASD-STE100: short sentences, active voice, simple tense, approved words.
- Without a sign-in, no command does a network request. The existing commands (`init`, `load`, `edit`, `status`, `list`, `show`, `inject`, `diff`, `save`, `export`, `run`, `completions`) never call `cloud::`.
- The server never receives a project name, a variable name or a value in plain text.
- Project data: XChaCha20-Poly1305, random 24-byte nonce for each encryption, associated data `keyman/v1|project|{project_id}|{version}`.
- Private key copies: XChaCha20-Poly1305, associated data `keyman/v1|private-key|{user_id}`.
- Group name: XChaCha20-Poly1305 with the vault key, associated data `keyman/v1|vault-name|{vault_id}`.
- Vault key for a member: X25519 sealed box (`crypto_box::seal` / `PublicKey::seal`).
- Password and recovery code to key: Argon2id, production parameters `m = 65536 KiB, t = 3, p = 1`, 16-byte random salt, 32-byte output. Parameters are stored with the profile so they can change later.
- Sync state file: `~/.keyman_sync`, permissions 600, written with `master::atomic_write` then `master::set_private_permissions`.
- Credential store service name: `keyman`. Entries: `{user_id}/private-key`, `session`.
- Push refuses when the cloud version is newer than the base. The user must pull first.
- Invite is accepted on email match only. No fingerprint check (user decision).
- `unlink` removes only the local link. `cloud delete` deletes the cloud copy, after confirmation.
- The CI command set must still pass: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`.

## Decisions This Plan Adds To The Spec

- Spec 7.3 lists `vault_members.status`. This plan keeps pending invites only in `invites`; a `vault_members` row always means an active member. Same behavior, one less state.
- Vault creation, invite acceptance and member removal go through `security definer` functions (`create_vault`, `accept_invite`, `rotate_vault`). Clients have no direct insert or update on `vaults` and `vault_members`.
- A trigger forces `projects.version = old.version + 1` and `updated_at = now()` on every update.
- On a Linux computer with no credential store, the refresh token is kept in `~/.keyman_sync` (600). The private key is never written to a file; Keyman asks for the vault password at each cloud command (spec section 8, item 11).
- Flag values: `cli::Spec.flags` gets value flags written as `"--group <name>"`. `link` uses `--group <name>`; `pull` uses `--as <name>`.
- The Supabase URL and publishable key are constants in `src/cloud/config.rs`. The environment variables `KEYMAN_SUPABASE_URL` and `KEYMAN_SUPABASE_KEY` override them (for local tests).

## Review Focus

1. A link points to a local project that the user renamed or deleted in the master file. Expected: `push` and `pull` stop for that project with a clear message that names `keyman unlink`, and continue with the other projects.
2. The user types a wrong vault password or recovery code. Expected: the message "The vault password is not correct." No data changes. The user can try again.
3. `pull` merges, but `save::update_project` refuses the project layout (flow style). Expected: Keyman writes nothing, keeps the old base, and shows the `update_project` error.
4. The network stops after the master file was written but before the base was stored. Expected: the next `pull` finds local and cloud equal and reports no conflict.
5. Values with newlines, quotes, `#`, Thai text and empty strings. Expected: they round-trip unchanged through encrypt, decrypt, merge and the master file.

Tests that pin these: Task 2 (`merge_after_lost_base_has_no_conflicts`), Task 1 (`round_trip_keeps_any_text`), Task 6 (`wrong_password_is_reported`), Task 7 (`missing_local_project_is_skipped_with_message`, `refused_layout_writes_nothing`).

---

## File Structure

| File | Responsibility |
| --- | --- |
| `Cargo.toml` | New dependencies |
| `src/lib.rs` | `pub mod cloud;` |
| `src/cloud/mod.rs` | Module list, shared types (`Variables` re-export) |
| `src/cloud/crypto.rs` | Keys, KDF, seal/open, wrap/unwrap, recovery code, blob format |
| `src/cloud/merge.rs` | Three-way merge, conflict resolution (pure) |
| `src/cloud/state.rs` | `~/.keyman_sync` read/write, links, bases |
| `src/cloud/config.rs` | Supabase URL and key, env overrides |
| `src/cloud/api.rs` | HTTP calls: Auth (PKCE, refresh), PostgREST tables, RPC |
| `src/cloud/auth.rs` | Browser sign-in with loopback listener, PKCE values |
| `src/cloud/keystore.rs` | Credential store with Linux fallback |
| `src/cloud/account.rs` | Setup, unlock, recover, change password, logout |
| `src/cloud/sync.rs` | Link, unlink, push, pull, cloud status, cloud delete |
| `src/cloud/groups.rs` | Create, invite, accept pending invites, remove (rotate), list, delete |
| `src/cli.rs` | New command specs, value flags |
| `src/main.rs` | Dispatch for new commands, terminal prompts |
| `src/completions.rs` | New commands in shell completions |
| `supabase/config.toml` | Local Supabase config (auth providers, redirect URLs) |
| `supabase/migrations/20261117000000_cloud_sync.sql` | Tables, RLS, functions, trigger |
| `supabase/tests/rls_test.sql` | pgTAP tests for RLS and functions |
| `tests/cloud_e2e.rs` | End-to-end tests against local Supabase (ignored unless `KEYMAN_E2E=1`) |
| `README.md` | New "Cloud sync" section |

---

### Task 1: Crypto primitives

**Files:**
- Modify: `Cargo.toml`, `src/lib.rs`
- Create: `src/cloud/mod.rs`, `src/cloud/crypto.rs` (tests inline, `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  - `pub struct Key(Zeroizing<[u8; 32]>)` with `Key::random() -> Key`, `Key::from_bytes([u8; 32]) -> Key`, `Key::to_base64(&self) -> String`, `Key::from_base64(&str) -> Result<Key, String>`
  - `pub struct KeyPair { pub public: [u8; 32], pub secret: Key }` with `KeyPair::generate() -> KeyPair`, `KeyPair::from_secret(Key) -> KeyPair`
  - `#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Debug)] pub struct KdfParams { pub m: u32, pub t: u32, pub p: u32 }` with `pub const KdfParams::PRODUCTION = KdfParams { m: 65536, t: 3, p: 1 }`
  - `pub fn new_salt() -> [u8; 16]`
  - `pub fn derive_key(secret: &str, salt: &[u8; 16], params: KdfParams) -> Result<Key, String>`
  - `pub fn seal(key: &Key, aad: &str, plaintext: &[u8]) -> String` — base64 (standard, padded) of `nonce(24) || ciphertext`
  - `pub fn open(key: &Key, aad: &str, sealed: &str) -> Result<Zeroizing<Vec<u8>>, String>`
  - `pub fn wrap_for(public: &[u8; 32], key: &Key) -> String`
  - `pub fn unwrap(pair: &KeyPair, wrapped: &str) -> Result<Key, String>`
  - `pub fn recovery_code() -> String` — 20 random bytes, Crockford base32, groups of 4 joined with `-` (8 groups)
  - `pub fn normalize_recovery_code(input: &str) -> String` — uppercase, removes spaces and `-`
  - `pub fn project_aad(project_id: &str, version: i64) -> String`, `pub fn private_key_aad(user_id: &str) -> String`, `pub fn vault_name_aad(vault_id: &str) -> String`
  - `pub fn seal_project(key: &Key, project_id: &str, version: i64, name: &str, variables: &[(String, String)]) -> String` — plaintext is JSON `{"name": "...", "variables": [["K","V"], ...]}`
  - `pub fn open_project(key: &Key, project_id: &str, version: i64, sealed: &str) -> Result<(String, Variables), String>`

- [ ] **Step 1: Add dependencies**

```toml
[dependencies]
yaml-rust2 = "=0.13.0"
argon2 = "0.5"
chacha20poly1305 = "0.10"
crypto_box = { version = "0.9", features = ["seal"] }
base64 = "0.22"
getrandom = "0.2"
zeroize = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Add `pub mod cloud;` to `src/lib.rs`. `src/cloud/mod.rs` declares `pub mod crypto;` and later modules as each task adds them.

- [ ] **Step 2: Write the failing tests** in `src/cloud/crypto.rs`

Tests use `const FAST: KdfParams = KdfParams { m: 256, t: 1, p: 1 };`.

```rust
#[test] fn seal_open_round_trip() { let k = Key::random(); let s = seal(&k, "a", b"hi"); assert_eq!(&*open(&k, "a", &s).unwrap(), b"hi"); }
#[test] fn wrong_key_fails() { let s = seal(&Key::random(), "a", b"hi"); assert!(open(&Key::random(), "a", &s).is_err()); }
#[test] fn wrong_aad_fails() { let k = Key::random(); let s = seal(&k, "a", b"hi"); assert!(open(&k, "b", &s).is_err()); }
#[test] fn changed_ciphertext_fails() { /* flip one byte after base64 decode, re-encode */ assert!(open(&k, "a", &tampered).is_err()); }
#[test] fn same_plaintext_gives_different_ciphertext() { assert_ne!(seal(&k, "a", b"x"), seal(&k, "a", b"x")); }
#[test] fn derive_key_is_stable_and_salted() {
    let s1 = [1u8; 16]; let s2 = [2u8; 16];
    assert_eq!(derive_key("pw", &s1, FAST).unwrap().to_base64(), derive_key("pw", &s1, FAST).unwrap().to_base64());
    assert_ne!(derive_key("pw", &s1, FAST).unwrap().to_base64(), derive_key("pw", &s2, FAST).unwrap().to_base64());
}
#[test] fn wrap_unwrap_round_trip() { let p = KeyPair::generate(); let k = Key::random(); assert_eq!(unwrap(&p, &wrap_for(&p.public, &k)).unwrap().to_base64(), k.to_base64()); }
#[test] fn unwrap_with_other_pair_fails() { assert!(unwrap(&KeyPair::generate(), &wrap_for(&p.public, &k)).is_err()); }
#[test] fn key_pair_from_secret_gives_same_public() { let p = KeyPair::generate(); assert_eq!(KeyPair::from_secret(Key::from_base64(&p.secret.to_base64()).unwrap()).public, p.public); }
#[test] fn recovery_code_shape() { let c = recovery_code(); assert_eq!(c.len(), 39); assert_eq!(c.split('-').count(), 8); assert_eq!(normalize_recovery_code(&c.to_lowercase().replace('-', " ")), c.replace('-', "")); }
#[test] fn project_moved_to_other_id_fails() { let k = Key::random(); let s = seal_project(&k, "p1", 2, "app", &vars); assert!(open_project(&k, "p2", 2, &s).is_err()); assert!(open_project(&k, "p1", 3, &s).is_err()); }
#[test] fn round_trip_keeps_any_text() {
    let vars = vec![
        ("MULTI".into(), "line1\nline2".into()), ("QUOTE".into(), "a\"b'c".into()),
        ("HASH".into(), "x # y".into()), ("THAI".into(), "รหัสผ่าน".into()), ("EMPTY".into(), "".into()),
    ];
    let k = Key::random(); let s = seal_project(&k, "p1", 1, "แอป", &vars);
    assert_eq!(open_project(&k, "p1", 1, &s).unwrap(), ("แอป".to_owned(), vars));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test cloud::crypto`
Expected: FAIL (compile errors: items not defined)

- [ ] **Step 4: Implement `src/cloud/crypto.rs`**

`seal` uses `XChaCha20Poly1305` with `Payload { msg, aad }` and a nonce from `getrandom`. `open` returns `Err("the data does not decrypt: wrong key, or the data changed".into())` on any failure (one message for all failure kinds). `derive_key` uses `Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(m, t, p, Some(32)))` and `hash_password_into`. Crockford alphabet: `0123456789ABCDEFGHJKMNPQRSTVWXYZ`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test cloud::crypto`
Expected: PASS (13 tests)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/cloud
git commit -m "Add cloud crypto primitives"
```

---

### Task 2: Three-way merge

**Files:**
- Create: `src/cloud/merge.rs` (tests inline)

**Interfaces:**
- Consumes: `master::Variables` (`Vec<(String, String)>`)
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub struct Conflict { pub name: String, pub local: Option<String>, pub cloud: Option<String> }` (`None` = removed on that side)
  - `#[derive(Debug, PartialEq)] pub struct Merge { pub result: Variables, pub conflicts: Vec<Conflict> }` — `result` holds every non-conflict variable
  - `pub fn merge(base: &[(String, String)], local: &[(String, String)], cloud: &[(String, String)]) -> Merge`
  - `pub fn resolve(merge: Merge, choices: &[Option<String>]) -> Variables` — `choices[i]` is the value for `merge.conflicts[i]`; `None` removes the variable. Panics if lengths differ (caller bug).
  - `pub enum Keep { Local, Cloud }` and `pub fn choose_all(conflicts: &[Conflict], keep: Keep) -> Vec<Option<String>>`

Rules for each name in base ∪ local ∪ cloud (L = local, C = cloud, B = base; absent = `None`):
- `L == C` → take L (absent → dropped).
- `L == B` → take C.
- `C == B` → take L.
- otherwise → conflict.

Order of `result`: local order first, then names only in cloud in cloud order. `resolve` inserts each chosen value at its local position when the name is in local, else at the end.

- [ ] **Step 1: Write the failing tests**

```rust
fn v(pairs: &[(&str, &str)]) -> Variables { pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect() }

#[test] fn only_local_changed() { let m = merge(&v(&[("A","1")]), &v(&[("A","2")]), &v(&[("A","1")])); assert_eq!(m.result, v(&[("A","2")])); assert!(m.conflicts.is_empty()); }
#[test] fn only_cloud_changed() { /* base A=1, local A=1, cloud A=3 */ assert_eq!(m.result, v(&[("A","3")])); }
#[test] fn both_added_different_names() { /* base {}, local B=1, cloud C=2 */ assert_eq!(m.result, v(&[("B","1"),("C","2")])); }
#[test] fn both_changed_same_value_is_not_conflict() { /* base A=1, local A=2, cloud A=2 */ assert_eq!(m.result, v(&[("A","2")])); assert!(m.conflicts.is_empty()); }
#[test] fn both_changed_different_values_conflict() {
    let m = merge(&v(&[("A","1")]), &v(&[("A","2")]), &v(&[("A","3")]));
    assert_eq!(m.conflicts, vec![Conflict { name: "A".into(), local: Some("2".into()), cloud: Some("3".into()) }]);
    assert!(m.result.is_empty());
}
#[test] fn removed_on_one_side_unchanged_on_other_is_removed() { /* base A=1, local {}, cloud A=1 */ assert!(m.result.is_empty()); assert!(m.conflicts.is_empty()); }
#[test] fn removed_on_one_side_changed_on_other_conflicts() { /* base A=1, local {}, cloud A=5 */ assert_eq!(m.conflicts[0], Conflict { name: "A".into(), local: None, cloud: Some("5".into()) }); }
#[test] fn added_both_sides_different_values_conflict() { /* base {}, local A=1, cloud A=2 */ assert_eq!(m.conflicts.len(), 1); }
#[test] fn merge_after_lost_base_has_no_conflicts() { /* base A=1, local A=2,B=3, cloud A=2,B=3 */ assert_eq!(m.result, v(&[("A","2"),("B","3")])); assert!(m.conflicts.is_empty()); }
#[test] fn resolve_keeps_local_order() {
    let m = merge(&v(&[("A","1"),("B","1")]), &v(&[("A","2"),("B","1"),("Z","9")]), &v(&[("A","3"),("B","1")]));
    assert_eq!(resolve(m, &[Some("4".into())]), v(&[("A","4"),("B","1"),("Z","9")]));
}
#[test] fn resolve_none_removes() { /* conflict on A, choice None */ assert!(!result.iter().any(|(n, _)| n == "A")); }
#[test] fn choose_all_cloud() { assert_eq!(choose_all(&[Conflict { name: "A".into(), local: Some("2".into()), cloud: None }], Keep::Cloud), vec![None]); }
```

- [ ] **Step 2: Run to verify fail** — `cargo test cloud::merge` → FAIL (not defined)
- [ ] **Step 3: Implement `src/cloud/merge.rs`** per the rules above. Add `pub mod merge;` to `src/cloud/mod.rs`.
- [ ] **Step 4: Run to verify pass** — `cargo test cloud::merge` → PASS (12 tests)
- [ ] **Step 5: Commit** — `git add src/cloud && git commit -m "Add three-way merge for cloud sync"`

---

### Task 3: Sync state file

**Files:**
- Create: `src/cloud/state.rs` (tests inline, use a unique temp dir as `home`, same pattern as existing tests in `src/main.rs`)

**Interfaces:**
- Consumes: `master::atomic_write(path, bytes)`, `master::set_private_permissions(path)`
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)] pub struct Link { pub project: String, pub cloud_id: String, pub vault_id: String, pub version: i64, pub base: Variables }`
  - `#[derive(Serialize, Deserialize, Default, Debug, PartialEq)] pub struct State { pub user_id: Option<String>, pub email: Option<String>, pub links: Vec<Link>, #[serde(default, skip_serializing_if = "Option::is_none")] pub refresh_token: Option<String> }`
  - `pub fn path(home: &Path) -> PathBuf` → `home/.keyman_sync`
  - `pub fn load(home: &Path) -> Result<State, String>` — missing file → `State::default()`
  - `pub fn save(home: &Path, state: &State) -> Result<(), String>` — JSON, pretty, 600
  - `impl State { pub fn link(&self, project: &str) -> Option<&Link>; pub fn link_mut(&mut self, project: &str) -> Option<&mut Link>; pub fn set_link(&mut self, link: Link); pub fn remove_link(&mut self, project: &str) -> bool; pub fn link_by_cloud_id(&self, id: &str) -> Option<&Link> }`

- [ ] **Step 1: Write failing tests:** `missing_file_is_empty_state`, `save_then_load_round_trip`, `set_link_replaces_same_project`, `remove_link_reports_absence` (returns `false` when absent), `saved_file_is_private` (`#[cfg(unix)]`, mode `0o600`), `corrupt_file_is_an_error` (message contains `.keyman_sync`).
- [ ] **Step 2: Run to verify fail** — `cargo test cloud::state` → FAIL
- [ ] **Step 3: Implement `src/cloud/state.rs`.** Corrupt file error: `"cannot read {path}: {serde error}\n\nThe file is damaged. Remove it, then run 'keyman login' and 'keyman pull' again."`
- [ ] **Step 4: Run to verify pass** — `cargo test cloud::state` → PASS (6 tests)
- [ ] **Step 5: Commit** — `git commit -m "Add cloud sync state file"`

---

### Task 4: Supabase schema, RLS and functions

**Files:**
- Create: `supabase/config.toml`, `supabase/migrations/20261117000000_cloud_sync.sql`, `supabase/tests/rls_test.sql`

**Interfaces:**
- Produces (used by Task 5 `api.rs`):
  - Tables: `profiles`, `vaults`, `vault_members`, `invites`, `projects` (columns below)
  - RPC: `create_vault(p_kind text, p_name_sealed text, p_wrapped_key text) returns uuid`
  - RPC: `accept_invite(p_invite uuid, p_user uuid, p_wrapped_key text) returns void`
  - RPC: `rotate_vault(p_vault uuid, p_remove uuid, p_keys jsonb, p_projects jsonb) returns int` (returns new generation). `p_keys = [{"user_id": uuid, "wrapped_key": text}]`, `p_projects = [{"id": uuid, "version": int, "sealed": text}]` where `version` is the current version; the function writes `version + 1`.

- [ ] **Step 1: Initialize local Supabase.** Run `supabase init` (if `supabase/` does not exist). In `supabase/config.toml` set `[auth] additional_redirect_urls = ["http://127.0.0.1:*/callback"]` and enable `[auth.external.github]` and `[auth.external.google]` with `env(...)` client IDs/secrets. Check the Supabase docs for wildcard port support in redirect URLs; if wildcards are not supported, use the fixed port list `47821..47825` in both config and Task 5 `auth.rs`.

- [ ] **Step 2: Write the pgTAP tests** in `supabase/tests/rls_test.sql`. Create users A, B, C with `auth.users` rows and switch with `set local role authenticated; set local request.jwt.claims = '{"sub":"...","email":"..."}'`. Assertions:
  - `profile_insert_requires_own_email`: A inserts a profile with email of B → `throws_ok`.
  - `create_vault_makes_owner_member`: A calls `create_vault('group', 'x', 'k')` → A has a `vault_members` row with role `owner`, generation 1.
  - `second_personal_vault_refused`: A calls `create_vault('personal', ...)` twice → second `throws_ok`.
  - `non_member_cannot_read_projects`: B selects `projects` of A's vault → 0 rows.
  - `only_owner_invites`: after accept, B inserts into `invites` for A's vault → `throws_ok`.
  - `invitee_sees_own_invite`: B (email in invite) selects `invites` → 1 row.
  - `member_reads_invitee_profile`: A selects `profiles` where email = B's email → 1 row; C (no relation) selects A's profile → 0 rows.
  - `accept_invite_needs_matching_email`: A calls `accept_invite(invite_for_B, C_user_id, 'k')` → `throws_ok`.
  - `accept_invite_by_member`: A calls `accept_invite(invite_for_B, B_user_id, 'k')` → B is member, invite row gone.
  - `push_requires_current_version`: B updates project with filter `version = 0` → 0 rows changed; with `version = 1` and `version = 2` in new row → 1 row, version is 2.
  - `version_trigger_forces_increment`: update sets `version = 10` → stored version is old + 1.
  - `insert_requires_current_generation`: insert project with `key_generation = 99` → `throws_ok`.
  - `rotate_requires_owner`: B calls `rotate_vault` → `throws_ok`.
  - `rotate_requires_all_members_and_projects`: A calls with a missing project → `throws_ok`.
  - `rotate_removes_member`: A calls with correct data, removing B → B has no row, generation is 2, every project version incremented, B selects projects → 0 rows.
  - `rotate_refuses_stale_version`: project version changed after read → `throws_ok`.
  - `cannot_remove_owner`: `p_remove` = A → `throws_ok`.

- [ ] **Step 3: Run to verify fail** — `supabase start && supabase test db` → FAIL (tables not defined)

- [ ] **Step 4: Write the migration** `supabase/migrations/20261117000000_cloud_sync.sql`

```sql
create table public.profiles (
  user_id uuid primary key references auth.users on delete cascade,
  email text not null unique,
  public_key text not null,
  kdf_salt text not null,
  kdf_params jsonb not null,
  private_key_pw text not null,
  private_key_recovery text not null,
  created_at timestamptz not null default now()
);

create table public.vaults (
  id uuid primary key default gen_random_uuid(),
  kind text not null check (kind in ('personal', 'group')),
  owner_id uuid not null references auth.users on delete cascade,
  name_sealed text,
  key_generation int not null default 1,
  created_at timestamptz not null default now()
);
create unique index vaults_one_personal on public.vaults (owner_id) where kind = 'personal';

create table public.vault_members (
  vault_id uuid not null references public.vaults on delete cascade,
  user_id uuid not null references auth.users on delete cascade,
  role text not null check (role in ('owner', 'member')),
  wrapped_key text not null,
  key_generation int not null,
  primary key (vault_id, user_id)
);

create table public.invites (
  id uuid primary key default gen_random_uuid(),
  vault_id uuid not null references public.vaults on delete cascade,
  email text not null,
  invited_by uuid not null references auth.users on delete cascade,
  created_at timestamptz not null default now(),
  unique (vault_id, email)
);

create table public.projects (
  id uuid primary key default gen_random_uuid(),
  vault_id uuid not null references public.vaults on delete cascade,
  version int not null default 1,
  key_generation int not null,
  sealed text not null,
  updated_by uuid not null references auth.users,
  updated_at timestamptz not null default now()
);

alter table public.profiles enable row level security;
alter table public.vaults enable row level security;
alter table public.vault_members enable row level security;
alter table public.invites enable row level security;
alter table public.projects enable row level security;

create function public.my_vault_ids() returns setof uuid
language sql stable security definer set search_path = '' as $$
  select vault_id from public.vault_members where user_id = auth.uid()
$$;

create function public.is_vault_owner(v uuid) returns boolean
language sql stable security definer set search_path = '' as $$
  select exists (select 1 from public.vaults where id = v and owner_id = auth.uid())
$$;

create function public.vault_generation(v uuid) returns int
language sql stable security definer set search_path = '' as $$
  select key_generation from public.vaults where id = v
$$;

-- profiles
create policy profiles_select on public.profiles for select to authenticated using (
  user_id = auth.uid()
  or user_id in (select m.user_id from public.vault_members m where m.vault_id in (select public.my_vault_ids()))
  or email in (select i.email from public.invites i where i.vault_id in (select public.my_vault_ids()))
);
create policy profiles_insert on public.profiles for insert to authenticated
  with check (user_id = auth.uid() and email = (auth.jwt() ->> 'email'));
create policy profiles_update on public.profiles for update to authenticated
  using (user_id = auth.uid())
  with check (user_id = auth.uid() and email = (auth.jwt() ->> 'email'));

-- vaults
create policy vaults_select on public.vaults for select to authenticated
  using (id in (select public.my_vault_ids()));
create policy vaults_delete on public.vaults for delete to authenticated
  using (owner_id = auth.uid() and kind = 'group');

-- vault_members
create policy members_select on public.vault_members for select to authenticated
  using (vault_id in (select public.my_vault_ids()));

-- invites
create policy invites_select on public.invites for select to authenticated using (
  vault_id in (select public.my_vault_ids()) or email = (auth.jwt() ->> 'email')
);
create policy invites_insert on public.invites for insert to authenticated
  with check (public.is_vault_owner(vault_id) and invited_by = auth.uid());
create policy invites_delete on public.invites for delete to authenticated
  using (public.is_vault_owner(vault_id));

-- projects
create policy projects_select on public.projects for select to authenticated
  using (vault_id in (select public.my_vault_ids()));
create policy projects_insert on public.projects for insert to authenticated with check (
  vault_id in (select public.my_vault_ids())
  and updated_by = auth.uid()
  and version = 1
  and key_generation = public.vault_generation(vault_id)
);
create policy projects_update on public.projects for update to authenticated
  using (vault_id in (select public.my_vault_ids()))
  with check (
    vault_id in (select public.my_vault_ids())
    and updated_by = auth.uid()
    and key_generation = public.vault_generation(vault_id)
  );
create policy projects_delete on public.projects for delete to authenticated
  using (vault_id in (select public.my_vault_ids()));

create function public.projects_bump() returns trigger
language plpgsql set search_path = '' as $$
begin
  if new.vault_id <> old.vault_id or new.id <> old.id then
    raise exception 'a project cannot move to a different vault';
  end if;
  new.version := old.version + 1;
  new.updated_at := now();
  return new;
end $$;
create trigger projects_bump before update on public.projects
  for each row execute function public.projects_bump();

create function public.create_vault(p_kind text, p_name_sealed text, p_wrapped_key text) returns uuid
language plpgsql security definer set search_path = '' as $$
declare v uuid;
begin
  if auth.uid() is null then raise exception 'sign in first'; end if;
  insert into public.vaults (kind, owner_id, name_sealed) values (p_kind, auth.uid(), p_name_sealed) returning id into v;
  insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation)
    values (v, auth.uid(), 'owner', p_wrapped_key, 1);
  return v;
end $$;

create function public.accept_invite(p_invite uuid, p_user uuid, p_wrapped_key text) returns void
language plpgsql security definer set search_path = '' as $$
declare inv public.invites; gen int;
begin
  select * into inv from public.invites where id = p_invite for update;
  if not found then raise exception 'the invite does not exist'; end if;
  if not exists (select 1 from public.vault_members where vault_id = inv.vault_id and user_id = auth.uid()) then
    raise exception 'only a member can accept an invite';
  end if;
  if not exists (select 1 from public.profiles where user_id = p_user and email = inv.email) then
    raise exception 'the user does not match the invite email';
  end if;
  select key_generation into gen from public.vaults where id = inv.vault_id for update;
  insert into public.vault_members (vault_id, user_id, role, wrapped_key, key_generation)
    values (inv.vault_id, p_user, 'member', p_wrapped_key, gen);
  delete from public.invites where id = p_invite;
end $$;

create function public.rotate_vault(p_vault uuid, p_remove uuid, p_keys jsonb, p_projects jsonb) returns int
language plpgsql security definer set search_path = '' as $$
declare gen int; owner uuid; k jsonb; p jsonb; changed int;
begin
  select owner_id, key_generation into owner, gen from public.vaults where id = p_vault for update;
  if not found or owner <> auth.uid() then raise exception 'only the owner can remove a member'; end if;
  if p_remove = owner then raise exception 'the owner cannot be removed'; end if;
  delete from public.vault_members where vault_id = p_vault and user_id = p_remove;
  if not found then raise exception 'the user is not a member'; end if;
  gen := gen + 1;
  if (select count(*) from public.vault_members where vault_id = p_vault) <> jsonb_array_length(p_keys) then
    raise exception 'a new key is necessary for each remaining member';
  end if;
  for k in select * from jsonb_array_elements(p_keys) loop
    update public.vault_members set wrapped_key = k->>'wrapped_key', key_generation = gen
      where vault_id = p_vault and user_id = (k->>'user_id')::uuid;
    if not found then raise exception 'a key is for a user who is not a member'; end if;
  end loop;
  if (select count(*) from public.projects where vault_id = p_vault) <> jsonb_array_length(p_projects) then
    raise exception 'all projects of the vault must be encrypted again';
  end if;
  update public.vaults set key_generation = gen where id = p_vault;
  for p in select * from jsonb_array_elements(p_projects) loop
    update public.projects set sealed = p->>'sealed', key_generation = gen, updated_by = auth.uid()
      where id = (p->>'id')::uuid and vault_id = p_vault and version = (p->>'version')::int;
    get diagnostics changed = row_count;
    if changed <> 1 then raise exception 'a project changed during the removal; try again'; end if;
  end loop;
  return gen;
end $$;

revoke execute on function public.create_vault, public.accept_invite, public.rotate_vault from anon, public;
grant execute on function public.create_vault, public.accept_invite, public.rotate_vault to authenticated;
```

Note: the update policy's `with check` uses `vault_generation(vault_id)`; inside `rotate_vault` the vault generation is updated before the project updates, so new rows pass. The trigger sets `version = old.version + 1`, so rotated ciphertext must be sealed for `version + 1` (Task 8 does this).

- [ ] **Step 5: Run to verify pass** — `supabase db reset && supabase test db` → PASS (all pgTAP tests)
- [ ] **Step 6: Commit** — `git add supabase && git commit -m "Add Supabase schema and RLS for cloud sync"`

---

### Task 5: Config, HTTP client and browser sign-in

**Files:**
- Modify: `Cargo.toml` (add `ureq = { version = "3", features = ["json"] }`, `sha2 = "0.10"`, `webbrowser = "1"`)
- Create: `src/cloud/config.rs`, `src/cloud/api.rs`, `src/cloud/auth.rs` (tests inline)

**Interfaces:**
- Produces:
  - `config.rs`: `pub struct Config { pub url: String, pub key: String }`, `pub fn config() -> Config` — constants `DEFAULT_URL` and `DEFAULT_KEY` (filled in Task 10), env overrides `KEYMAN_SUPABASE_URL`, `KEYMAN_SUPABASE_KEY`.
  - `auth.rs`:
    - `pub struct Pkce { pub verifier: String, pub challenge: String }`, `pub fn pkce() -> Pkce` (verifier: 64 chars from `A-Za-z0-9-._~`; challenge: base64url no-pad of SHA-256)
    - `pub fn authorize_url(cfg: &Config, provider: &str, redirect: &str, challenge: &str) -> String` → `{url}/auth/v1/authorize?provider={provider}&redirect_to={enc(redirect)}&code_challenge={challenge}&code_challenge_method=s256`
    - `pub fn sign_in_with_browser(cfg: &Config, provider: &str) -> Result<Session, String>` — binds `TcpListener` on `127.0.0.1:0`, opens the URL with `webbrowser::open` (prints it too), waits up to 300 s for `GET /callback?code=...`, replies with a small HTML page ("Sign-in is complete. You can close this window."), then calls `api::exchange_code`.
    - `pub fn parse_callback(request_line: &str) -> Result<String, String>` — returns `code`; an `error_description` parameter becomes the error text.
  - `api.rs`:
    - `#[derive(Clone, Serialize, Deserialize)] pub struct Session { pub access_token: String, pub refresh_token: String, pub expires_at: i64, pub user_id: String, pub email: String }`
    - `pub fn exchange_code(cfg: &Config, code: &str, verifier: &str) -> Result<Session, String>` — `POST /auth/v1/token?grant_type=pkce` body `{"auth_code", "code_verifier"}`
    - `pub fn refresh(cfg: &Config, refresh_token: &str) -> Result<Session, String>` — `POST /auth/v1/token?grant_type=refresh_token`
    - `pub fn sign_out(cfg: &Config, session: &Session) -> Result<(), String>` — `POST /auth/v1/logout`
    - `pub struct Api { cfg: Config, session: Session }` with `Api::new(cfg, session) -> Api`, and:
      - `get_profile(&self, user_id: &str) -> Result<Option<Profile>, String>`
      - `find_profile_by_email(&self, email: &str) -> Result<Option<Profile>, String>`
      - `put_profile(&self, p: &Profile) -> Result<(), String>` (upsert, `Prefer: resolution=merge-duplicates`)
      - `my_memberships(&self) -> Result<Vec<Membership>, String>` (`vault_members` where `user_id = me`, embedded `vaults(kind,owner_id,name_sealed,key_generation)`)
      - `vault_members(&self, vault_id: &str) -> Result<Vec<Membership>, String>`
      - `create_vault(&self, kind: &str, name_sealed: Option<&str>, wrapped_key: &str) -> Result<String, String>` (RPC)
      - `delete_vault(&self, vault_id: &str) -> Result<(), String>`
      - `projects(&self, vault_ids: &[String]) -> Result<Vec<CloudProject>, String>`
      - `project(&self, id: &str) -> Result<Option<CloudProject>, String>`
      - `insert_project(&self, vault_id: &str, key_generation: i64, sealed_for: impl Fn(&str) -> String) -> Result<CloudProject, String>` — generates the UUID on the client (`getrandom`, v4 format) so it can seal with the ID before insert
      - `update_project(&self, id: &str, base_version: i64, key_generation: i64, sealed: &str) -> Result<Option<CloudProject>, String>` — `PATCH ...?id=eq.{id}&version=eq.{base}` with `Prefer: return=representation`; empty array → `Ok(None)` (cloud is newer)
      - `delete_project(&self, id: &str) -> Result<(), String>`
      - `invites_for_vaults(&self, vault_ids: &[String]) -> Result<Vec<Invite>, String>`, `my_invites(&self) -> Result<Vec<Invite>, String>`
      - `create_invite(&self, vault_id: &str, email: &str) -> Result<(), String>`
      - `accept_invite(&self, invite_id: &str, user_id: &str, wrapped_key: &str) -> Result<(), String>`
      - `rotate_vault(&self, vault_id: &str, remove: &str, keys: &[(String, String)], projects: &[(String, i64, String)]) -> Result<i64, String>`
    - Row types (`Deserialize`, field names = column names): `Profile { user_id, email, public_key, kdf_salt, kdf_params: KdfParams, private_key_pw, private_key_recovery }`, `Membership { vault_id, user_id, role, wrapped_key, key_generation: i64, vaults: VaultRow }`, `VaultRow { kind, owner_id, name_sealed: Option<String>, key_generation: i64 }`, `CloudProject { id, vault_id, version: i64, key_generation: i64, sealed, updated_at }`, `Invite { id, vault_id, email }`.
    - Every request sends `apikey: {key}` and `Authorization: Bearer {access_token}`. HTTP errors become `Err` with the server's `message` field. A connection failure becomes `"cannot connect to the Keyman server. Check the network connection, then try again."`

- [ ] **Step 1: Write failing tests** (pure, no network):
  - `pkce_challenge_matches_rfc7636_example`: for verifier `dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk`, challenge is `E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM` (expose `pub fn challenge_for(verifier: &str) -> String`).
  - `pkce_verifier_is_valid`: length 64, only unreserved characters.
  - `authorize_url_encodes_redirect`: contains `redirect_to=http%3A%2F%2F127.0.0.1%3A5000%2Fcallback`.
  - `parse_callback_reads_code`: `"GET /callback?code=abc&state=x HTTP/1.1"` → `Ok("abc")`.
  - `parse_callback_reports_error`: `"GET /callback?error=access_denied&error_description=User+denied HTTP/1.1"` → `Err` containing `User denied`.
  - `config_env_overrides`: with env vars set (use a serial test helper or `std::env::set_var` in one test only) → values used.
- [ ] **Step 2: Run to verify fail** — `cargo test cloud::auth cloud::config` → FAIL
- [ ] **Step 3: Implement** `config.rs`, `auth.rs`, `api.rs`. The `api.rs` HTTP methods have no unit tests; Task 10 E2E tests cover them.
- [ ] **Step 4: Run to verify pass** — `cargo test cloud::` → PASS
- [ ] **Step 5: Commit** — `git commit -m "Add Supabase client and browser sign-in"`

---

### Task 6: Credential store and account (login, setup, unlock, recover, logout)

**Files:**
- Modify: `Cargo.toml` (add `keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }`, `rpassword = "7"`)
- Create: `src/cloud/keystore.rs`, `src/cloud/account.rs` (tests inline)
- Modify: `src/cli.rs`, `src/main.rs`, `src/completions.rs`

**Interfaces:**
- Consumes: Task 1 crypto, Task 3 `state`, Task 5 `api`, `auth`.
- Produces:
  - `keystore.rs`: `pub trait Store { fn get(&self, name: &str) -> Result<Option<String>, String>; fn set(&self, name: &str, value: &str) -> Result<(), String>; fn delete(&self, name: &str) -> Result<(), String>; }`, `pub struct SystemStore;` (keyring, service `keyman`), `pub struct MemoryStore(RefCell<HashMap<String,String>>)` for tests, `pub fn system_available() -> bool`. (Two real implementations, so the trait is justified.)
  - `account.rs`:
    - `pub struct Unlocked { pub api: Api, pub pair: KeyPair, pub user_id: String }`
    - `pub trait Prompt { fn new_password(&mut self) -> Result<String, String>; fn password(&mut self) -> Result<String, String>; fn recovery_code(&mut self) -> Result<String, String>; fn show_recovery_code(&mut self, code: &str) -> Result<(), String>; }` (CLI terminal and tests implement it; Plan 2 GUI implements it too)
    - `pub fn login(home: &Path, store: &dyn Store, provider: &str, prompt: &mut dyn Prompt) -> Result<LoginOutcome, String>` — browser sign-in, then `setup` (no profile) or `unlock` (profile exists). `pub enum LoginOutcome { Created, Unlocked }`
    - `pub fn setup_profile(api: &Api, store: &dyn Store, prompt: &mut dyn Prompt, params: KdfParams) -> Result<KeyPair, String>` — spec 7.2 steps 1–9; also calls `create_vault("personal", None, wrap_for(public, vault_key))`
    - `pub fn unlock_with_password(profile: &Profile, password: &str) -> Result<KeyPair, String>`
    - `pub fn unlock_with_recovery(profile: &Profile, code: &str) -> Result<KeyPair, String>`
    - `pub fn recover(home: &Path, store: &dyn Store, prompt: &mut dyn Prompt) -> Result<(), String>` — recovery code, then new password, re-encrypt `private_key_pw` with a new salt (the recovery copy stays valid: it uses its own derived key from the same salt, so `recover` re-seals **both** copies with the new salt)
    - `pub fn session(home: &Path, store: &dyn Store, prompt: &mut dyn Prompt) -> Result<Unlocked, String>` — loads refresh token (store, or `state.refresh_token` when `!system_available()`), refreshes, saves the new refresh token, loads the private key from the store (or calls `prompt.password()` and unlocks when the store is unavailable). Error when not signed in: `"You are not signed in. Run 'keyman login' first."`
    - `pub fn change_password(home: &Path, store: &dyn Store, prompt: &mut dyn Prompt) -> Result<(), String>` — asks the current password, then a new one; re-seals only `private_key_pw` with the **same** salt (the recovery copy depends on that salt, and Keyman does not have the recovery code here). No project changes.
    - `pub fn logout(home: &Path, store: &dyn Store) -> Result<(), String>` — sign out on the server (ignore network errors), delete store entries, clear `user_id`, `email`, `refresh_token` in state (links stay)
  - Wrong password or recovery code → `Err("The vault password is not correct.")` / `Err("The recovery code is not correct.")`.

- [ ] **Step 1: Write failing tests** (no network: build a `Profile` by hand with `FAST` params):
  - `setup_values_unlock_with_password` — make profile values the same way `setup_profile` does (expose `pub fn new_profile_values(user_id, email, password, recovery, params) -> (Profile, KeyPair)`), then `unlock_with_password` gives the same public key.
  - `setup_values_unlock_with_recovery` — same with the recovery code, also with lowercase and spaces.
  - `wrong_password_is_reported` — error text is exactly `The vault password is not correct.`
  - `wrong_recovery_code_is_reported`.
  - `changed_password_keeps_recovery_code_valid` — re-seal `private_key_pw` with a new password and the same salt (expose `pub fn reseal_password(profile: &Profile, pair: &KeyPair, new_password: &str) -> Result<Profile, String>`); the new password unlocks, the old password fails, the original recovery code still unlocks.
  - `memory_store_round_trip`.
- [ ] **Step 2: Run to verify fail** — `cargo test cloud::account cloud::keystore` → FAIL
- [ ] **Step 3: Implement** `keystore.rs`, `account.rs`.
- [ ] **Step 4: Add CLI commands** in `src/cli.rs` (new group `"Cloud"` added to `GROUPS` after `"More"`), dispatch in `src/main.rs`, names in `src/completions.rs`:

| Command | args | flags | summary |
| --- | --- | --- | --- |
| `login` | `` | `--github`, `--google` | `Sign in to Keyman Cloud` |
| `logout` | `` | | `Sign out and remove the keys from this computer` |
| `recover` | `` | | `Set a new vault password with your recovery code` |
| `password` | `` | | `Change your vault password` |

`login` without a provider flag asks: `Sign in with: 1) GitHub  2) Google`. Setup output (STE):

```
Make a vault password. Keyman uses it to encrypt your secrets.
Keyman cannot reset this password.
...
Your recovery code:

    XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX

Write down this code and keep it in a safe place.
If you lose the password and this code, nobody can get your cloud data back.
Type the code again to confirm:
✓ Signed in as {email}. Keyman Cloud is ready.
```

`TerminalPrompt` in `src/main.rs` implements `Prompt` with `rpassword::prompt_password`. `new_password` asks two times and requires at least 10 characters (`The password must have 10 characters or more.`); different entries give `The two passwords are not the same.`.

- [ ] **Step 5: Add CLI parse tests** in `src/cli.rs` tests: `login_accepts_provider_flag`, `cloud_group_in_help` (help text contains `Cloud` and `login`).
- [ ] **Step 6: Run** — `cargo test` → PASS; `cargo clippy --all-targets -- -D warnings` → no warnings
- [ ] **Step 7: Commit** — `git commit -m "Add cloud account: login, setup, unlock, recover, logout"`

---

### Task 7: Link, push, pull, status, delete

**Files:**
- Create: `src/cloud/sync.rs` (tests inline for the pure parts)
- Modify: `src/cli.rs` (value flags + commands), `src/main.rs`, `src/completions.rs`

**Interfaces:**
- Consumes: `account::session`, `api::Api`, `crypto::{seal_project, open_project, unwrap}`, `merge::{merge, resolve, choose_all, Conflict, Keep}`, `state`, `master::{read_master, project_variables, atomic_write}`, `save::{plan, update_project}`
- Produces:
  - `pub struct Vaults { by_id: HashMap<String, (Key, i64)> }` with `pub fn load(u: &Unlocked) -> Result<Vaults, String>` (unwraps every membership key) and `pub fn key(&self, vault_id: &str) -> Result<&(Key, i64), String>`; `pub fn personal_vault(u: &Unlocked) -> Result<String, String>`
  - `pub fn link(home: &Path, u: &Unlocked, project: &str, group: Option<&str>) -> Result<Link, String>` — encrypts and inserts the project, stores the link with `version` and `base` = current local variables. Error if already linked: `Project '{p}' is already linked.`
  - `pub fn unlink(home: &Path, project: &str) -> Result<(), String>`
  - `pub enum PushResult { Pushed { project: String, version: i64 }, UpToDate(String), CloudIsNewer(String), Skipped { project: String, reason: String } }`
  - `pub fn push(home: &Path, u: &Unlocked, project: Option<&str>) -> Result<Vec<PushResult>, String>`
  - `pub trait Resolver { fn resolve(&mut self, project: &str, conflicts: &[Conflict]) -> Result<Option<Vec<Option<String>>>, String>; }` — `None` = stop without change
  - `pub enum PullResult { Updated { project: String, changes: usize }, Added(String), UpToDate(String), Stopped { project: String, conflicts: usize }, Skipped { project: String, reason: String } }`
  - `pub fn pull(home: &Path, u: &Unlocked, project: Option<&str>, local_name: Option<&str>, resolver: &mut dyn Resolver) -> Result<Vec<PullResult>, String>`
  - `pub fn pull_merge(base: &[(String,String)], local: &[(String,String)], cloud: &[(String,String)], resolver: ...) -> Result<Option<Variables>, String>` (pure helper, tested)
  - `pub fn write_project(yaml: &str, project: &str, exists: bool, variables: &[(String,String)]) -> Result<String, String>` — `save::plan(existing, variables, true)` then `save::update_project`
  - `pub enum LinkStatus { Synced, LocalChanges, Missing }` and `pub fn local_status(home: &Path) -> Result<Vec<(String, LinkStatus)>, String>` (offline; compares local to base)
  - `pub fn delete_cloud(home: &Path, u: &Unlocked, project: &str) -> Result<(), String>`

Push algorithm, per link: local missing → `Skipped` with reason `Project '{p}' is not in the master file. Run 'keyman unlink {p}' to remove the link.`; local == base → `UpToDate`; else seal with `version = link.version + 1` and `update_project(id, link.version, gen, sealed)`; `None` → `CloudIsNewer`; `Some(row)` → set `link.version = row.version`, `link.base = local`, save state after each project.

Pull algorithm, per cloud project in my vaults: decrypt; find link by cloud ID.
- No link: name from blob (or `--as`). If the name exists locally and is not linked → `Skipped` with `A local project named '{n}' exists. Use --as <name>.` Else write project, add link → `Added`.
- Link: local missing → `Skipped` (same message as push). `cloud.version == link.version` and local == base → `UpToDate`. Else `pull_merge(base, local, cloud)`. Conflicts → `resolver.resolve`; `None` → `Stopped`. Then `write_project`; on error return the error for that project **before** any write and keep the base. On success: atomic write of the master file, then set `link.version`, `link.base = cloud vars` (not the merged result, so a later push sends local changes), save state.

Before the loop, `pull` calls `groups::accept_pending(u)` (Task 8). Until Task 8 lands, this is a no-op function in `sync.rs`.

- [ ] **Step 1: Add value flags to `cli::parse`.** A flag spec written `"--group <name>"` takes the next argument. `Parsed::Args.flags` becomes `Vec<(&'static str, Option<&'a OsString>)>`; update existing call sites (`flags.contains(&"--reveal")` → a helper `has(&flags, "--reveal")`, `value(&flags, "--group")`). Tests: `value_flag_takes_next_argument`, `value_flag_without_value_is_error` (message: `the option '--group' needs a value`), existing tests still pass.
- [ ] **Step 2: Write failing tests** in `src/cloud/sync.rs`:
  - `pull_merge_without_conflicts_returns_result`
  - `pull_merge_calls_resolver_on_conflict` (fake `Resolver` returns `Some(vec![Some("x")])` → result has `A=x`)
  - `pull_merge_stops_when_resolver_declines` → `Ok(None)`
  - `write_project_adds_new_project` and `write_project_replaces_variables` (removed variables are gone; comments of other projects stay)
  - `refused_layout_writes_nothing` — project in flow style (`app: {A: "1"}`) → `write_project` returns `Err` and the input text is unchanged.
  - `missing_local_project_is_skipped_with_message` — `local_status` with a link to a project not in the master file → `Missing`; and the push message builder `skip_reason_missing("app")` contains `keyman unlink app`.
  - `local_status_detects_local_changes`
- [ ] **Step 3: Run to verify fail** — `cargo test cloud::sync cli::` → FAIL
- [ ] **Step 4: Implement `src/cloud/sync.rs`.**
- [ ] **Step 5: Add CLI commands:**

| Command | args | flags | summary |
| --- | --- | --- | --- |
| `link` | `[project]` | `--group <name>` | `Connect a project to Keyman Cloud` |
| `unlink` | `[project]` | | `Remove the cloud link on this computer` |
| `push` | `[project]` | | `Send local changes to the cloud` |
| `pull` | `[project]` | `--as <name>`, `--keep-local`, `--keep-cloud` | `Get changes from the cloud and merge them` |
| `cloud` | `<action> [project]` | | `Cloud status and cloud delete` (`keyman cloud status`, `keyman cloud delete <project>`) |

Project defaults to the current folder name for `link` and `unlink` (same rule as `inject`, via the existing `project_for` logic); for `push` and `pull`, no project means all links. `TerminalResolver` in `src/main.rs`: when stdin is a terminal (`std::io::IsTerminal`) ask for each conflict:

```
Conflict in my-app: API_KEY
  1) This computer: ••••••••
  2) Cloud:         ••••••••
  3) Type a new value
  4) Show the values
Select 1, 2, 3 or 4:
```

Without a terminal and without `--keep-local`/`--keep-cloud`: return `None` and print `Pull stopped: {n} conflicts in {project}. Run 'keyman pull' in a terminal, or add --keep-local or --keep-cloud.`; exit code 1. `CloudIsNewer` prints `The cloud has newer changes for {project}. Run 'keyman pull' first.` and exit code 1. `cloud delete` asks `Delete {project} from the cloud for all members? Type the project name to confirm:`. `cloud status` works offline (uses `local_status`) and prints `✓ synced`, `● local changes`, `✗ not in master file` for each link, then `Run 'keyman pull' to check the cloud for new changes.`

- [ ] **Step 6: Run** — `cargo test && cargo clippy --all-targets -- -D warnings` → PASS, no warnings
- [ ] **Step 7: Commit** — `git commit -m "Add cloud link, push, pull, status and delete"`

---

### Task 8: Groups

**Files:**
- Create: `src/cloud/groups.rs`
- Modify: `src/cloud/sync.rs` (replace the `accept_pending` stub), `src/cli.rs`, `src/main.rs`, `src/completions.rs`

**Interfaces:**
- Consumes: `api::Api` (vault, invite, rotate calls), `crypto::{Key, wrap_for, seal, open, seal_project, open_project, vault_name_aad}`, `sync::Vaults`
- Produces:
  - `pub struct Group { pub vault_id: String, pub name: String, pub owner: bool, pub members: Vec<String>, pub invited: Vec<String> }`
  - `pub fn create(u: &Unlocked, name: &str) -> Result<String, String>` — new vault key; `create_vault("group", seal(key, vault_name_aad(id)...))`. The vault ID is not known before the RPC, so: create with `name_sealed = None`, then seal the name with the returned ID and `PATCH` it — add `Api::set_vault_name(&self, vault_id, name_sealed)` and an RLS policy `vaults_update_name` (owner may update `name_sealed` only; add a column-level `grant update (name_sealed) on public.vaults to authenticated` in a new migration `20261117000100_vault_name.sql` with a pgTAP test `owner_sets_name_only`).
  - `pub fn find(u: &Unlocked, vaults: &Vaults, name: &str) -> Result<String, String>` — group name → vault ID; error `There is no group named '{name}'.`
  - `pub fn invite(u: &Unlocked, group: &str, email: &str) -> Result<(), String>` — owner only (else `Only the owner of the group can invite persons.`)
  - `pub fn accept_pending(u: &Unlocked, vaults: &Vaults) -> Result<Vec<(String, String)>, String>` — for each invite in my vaults whose email has a profile: `accept_invite(id, profile.user_id, wrap_for(profile.public_key, vault_key))`. Returns `(group, email)` pairs to print: `Gave {email} access to {group}.`
  - `pub fn waiting(u: &Unlocked) -> Result<usize, String>` — my own pending invites (shown by `pull`: `Waiting for access to {n} groups. A member must run 'keyman pull'.`)
  - `pub fn remove(u: &Unlocked, vaults: &Vaults, group: &str, email: &str) -> Result<(), String>` — new key; re-seal each project with `version + 1` (open with old key, `seal_project(new, id, version + 1, ...)`); wrap new key for each remaining member's public key; `rotate_vault`. Error `a project changed during the removal` from the server → message `A member changed a project during the removal. Run the command again.`
  - `pub fn list(u: &Unlocked, vaults: &Vaults) -> Result<Vec<Group>, String>`
  - `pub fn delete(u: &Unlocked, vaults: &Vaults, group: &str) -> Result<(), String>` — owner only; confirmation in CLI.

- [ ] **Step 1: Write failing tests** (pure parts): `reseal_for_rotation_uses_next_version` — expose `pub fn reseal(old: &Key, new: &Key, id: &str, version: i64, sealed: &str) -> Result<String, String>`; the result opens with `new` at `version + 1` and not at `version`; `reseal_with_wrong_old_key_fails`.
- [ ] **Step 2: Run to verify fail** — `cargo test cloud::groups` → FAIL
- [ ] **Step 3: Implement `src/cloud/groups.rs`** and the new migration + pgTAP test.
- [ ] **Step 4: Add CLI command** `group` with args `<action> [group] [email]`, actions `create <group>`, `invite <group> <email>`, `remove <group> <email>`, `list`, `delete <group>`. Unknown action → `unknown action '{a}' for 'keyman group'` with `did_you_mean`. `group list` output:

```
Groups · 2

  team-api   owner · 3 members · 1 invited
  billing    member · 2 members
```

`group remove` prints, before it asks for confirmation: `Keyman makes a new group key and encrypts all {n} projects again. {email} keeps the data that they got before.`

- [ ] **Step 5: Run** — `cargo test && supabase test db` → PASS
- [ ] **Step 6: Commit** — `git commit -m "Add cloud share groups"`

---

### Task 9: Offline guarantee and docs

**Files:**
- Modify: `src/main.rs` (test), `README.md`

- [ ] **Step 1: Write the test** `existing_commands_do_no_network` in `src/main.rs` tests: set `KEYMAN_SUPABASE_URL=http://192.0.2.1:9` (TEST-NET, unroutable), write a `~/.keyman_sync` with a link and a refresh token in the temp home, then run `execute` for `status`, `show`, `inject`, `diff`, `save --dry-run`, `export` against the temp home. Each must succeed in under 2 s total. (A call to the cloud would hang on the unroutable address.)
- [ ] **Step 2: Run** — `cargo test existing_commands_do_no_network` → PASS
- [ ] **Step 3: README.** Add a "Cloud sync (optional)" section after "Everyday use", in ASD-STE100: what the server keeps, `login`, `link`, `push`, `pull`, conflicts, groups, recovery code warning, and the accepted invite risk (one sentence). Add the new commands to the Commands table.
- [ ] **Step 4: Commit** — `git commit -m "Document cloud sync and test offline behavior"`

---

### Task 10: Hosted Supabase project and end-to-end tests

**Files:**
- Modify: `src/cloud/config.rs` (constants)
- Create: `tests/cloud_e2e.rs`

**Interfaces:**
- Consumes: the whole `cloud` module. Tests sign users in with the local Supabase admin API (`POST /auth/v1/admin/users` with the service role key from `supabase status`, then `POST /auth/v1/token?grant_type=password`), so no browser is needed. Expose `pub fn account::setup_with(api: &Api, store: &dyn Store, password: &str, params: KdfParams) -> Result<(KeyPair, String /*recovery*/), String>` for tests.

- [ ] **Step 1: Ask the user which Supabase project to use** (new or existing; creating one can cost money). Use the Supabase tools to apply both migrations to it, enable GitHub and Google providers (the user enters the OAuth client secrets in the Supabase dashboard; never type secrets for them), add the redirect URL from Task 4, and copy the project URL and publishable key into `config.rs`. Run the Supabase security advisors; fix any finding on the new tables.
- [ ] **Step 2: Write E2E tests** (each `#[test]` returns early unless `KEYMAN_E2E=1`):
  - `two_computers_push_and_pull` — computer 1 links and pushes; computer 2 (second temp home, same user, `MemoryStore`) pulls → same variables.
  - `conflict_is_resolved_with_keep_cloud`
  - `push_refused_when_cloud_is_newer`
  - `server_rows_hold_no_plain_text` — with the service role, read all `projects.sealed` and `vaults.name_sealed`; none contains the project name, a variable name or a value.
  - `invite_then_accept_on_pull` — A invites B; B sets up; B `waiting == 1`; A pulls; B pulls → B has the group project.
  - `remove_rotates_key` — A removes B; B cannot read projects; A and C still pull; generation is 2.
  - `recover_sets_new_password` — recover with the code, then unlock with the new password works and the old password fails.
  - `expired_access_token_is_refreshed` — store a session with `expires_at` in the past → `session()` succeeds.
- [ ] **Step 3: Run** — `supabase start && KEYMAN_E2E=1 cargo test --test cloud_e2e -- --test-threads=1` → PASS
- [ ] **Step 4: Manual check of browser sign-in** — `cargo run -- login --github` against the hosted project; report the result to the user.
- [ ] **Step 5: Commit** — `git commit -m "Connect hosted Supabase and add cloud end-to-end tests"`

---

## After This Plan

Plan 2 (desktop app): Settings › Account (sign-in, setup, recovery code screen), **Sync** and **Link to cloud** buttons, conflicts in the current **Resolve differences** dialog (it implements `sync::Resolver`), Settings › Groups, cloud status marks in the sidebar, English and Thai text. It is written after this plan is complete, so it can use the real `cloud` interfaces.
