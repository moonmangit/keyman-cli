# Keyman Cloud in the Desktop App: Design

Date: 2026-11-18
Status: Draft, waiting for approval
Language: ASD-STE100 (Simplified Technical English)
Depends on: `docs/superpowers/specs/2026-11-17-cloud-sync-design.md` (the cloud design) and the cloud library in `src/cloud/`.

## 1. Intent

The cloud library and the CLI are complete. The user wants to use Keyman Cloud in the desktop app (`gui/`), without a terminal, for normal use: sign in, link projects, and sync them.

## 2. Goals

1. A user can sign in with GitHub or Google, set up the vault password, see and confirm the recovery code, unlock on a new computer, recover with the recovery code, change the vault password, and sign out. All in the app.
2. A user can link a project to the cloud (personal vault or one of their groups), sync all linked projects, and resolve conflicts. All in the app.
3. The app shows the cloud state of each project without network use.
4. The app does not freeze while it waits for the browser, the network, or the key step (Argon2id, about 1 second).
5. The app uses the same library code as the CLI. A fix in the library fixes both.

## 3. Non-goals

- Group management in the app (create, invite, remove, list, delete). It stays in the CLI.
- Automatic or background sync. Sync runs only when the user clicks **Sync**.
- Changes to the cloud design (keys, server, merge rules).
- A web version of the app.

## 4. Anti-goals

The design fails if one of these conditions occurs, also when the function operates:

- The window freezes during a cloud action.
- The app and the CLI behave differently for the same action (different merge result, different checks).
- A secret (password, recovery code, private key, variable value) goes to a log, to stdout, or stays in the clipboard history.
- The app does a network request before the user signs in or clicks a cloud action.
- A cloud action that the user cancels leaves a half-done change (for example a master file written but the link not updated, or a profile without a personal vault).

## 5. Approaches

The user selected approach A.

| Approach | Description | Advantage | Disadvantage |
| --- | --- | --- | --- |
| **A. Background thread + dialog bridge (selected)** | Each cloud action runs on a background thread and calls the library functions that the CLI uses. When the library needs an answer (password, code, conflict choice), the thread waits; the app shows a dialog and sends the answer back. | Same code as the CLI, which is reviewed and tested. Small change. | Cancel and window close must release a waiting thread. |
| B. Library split into steps | Each library flow becomes separate step functions; the app calls one step at a time. | No waiting threads. | Large change to reviewed code. Flow control in two places (CLI and app). |
| C. Run the CLI from the app | The app starts `keyman` as a separate program. | No library change. | Terminal questions do not fit; secrets go through pipes. Not a good fit. |

## 6. Decisions from the user

1. Scope: sign-in and sync. Group management stays in the CLI.
2. First use: after the welcome screen, the app shows one card that offers cloud sign-in, with **Skip**. After **Skip**, the app does not ask again. Sign-in is always available later in **Settings › Account**.
3. Sync is manual: one **Sync** button runs pull, then push, for all linked projects.
4. The screens as described in section 7.3 (the user accepted the sketches).
5. Approach A.

## 7. Design

### 7.1 Library changes (`src/cloud/`)

The app must not get text on stdout. Six places in the library print text. Change them as follows; the CLI keeps the same output.

| Place | Now | Change |
| --- | --- | --- |
| `account.rs` (code not the same, user switched, password problem) | `ui::warning(...)` | Add `fn notice(&mut self, text: &str)` to the `Prompt` trait. The library calls it. The CLI `TerminalPrompt` prints the text as a warning, as now. |
| `auth.rs` `sign_in_with_browser` | `println!` of the sign-in link | Give the link to the caller: `account::login` and `account::recover` call `prompt.notice(...)` (or a new `Prompt::sign_in_link(&mut self, url)`) with the link. The CLI prints it as now. |
| `sync.rs` `pull` (accepted invites, invite warnings) | `println!` / `eprintln!` | `pull` returns these lines in its result (for example a `PullReport { results, given, warnings }`). The CLI prints them as now. |

All other `ui::warning` calls in `account.rs` (wrong password in `ask_password`, the code retry in `setup_profile`, user switched in `store_session`) also go through `Prompt::notice`.

**Same-name project.** `pull` now reports a same-name collision as a structured result, for example `PullResult::NameTaken { project }`, not as a `Skipped` text. The CLI prints the same text as now. The app uses it to show the **Merge** button, so it never matches English text.

**Group names.** Add a small public helper, for example `groups::group_names(vaults: &Vaults) -> Vec<(String /*vault id*/, String /*name*/)>`, for the link picker. It returns the same names that `link --group` accepts.

**Cancel.** The app must be able to stop a sign-in while it waits for the browser (up to 300 seconds now). Add a cancel signal to `auth::sign_in_with_browser` (for example `&AtomicBool`, checked in the listener loop). When it is set, the function returns the STE error "Sign-in was cancelled." The CLI passes a signal that is never set.

