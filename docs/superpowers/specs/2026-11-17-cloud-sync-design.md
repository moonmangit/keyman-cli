# Keyman Cloud Sync: Design

Date: 2026-11-17
Status: Draft, waiting for approval
Language: ASD-STE100 (Simplified Technical English)

## 1. Intent

Keyman keeps all project secrets in one local master YAML file. Users want these functions:

- Keep a copy of their secrets on a server, so that they can use the secrets on more than one computer.
- Share the secrets of a project with co-workers.

The server must not keep secrets as plain text. The function must be easy to use.

Cloud sync is optional. A user who does not sign in uses Keyman as before. Keyman then does no network operation.

## 2. Goals

1. The server keeps only encrypted data. The server cannot read project names, variable names or values.
2. A user signs in with GitHub or Google (OAuth through Supabase).
3. A user has one vault password. The user types it one time on each computer.
4. A user can make a group, invite co-workers, and remove co-workers.
5. All group members can read and change the projects of the group.
6. The CLI and the desktop app have the same functions. Both use the shared library (`src/`).
7. The user starts each sync. Keyman does not sync automatically.

## 3. Non-goals

- Automatic sync or background sync.
- A web interface.
- Sync of `.env` files. Keyman syncs only the data of the master file. `.env` files stay local, as before.
- Share of one variable. The smallest shared unit is one project.
- Roles other than owner and member (for example, read-only members).
- An audit log of changes.
- Recovery of data when the user loses the vault password and the recovery code.

## 4. Anti-goals

The design fails if one of these conditions occurs, also when the function operates:

- The server, or a person with access to the database, can read a secret, a variable name or a project name.
- A user who does not sign in sees a change in behavior, a delay, or a network request.
- A change from one person replaces a change from a different person, and no person sees it.

## 5. Approaches

Three approaches were examined. The user selected approach A.

| Approach | Description | Advantage | Disadvantage |
| --- | --- | --- | --- |
| **A. Vault password (selected)** | The user selects one vault password. Keyman derives a key from it. The operating system keeps the key on each computer. | Easy. Familiar. A new computer needs only the password. | If the user forgets the password and loses the recovery code, the cloud data is lost. |
| B. Device keys and recovery code | Each computer has its own key. The user approves a new computer from an old computer, or types a recovery code. | No password to remember. | A new computer is difficult to add. Loss of the recovery code and all computers causes data loss. |
| C. User-managed key files | The user keeps and moves key files. | Full control. | Difficult to use. This is against the goal "easy to use". |

Sync time: the user compared automatic sync and manual sync. The user selected **manual sync**.

Conflicts: the user selected **merge by variable**. When two persons change the same variable, the user resolves it.

## 6. Decisions from the user

1. Approach A: one vault password for each user, and one recovery code.
2. Encrypt all data: project names, variable names and values.
3. Groups have one owner and members. All members can read and change. Only the owner can invite and remove.
4. Merge by variable. When two persons change the same variable, the **Resolve differences** screen opens.
5. Manual sync: `keyman push`, `keyman pull`, and a **Sync** button in the app.
6. Invite confirmation by email match only. No fingerprint check. The user accepts the risk in 7.5.

## 7. Design

### 7.1 Words used in this document

| Word | Meaning |
| --- | --- |
| Vault | A set of cloud projects that share one vault key. Each user has one personal vault. Each group is one vault. |
| Vault key | A random symmetric key. It encrypts all projects in one vault. |
| Account key pair | An X25519 public key and private key for each user. Other users encrypt vault keys with the public key. |
| Password key | A key that Keyman derives from the vault password with Argon2id. |
| Recovery key | A key that Keyman derives from the recovery code. |
| Link | A record that connects one local project to one cloud project. |
| Base | The last version of a project that this computer pulled or pushed. Keyman uses it to merge. |

### 7.2 Keys

**At the first sign-in (setup):**

1. Keyman makes the account key pair.
2. The user types a new vault password two times.
3. Keyman derives the password key (Argon2id, random salt).
4. Keyman encrypts the private key with the password key.
5. Keyman makes a random recovery code and shows it one time. The user must write it down.
6. Keyman encrypts the private key with the recovery key too.
7. Keyman makes the personal vault and its vault key. Keyman encrypts the vault key with the public key of the user.
8. Keyman sends to the server: the public key, the two encrypted copies of the private key, the salt and the Argon2id parameters, and the encrypted vault key.
9. Keyman keeps the private key in the operating system credential store (macOS Keychain, Windows Credential Manager, Linux Secret Service).

**On a new computer:**

