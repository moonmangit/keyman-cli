# Keyman desktop app

A native window for the `keyman` commands, built with [Rune](https://github.com/anomalyco/rune). It reads and writes the same master file as the CLI (`~/.keyman_config` → `~/.keyman.yml`) and calls the same library code, so the CLI and the app can be used side by side.

The window draws its own title bar (breadcrumb, Reload, Full screen, Help, Settings) while keeping the native traffic lights on macOS and the caption buttons with snap layouts on Windows. A project's page shows its **Variables** and **Folders** side by side, and both columns grow with the window. In a narrow window they stack, and the page's buttons show only their icons. Drag any vertical divider (the project list, the stage column, or the gap between Variables and Folders) to resize; double-click it to reset. The widths are saved between launches.

| In the app | CLI equivalent |
| --- | --- |
| Welcome screen: **Create ~/.keyman.yml** / **Load existing file…** | `keyman init` / `keyman load <file>` |
| Sidebar: searchable project list with status and variable counts | `keyman status`, `keyman list` |
| Sidebar footer: **Edit** (system text editor), **Load** · title bar **Reload** | `keyman edit`, `keyman load` |
| **Variables**: per-value show/copy and **Reveal values** | `keyman show [--reveal]` |
| **Folders**: right-click a folder › **Write .env**, or **Write to all** tied folders | `keyman inject <project> <folder>` |
| Right-click a folder › **Compare with master** (opens in a modal) | `keyman diff` |
| Right-click a folder › **Save .env to master** (preview modal, optional remove, confirm) | `keyman save --dry-run`, `keyman save [--replace]` |
| Sidebar **+** / **New project from folder** | `keyman save <folder-name> --new` |
| **Copy export** (kept out of clipboard history, then cleared) | `keyman export` |
| Settings › Account: **Sign in with GitHub** / **Google**, **Sign out** | `keyman login`, `keyman logout` |
| Account: **Use recovery code** (also when signed in) | `keyman recover` |
| Account: **Change vault password** | `keyman password` |
| Project page: **Link to cloud** (Personal or a group) / ⋯ › **Unlink** | `keyman link`, `keyman unlink` |
| Title bar: **Sync** (when signed in) | `keyman pull` then `keyman push` |
| Project page: ⋯ › **Delete from cloud** | `keyman cloud delete` |

**Tied folders**: a project can be tied to any number of local folders (a checkout, a worktree, a deploy copy). The Folders column draws them as a tree, master file → project → each folder, with every branch coloured by that folder's `.env`: in sync, differs, no `.env` yet, or folder missing. Click a folder to make it active. **Right-click** a folder (or press its ⋯ button) for its action panel: make active, write `.env`, compare, resolve differences, save to master, show in Finder/Explorer, copy path, or untie. Compare and Save to master open as modals. Folders tied by earlier versions (one per project) carry over.

**Stages**: a project with stages (see [Stages](../README.md#stages-production-staging-and-more)) opens a second column next to the project list, with one row per stage: how many values it sets itself, and how many tied folders write from it. Pick a stage there to see its variables. A badge marks each value the stage sets itself; the others are shared by every stage. Each tied folder writes from its own stage, shown before its path in the tree. Change it in the folder's action panel under **Write from stage**. A folder tied before its project had stages, or whose stage was removed, shows **Pick a stage**: nothing is written to it until you pick one. Clicking a folder shows its stage. Writing, comparing, resolving and saving all use the folder's stage, and saving keeps what differs from the shared values in that stage only.

**Sync on tie**: tying a folder syncs it straight away. A folder with no `.env` gets one written; one that already matches is left alone. When its `.env` disagrees with the master file, **Resolve differences** opens: for each variable, keep the master value (the default), keep the `.env` value, or type a new one. Applying writes the result into both the master file and the folder's `.env`, so they end in sync. The same resolver is available later from a folder's action panel and from the overwrite warning.

**Keyman Cloud** (optional): the app can sync projects with Keyman Cloud, like the CLI. If you do not sign in, the app does not use the network. The server keeps only encrypted data. For the full design, see [Cloud sync (optional)](../README.md#cloud-sync-optional) in the main README.

- **First use**: a card asks if you want to use the cloud. Choose **Skip** to continue without it. You can sign in later.
- **Account**: open Settings › Account. Sign in with GitHub or Google. The browser opens. The first time, make a vault password. The app then shows a recovery code one time. Write it down and keep it safe. Nobody can reset the vault password.
- **Recovery and password**: if you forget the vault password, use **Use recovery code**. You can also use it when you are signed in. **Change vault password** sets a new one. **Sign out** ends the session on this computer.
- **Link**: on a project's page, **Link to cloud** asks where to keep it: Personal or one of your groups. A status line shows the state of the link. In the sidebar, a linked project has a cloud icon with a check mark (in sync) or a dot (changes on this computer). The ⋯ menu on the project page has **Unlink** and **Delete from cloud**. Delete asks you to type the project name.
- **Sync**: the title bar shows a cloud icon and the **Sync** button. **Sync** gets the changes from the cloud first. Then it sends your changes. A message tells you the result, for example "2 projects updated, 1 added, 1 sent." If a project needs your attention, the **Sync results** dialog lists it. If this computer and the cloud have different projects with the same name, click **Merge** to join them. If you cancel a sync after Keyman got the cloud changes, the message tells you that Keyman did not send your changes.
- **Conflicts**: if the same variable changed in both places, **Resolve differences** opens. For each variable, choose **This computer**, **Cloud**, or **Type a new value**.
- **Rejected sign-in**: if the server does not accept your saved sign-in, the app signs you out and opens the Account page. Sign in again. If the sign-in expired during one action, the app stays signed in. Click the action again.

If you close Keyman while a cloud action runs, the action stops. Run it again; Keyman continues safely.

Use the CLI to create, invite, remove, list and delete groups (`keyman group ...`) and to get a cloud project with a different name (`keyman pull --as <name>`).

**Languages**: the whole interface is in English and Thai (ไทย). It follows the system's preferred language, or pick one in Settings › Appearance › Language. Thai text uses the bundled Noto Sans Thai (`assets/fonts`, alongside Inter, both under the SIL Open Font License) with Thai line breaking. Error details passed through from the master-file library stay in English.

**Settings** (⌘, / Ctrl+,): window size presets, remembered folders, overwrite confirmation and Git warning (General); language, System/Light/Dark theme, accent colour and interface size (Appearance); clipboard clear time and re-hiding values (Privacy); version, paths, links and updates (About). Settings and tied folders are saved between launches.

**Help** (F1): getting started, keyboard shortcuts and the CLI-to-app mapping.

## Build and run

Rune pins its toolchain, so this crate has its own `rust-toolchain.toml` (1.99.1) and its own Cargo workspace. The CLI at the repository root is unaffected.

```bash
cd gui && cargo run
```

Rune is a git dependency pinned to one `dev` revision. Its `[patch.crates-io]` entries in `Cargo.toml` must stay on the same revision; update them together.

## Try stages without touching your data

```bash
gui/scripts/try-stages.sh
```

It builds the CLI and the app, makes a demo master file with a staged project and three tied folders (one in sync, one that differs, one without a `.env`), prints CLI commands to try, and opens the app on it. The demo has its own home folder and app settings, so `~/.keyman.yml` and your settings stay as they are, and Keyman Cloud points at an address that does not answer. Add `--reset` to start again, or `--cli` to skip opening the app.

## Install on Windows

Download `Keyman-setup-windows-x64.exe` from the [newest release](https://github.com/moonmangit/keyman-cli/releases/latest) and double-click it. It installs for your user only (no administrator permission) into `%LOCALAPPDATA%\Programs\Keyman`, with a Start menu entry, an optional desktop shortcut and an uninstaller (Settings › Apps). The uninstaller keeps your settings and your master file. The installer is built from `../installer/windows/keyman.iss` with Inno Setup.

## Updates

**Settings › About › Updates** asks GitHub for the newest release. When it is newer, **Update now** downloads the zip for this system (`Keyman-windows-x64.zip`; releases have a Windows build only for now, so on macOS the page links to the release page), checks it against the release's `SHA256SUMS.txt`, and puts it in place of the running copy. Then **Restart now** opens the new version. Your settings stay. If Keyman is installed for all users, Windows asks for administrator permission and macOS asks for your password. A build that runs from `target/` does not replace itself.

**Check for updates automatically** (on by default) checks once a day when Keyman starts and shows a message when a newer version is available. Keyman installs an update only when you click **Update now**. The code is in `src/update.rs`.

## App icon and packaging

`assets/icon.svg` is the source artwork. `assets/make-icons.sh` regenerates every format from it (needs macOS and ImageMagick):

- `keyman.icns`: the macOS bundle icon.
- `keyman.ico`: embedded into the Windows `.exe` by `build.rs`, with 16–256 px sizes, so Explorer, the taskbar, shortcuts and Alt+Tab show it.
- `icon-macos.png`: set as the Dock icon at launch, so `cargo run` shows it too.

**macOS**: build `Keyman.app` (ad hoc signed; set `KEYMAN_SIGN_IDENTITY` to sign with a Developer ID):

```bash
cd gui && scripts/bundle-macos.sh
```

```bash
cd gui && scripts/bundle-macos.sh --universal
```

The app lands in `target/release/bundle/Keyman.app`; `--universal` builds one app for Apple silicon and Intel, and `--target <triple>` builds one target. Releases do not build macOS for now.

**Windows**: build with the MSVC toolchain. The executable carries its icon and version details, and release builds open no console window.

```bash
cd gui && cargo build --release
```

## Headless snapshots

Debug builds can render a frame without opening a window, which is handy for checking layout:

```bash
cd gui && cargo run -- --snap shot.png --scene diff
```

Scenes: `revealed`, `diff`, `save`, `menu`, `resolve`, `resolve-edit`, `settings`, `appearance`, `privacy`, `about`, `help`, `shortcuts`, `commands`, `confirm`, `confirm-all`, `toast`, `light`, the cloud scenes `cloud-signed-out`, `cloud-signed-in`, `cloud-busy`, `cloud-first-use`, `cloud-new-password`, `cloud-recovery-code`, `cloud-password`, `cloud-project-linked`, `cloud-project-unlinked`, `cloud-link-picker`, `cloud-link-loading`, `cloud-delete-confirm`, `cloud-sync-busy`, `cloud-sync-results`, `cloud-resolve`, and Thai versions `th`, `th-menu`, `th-diff`, `th-resolve`, `th-settings`, `th-help`, `th-cloud-sync-results`, `th-cloud-resolve`, `th-cloud-link-picker`, `th-cloud-signed-out`, `th-cloud-recovery-code`. Add `--size 1680x900` to try another window size, or `--chrome` to draw the macOS traffic lights and report anything overlapping them.