**Prompt and Resolver answers.** A `Prompt` or `Resolver` method that returns `Err` (or `None` for the resolver) stops the flow with no further change. This is already the rule in the library; the app uses it for **Cancel**.

No other library change. The library and the CLI tests must continue to pass, and the CLI output must stay the same.

### 7.2 Background work and the dialog bridge (`gui/`)

1. **One cloud task at a time.** `App` has a cloud task state: idle or busy (with a short label, for example "Signing in…" or "Syncing…"). These are cloud tasks: sign in, recover, change password, sign out, loading the link picker, link, Sync, **Merge**, and **Delete from cloud**. While a task runs, all cloud buttons and **Unlink** are disabled (Unlink is local, but it writes the same state file as Sync).
   - Each cloud task gets an `Unlocked` through `account::session`, so any task can show the **Vault password** dialog when the key is not stored on this computer.
2. **Thread.** A cloud action starts a `std::thread`. The thread calls the library (`account::login`, `account::session`, `sync::link`, `sync::pull`, `sync::push` and the others) with a bridge `Prompt` and a bridge `Resolver`.
3. **Questions.** When the library calls a bridge method, the bridge sends a request to the UI through `AppHandle::update` (for example `NewPassword`, `Password { tries_left }`, `RecoveryCode`, `ShowRecoveryCode(code)`, `Resolve { project, conflicts }`, `Notice(text)`). Then it waits on a one-time channel for the answer.
4. **Answers.** The dialog sends the answer on the channel. **Cancel**, closing the dialog, or closing the window sends "cancelled" (or drops the sender). The bridge returns `Err` to the library, so the flow stops.
   - Cancel takes effect at the next question, or in the browser wait (the cancel signal). A step that is running (the key step, a server call) finishes first. While it finishes, the label shows "Cancelling…". The library already stops before it writes anything when a question returns `Err`, so a cancel leaves no half-done change.
   - The bridge counts the **Vault password** questions itself to show the remaining tries.
   - The recovery code dialog answers two library calls: `show_recovery_code` (show the code) and the next `recovery_code` (the typed confirmation).
5. **Result.** At the end, the thread sends the result to the UI through `AppHandle::update`. The UI shows a toast or the **Sync results** dialog, reloads the master file, and sets the task state to idle.
6. **Secrets in memory.** Passwords and codes typed in dialogs go to the thread and are then cleared from the dialog fields. Do not keep them in `App` after the answer is sent.
7. **No network before an action.** At app start, the app finds out the cloud state from local data only: the sync state file (`~/.keyman_sync`) and whether a session entry is in the credential store (or a refresh token in the state file, on a computer without a credential store). It does not refresh the session or call the server until the user clicks a cloud action.
8. **Sign-in no longer valid.** When a cloud task fails because the server rejects the saved sign-in (the library message "Your sign-in is not valid now. Run 'keyman login'."), the app changes to signed out, shows a toast "Your sign-in is not valid now. Sign in again.", and opens **Settings › Account**.
9. **No credential store.** On a computer without a credential store, the library asks for the vault password at each cloud task. The app does the same, and the Account page shows one line: "This computer has no credential store. Keyman asks for the vault password at each cloud action."

### 7.3 Screens

All new text is in English and Thai (`i18n::t`), in the existing app style. Error details that come from the library stay in English, as the app already does for library errors.

**Title bar**
- A cloud icon. It shows: signed out, signed in, or busy. A click opens **Settings › Account**.
- A **Sync** button, only when the user is signed in. While a sync runs, the icon turns and the button is disabled.

**First-use card**
- When a master file is loaded and the user is not signed in and did not skip before, the main area shows one card: "Keep an encrypted copy in Keyman Cloud?" with **Sign in with GitHub**, **Sign in with Google**, and **Skip**.
- **Skip** saves a setting (in the app preferences), and the card does not show again.

**Settings › Account (new page)**
- Signed out: a short text about Keyman Cloud, **Sign in with GitHub**, **Sign in with Google**, and **Use recovery code** (for a forgotten password on a new computer).
- Signed in: "Signed in as {email}", **Change vault password**, **Sign out** (with confirmation).
- Busy: "Waiting for the browser…" with **Cancel** during sign-in.

**Dialogs (from the bridge)**
- **New vault password:** two hidden fields, 10 characters or more, the two entries must be the same. Text: "Keyman cannot reset this password."
- **Recovery code:** the code in large monospace text, **Copy** (kept out of clipboard history and cleared after the clipboard time, like **Copy export**), and a field "Type the code again". The dialog cannot close until the code matches, or the user cancels (cancel stops the setup with no change on the server).
- **Vault password:** one hidden field. After a wrong entry: the library message and the remaining tries. After the last try: the hint to use the recovery code, with a **Use recovery code** button.
- **Recovery code entry:** one field for the code, then the **New vault password** dialog.
- **Change vault password:** current password, then the **New vault password** dialog.
- **Notice:** a library notice shows as a toast.