1. The user signs in.
2. Keyman gets the encrypted private key from the server.
3. The user types the vault password. Keyman decrypts the private key.
4. Keyman keeps the private key in the credential store.

**When the user forgets the password:**

1. The user types the recovery code.
2. Keyman decrypts the private key.
3. The user selects a new vault password. Keyman encrypts the private key again and sends it.

**Change of password:** Keyman encrypts the private key again with the new password key. No project changes.

**Algorithms** (libsodium-equivalent constructions):

- Password and recovery code to key: Argon2id.
- Project data: XChaCha20-Poly1305 with a random nonce for each encryption. The associated data contains the project ID and the version, so the server cannot move a ciphertext to a different project.
- Vault key for a member: X25519 sealed box (`crypto_box_seal` equivalent) to the public key of the member.

### 7.3 What the server keeps

| Table | Columns (plain) | Columns (encrypted) |
| --- | --- | --- |
| `profiles` | user ID, email, public key, Argon2id salt and parameters | private key (password copy), private key (recovery copy) |
| `vaults` | vault ID, kind (personal or group), owner ID, key generation | group name |
| `vault_members` | vault ID, user ID, role, status (invited or active), key generation | vault key for this member |
| `invites` | vault ID, email, invited by, date | none |
| `projects` | project ID, vault ID, version, key generation, updated by, update time | project name and all variables (one blob) |

The server can see: emails, who is in which group, the number of projects, the size of each blob, and update times. The design accepts this.

Row Level Security (RLS) rules:

- A user reads and writes only their own `profiles` row. All signed-in users can read the public key of a user who has an invite or a membership in a shared vault.
- A user reads `projects` and `vault_members` rows only for vaults where the user is an active member.
- A member writes `projects` only for their vaults.
- Only the owner writes `invites` and removes `vault_members` rows.

### 7.4 Push and pull

The local master file stays the data that `inject`, `diff`, `run` and the app use. Sync does not change offline use.

Keyman keeps sync state in a separate local file, `~/.keyman_sync` (permissions 600). The master YAML file does not change format. The sync state contains, for each link: the local project name, the cloud project ID, the vault ID, and the base (the variables and the version of the last sync). The base contains secrets, so the file has the same protection as the master file.

**Push (`keyman push [project]`, or **Sync** in the app):**

1. Keyman reads the local project.
2. Keyman encrypts it with the vault key.
3. Keyman sends it with the base version.
4. If the server version is the same as the base version, the server keeps the new blob and increments the version.
5. If the server version is newer, the server refuses. Keyman tells the user to pull first. In the app, **Sync** does a pull and then a push.

**Pull (`keyman pull [project]`, or **Sync** in the app):**

1. Keyman gets the newest blob and decrypts it.
2. Keyman merges three sets of variables: base, local and cloud.
   - A variable that changed in one set only: Keyman uses that change.
   - A variable that changed in both sets to the same value: no conflict.
   - A variable that changed in both sets to different values: conflict.
   - A variable that one side removed and the other side did not change: Keyman removes it.
   - A variable that one side removed and the other side changed: conflict.
3. Keyman writes the result into the master file with `save::update_project` (it keeps comments and other projects).
4. For each conflict:
   - App: the **Resolve differences** screen opens, with the choices "Keep this computer", "Keep cloud", or "Type a new value".
   - CLI on a terminal: Keyman asks for each conflict.
   - CLI without a terminal: Keyman stops, lists the conflicts, and makes no change. The flags `--keep-local` and `--keep-cloud` select one side for all conflicts.
5. Keyman stores the new base.

Without a project name, `push` and `pull` operate on all linked projects.

### 7.5 Groups

**Make a group:** `keyman group create <name>`. Keyman makes a vault and a vault key, and encrypts the key for the owner.

**Invite:** `keyman group invite <group> <email>`.

1. The owner adds an invite row.
2. The co-worker signs in and does the setup (7.2). The co-worker sees "Waiting for access".
3. The next time that any active member does a pull, Keyman finds the invite with a public key. Keyman encrypts the vault key for the co-worker and makes the membership active.
4. The next pull of the co-worker gets the projects of the group.

The member's computer must do step 3, because the server does not have the vault key. Keyman gives access when the email of the profile is the same as the email of the invite. Keyman does not ask for a fingerprint check (decision of the user).

Known risk: a dishonest server, or a person with control of the database, can give a false public key for an invite. That person then gets the vault key of the group. The user accepts this risk for an easier invite. A fingerprint check can be added in a later version.

**Remove:** `keyman group remove <group> <email>`.

