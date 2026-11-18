# Changelog

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