**Project page**
- A cloud status line in the header: "Not in the cloud", "Synced", or "Changes on this computer".
- **Link to cloud** when the project is not linked. It opens a small picker: **Personal**, or one of the user's groups by name. The group list needs the network, so the picker loads it after the click and shows "Loading groups…".
- The **⋯** menu gets **Unlink** (this computer only) and **Delete from cloud** (all members; the user types the project name to confirm).

**Sidebar**
- A mark after each linked project: ☁ ✓ (synced) or ☁ ● (changes on this computer). No mark when the project is not linked. The app computes the marks with `sync::local_status`, without network.

**Sync**
- **Sync** runs `pull` (all projects, the same as `keyman pull`), then `push`. Like the CLI, `pull` also adds cloud projects that are not on this computer yet (for example from a second computer or a group) to the master file and links them. The user accepted this ("new cloud projects appear in your master file without extra steps").
- Conflicts open the existing **Resolve differences** dialog, adapted for cloud conflicts: the sides are "This computer" and "Cloud", plus "Type a new value". Values are hidden until the user reveals them, as in the existing resolver. **Cancel** = the resolver returns `None`, and Keyman changes nothing for that project.
- At the end:
  - If all projects are fine: a toast with the counts, for example "2 projects updated, 1 added, 1 sent." or "All projects are up to date."
  - If a project needs the user (Skipped, Stopped, cloud is newer, or a same-name project that can merge): the **Sync results** dialog lists each such project with its reason. It also lists the names of added projects.
  - A same-name project (`PullResult::NameTaken`) has a **Merge** button, which runs `pull` for that project name (the same as `keyman pull app`). The CLI option `--as <name>` (pull with another name) stays in the CLI.
- Invite results from `pull` ("Gave {email} access to {group}.") show in the toast or the results dialog.

### 7.4 Where the code goes

- `src/cloud/`: the library changes in section 7.1.
- `src/main.rs`: the CLI prints the returned lines and implements `Prompt::notice`.
- `gui/src/cloud.rs` (new): cloud task state, the thread start, the bridge `Prompt` and `Resolver`, the request and answer types.
- `gui/src/cloud_views.rs` (new): the Account page, the first-use card, the cloud dialogs, the link picker, the Sync results dialog.
- `gui/src/main.rs`, `views.rs`, `menu.rs`, `resolve.rs`, `settings.rs`, `i18n.rs`: the title bar items, the project page items, the ⋯ menu items, the cloud conflict mode of the resolver, the skip setting, the Thai text.
- `gui/Cargo.lock`: refresh it, because the app now builds the cloud dependencies through `keyman-cli`.

### 7.5 Tests

- Library: the existing tests continue to pass; add tests for the new return values and the cancel signal.
- App: unit tests for the bridge (an answer reaches the thread; **Cancel** and a dropped sender make the library call return `Err`; only one task runs at a time).
- Headless snapshots: new scenes for the Account page (signed out, signed in, busy), the first-use card, the new password dialog, the recovery code dialog, the vault password dialog, the link picker, the Sync results dialog, the cloud resolver, and Thai versions of the main ones.
- Manual test against the hosted project `keyman`: sign in with GitHub, set up, link a test project, sync, sync from a second home folder.

## 8. Calls made by the designer (not discussed with the user)

1. The cloud icon and **Sync** go in the title bar; **Sync** shows only when signed in.
2. **Use recovery code** is on the signed-out Account page, so recovery works on a new computer from the app.
3. Values in the cloud resolver are hidden until revealed, like the existing resolver.
4. The app finds the signed-in state offline at start (state file + credential store entry). It does not refresh the session at start.
5. Library error text stays in English in the app; app text is English and Thai.
6. The skip choice of the first-use card is an app preference, not part of `~/.keyman_sync`.
7. Same-name collisions become a structured `PullResult` variant (a small library change), so the app never matches English text.
8. A small public `groups::group_names` helper for the link picker.
9. Loading the link picker, **Merge**, **Delete from cloud** and **Sign out** are cloud tasks; **Unlink** is disabled while a task runs.
10. Cancel waits for a running step (key step or server call) and shows "Cancelling…".
11. A rejected sign-in changes the app to signed out and opens the Account page.
12. On a computer without a credential store, the app asks for the vault password at each cloud task, and says so on the Account page.
13. **Use another name** (`--as`) stays in the CLI; the app offers only **Merge** for a same-name project.

## 9. Left to the builder

- The exact Rune layout and widget choices, in the existing app style.
- The exact type names of the bridge requests and answers.
- How the turning icon is drawn.
- The Thai translations (natural Thai, like the existing app text).