1. Keyman on the owner's computer makes a new vault key.
2. Keyman encrypts all projects of the group again with the new key.
3. Keyman encrypts the new key for each remaining member.
4. Keyman sends all of it in one transaction (a Postgres function). The key generation increments.

The removed person keeps the data that they pulled before. The removed person cannot read changes after the removal.

**Link a project to a group:** `keyman link [project] --group <name>`. Without `--group`, the project goes to the personal vault. `keyman unlink [project]` removes only the link on this computer. The cloud copy and the local project stay. `keyman cloud delete <project>` deletes the cloud copy for all members of the vault. It asks for confirmation.

**Owner leaves:** the owner cannot leave a group. The owner can delete the group. Transfer of ownership is not in this version.

**Get a group project on a new computer:** `keyman pull` shows the projects of all vaults. A project with the same name as a different local project needs `--as <name>`.

### 7.6 Sign-in

- Supabase Auth with the PKCE flow. Providers: GitHub and Google.
- CLI: `keyman login` opens the browser. Keyman receives the result on a local address (`127.0.0.1` and a random port). `keyman logout` removes the tokens and the private key from the credential store.
- App: the same flow from **Settings › Account**.
- Keyman keeps the refresh token in the credential store, not in a file.
- The Supabase URL and the publishable (anon) key are in the program. RLS protects the data.

### 7.7 Commands and screens

| CLI | App | Function |
| --- | --- | --- |
| `keyman login` / `logout` | Settings › Account | Sign in, setup, unlock, sign out |
| `keyman link [project] [--group g]` / `unlink` | Project page › **Link to cloud** | Connect a project to a vault |
| `keyman push [project]` / `pull [project]` | **Sync** button (title bar and project page) | Manual sync |
| `keyman group create / invite / remove / list` | Settings › Groups | Group management |
| `keyman recover` | Sign-in screen › **Use recovery code** | Forgotten password |
| `keyman cloud status` | Sidebar status mark for each project | Linked, changed since last sync, conflict |

All new text in the app has English and Thai versions (`i18n`).

### 7.8 Where the code goes

- `src/cloud/` in the shared library:
  - `crypto.rs`: keys, encryption, decryption.
  - `merge.rs`: the three-way merge. It has no network code, so it is easy to test.
  - `state.rs`: the sync state file.
  - `api.rs`: the Supabase REST and Auth calls.
  - `keystore.rs`: the credential store.
- `src/main.rs` and `src/cli.rs`: the new commands.
- `gui/src/`: Settings › Account and Groups, **Sync**, **Link to cloud**, and conflicts in the current resolver.
- `supabase/migrations/`: tables, RLS rules, and the Postgres functions for removal and invite acceptance.

### 7.9 Offline and errors

- Without a sign-in, no command does a network request.
- When the network is not available, `push` and `pull` stop with a clear message. They make no change.
- When the vault key generation changed, Keyman gets the new vault key first and then continues.
- A blob that does not decrypt (wrong key or changed data) causes an error. Keyman does not overwrite local data.

### 7.10 Tests

- Unit tests: encryption round trip; wrong key fails; changed ciphertext fails; ciphertext moved to a different project ID fails; all merge cases.
- Integration tests against a local Supabase (`supabase start`): setup, two computers, push and pull, conflict, invite, removal and key rotation, RLS refuses access to a non-member.
- Test that commands without a sign-in do no network request.

## 8. Calls made by the designer (not discussed with the user)

1. The personal space is a vault of one member. This gives one code path for personal and group projects.
2. The sync state is a separate file (`~/.keyman_sync`). The master YAML format does not change.
3. Push refuses when the cloud is newer. The user pulls first.
4. CLI conflicts: questions on a terminal; without a terminal, stop or use `--keep-local` / `--keep-cloud`.
5. (Removed: the user decided on invite confirmation. See section 6, item 6.)
6. Algorithms: Argon2id, XChaCha20-Poly1305, X25519 sealed box.
7. Removal of a member rotates the key in one database transaction.
8. Cloud functions are part of the normal CLI build. They do nothing until the user signs in.
9. `unlink` removes only the local link. A separate command, `cloud delete`, deletes the cloud copy, because a group project belongs to all members.
10. The owner cannot leave a group. Ownership transfer is not in this version.
11. On a Linux computer without a credential store, Keyman asks for the vault password at each cloud command. Keyman does not write the private key to a file.

## 9. Left to the builder

- Rust crates (for example `ureq`, `keyring`, `argon2`, `chacha20poly1305`, `crypto_box`), and a JSON library for the API.
- Argon2id parameters (at least the OWASP recommendation).
- The format of the sync state file.
- The exact text of messages, in English and Thai.
- The layout of the new app screens, in the style of the current app.
