# Changelog

## 0.3.2

### New

- **Windows installer.** Download `Keyman-setup-windows-x64.exe` and double-click it. It installs the desktop app for your user only (no administrator permission), adds a Start menu entry, an optional desktop shortcut and an uninstaller, and the app can update itself in that folder.

### Changed

- Releases are built and published on GitHub again, like Sigit: push a `vX.Y.Z` tag. One Windows job builds the CLI, the app zip, the installer and the npm package. `scripts/release-windows.ps1` is removed.

## 0.3.1

### New

- **Updates in the desktop app.** Settings › About › Updates checks GitHub for a newer release and installs it with one click (**Update now**, then **Restart now**). Each download is checked against the release's `SHA256SUMS.txt`. The app also checks once a day at start; switch this off with **Check for updates automatically**.
- Releases include the desktop app for Windows: `Keyman-windows-x64.zip`.

### Changed

- Releases have Windows x64 builds only, for the CLI, the npm package and the app. macOS and Linux are not built for now (macOS runners wait in a long queue); build them from source.
- Releases are made on a Windows PC with `scripts/release-windows.ps1`. The GitHub workflow on `main` only checks the Windows release build and the npm package, and publishes nothing. It does not run CI again after a merge (the pull request already passed it).
- Faster builds: release builds no longer use cross-crate LTO (the binaries are a little larger), the build cache has a fixed key, and pull requests no longer build the app bundles.

## 0.3.0

### New

- **Stages.** A project can have stages such as `production`, `staging` and `pre-production`. Variables directly under the project are shared by every stage, and a stage lists only what it adds or changes. Name a stage as `project:stage` in `show`, `inject`, `save`, `diff`, `export` and `run`. In a folder named after the project, `:staging` is short for `project:staging`. `keyman list` and shell completion include every `project:stage`.
- **Keyman Cloud (optional).** End-to-end encrypted sync of projects between computers: `login`, `logout`, `recover`, `link`, `unlink`, `push`, `pull` and `cloud status` / `cloud delete`, with a three-way merge that asks which value to keep when both sides changed the same variable (`--keep-local` / `--keep-cloud` to choose without questions). Stages sync with their project.
- **Share groups.** `keyman group create`, `invite`, `remove` and `delete`, plus `link --group`, share projects with other people.
- **Desktop app** for macOS and Windows: projects, a stage column, tied folders drawn as a tree, conflict resolution, cloud sign-in and sync, English and Thai. The project list, stage column and Variables/Folders split can be resized by dragging; double-click a divider to reset it.

### Fixed

- Bash completion works without `extglob`, and completes `project:stage` names (bash splits words at `:`).

### Notes

- A project with stages needs a stage: `keyman inject my-app` stops and lists the stages instead of guessing.
- In the app, a folder with no stage picked is not written until you pick one.
- Projects without stages, and their cloud copies, work exactly as before.
