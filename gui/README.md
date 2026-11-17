# Keyman desktop app

A native window for the `keyman` commands, built with [Rune](https://github.com/anomalyco/rune). It reads and writes the same master file as the CLI (`~/.keyman_config` → `~/.keyman.yml`) and calls the same library code, so the CLI and the app can be used side by side.

The window draws its own title bar (breadcrumb, Reload, Full screen, Help, Settings) while keeping the native traffic lights on macOS and the caption buttons with snap layouts on Windows. A project's page shows its **Variables** and **Folders** side by side, and both columns grow with the window.

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

**Tied folders**: a project can be tied to any number of local folders (a checkout, a worktree, a deploy copy). The Folders column draws them as a tree, master file → project → each folder, with every branch coloured by that folder's `.env`: in sync, differs, no `.env` yet, or folder missing. Click a folder to make it active. **Right-click** a folder (or press its ⋯ button) for its action panel: make active, write `.env`, compare, resolve differences, save to master, show in Finder/Explorer, copy path, or untie. Compare and Save to master open as modals. Folders tied by earlier versions (one per project) carry over.

**Sync on tie**: tying a folder syncs it straight away. A folder with no `.env` gets one written; one that already matches is left alone. When its `.env` disagrees with the master file, **Resolve differences** opens: for each variable, keep the master value (the default), keep the `.env` value, or type a new one. Applying writes the result into both the master file and the folder's `.env`, so they end in sync. The same resolver is available later from a folder's action panel and from the overwrite warning.

**Languages**: the whole interface is in English and Thai (ไทย). It follows the system's preferred language, or pick one in Settings › Appearance › Language. Thai text uses the system's Thai font with Thai line breaking. Error details passed through from the master-file library stay in English.

**Settings** (⌘, / Ctrl+,): window size presets, remembered folders, overwrite confirmation and Git warning (General); language, System/Light/Dark theme, accent colour and interface size (Appearance); clipboard clear time and re-hiding values (Privacy); version, paths and links (About). Settings and tied folders are saved between launches.

**Help** (F1): getting started, keyboard shortcuts and the CLI-to-app mapping.

## Build and run

Rune pins its toolchain, so this crate has its own `rust-toolchain.toml` (1.99.1) and its own Cargo workspace. The CLI at the repository root is unaffected.

```bash
cd gui && cargo run
```

Rune is a git dependency pinned to one `dev` revision. Its `[patch.crates-io]` entries in `Cargo.toml` must stay on the same revision; update them together.

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

The app lands in `target/release/bundle/Keyman.app`; `--universal` builds one app for Apple silicon and Intel.

**Windows**: build with the MSVC toolchain. The executable carries its icon and version details, and release builds open no console window.

```bash
cd gui && cargo build --release
```

## Headless snapshots

Debug builds can render a frame without opening a window, which is handy for checking layout:

```bash
cd gui && cargo run -- --snap shot.png --scene diff
```

Scenes: `revealed`, `diff`, `save`, `menu`, `resolve`, `resolve-edit`, `settings`, `appearance`, `privacy`, `about`, `help`, `shortcuts`, `commands`, `confirm`, `confirm-all`, `toast`, `light`, and Thai versions `th`, `th-menu`, `th-diff`, `th-resolve`, `th-settings`, `th-help`. Add `--size 1680x900` to try another window size, or `--chrome` to draw the macOS traffic lights and report anything overlapping them.
