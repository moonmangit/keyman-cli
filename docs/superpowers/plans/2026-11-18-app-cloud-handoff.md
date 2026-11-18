# App cloud: handoff and remaining work

Date: 2026-11-18. Status: built and reviewed, **not merged, not released**.

## Where the work is

| Branch | What it holds |
|---|---|
| `desktop-app` | The CLI cloud work (Supabase back end, client-side encryption, groups, the personal vault key) and the bundled Inter + Noto Sans Thai fonts (commit `9f2643a`). |
| `app-cloud` | Branches from `desktop-app` at `c33c04e`. Adds sign-in and sync to the desktop app (`gui/src/cloud.rs`, `gui/src/cloud_views.rs`) and small library hooks in `src/cloud/*`. |

Design: `docs/superpowers/specs/2026-11-18-app-cloud-design.md`.
Plan: `docs/superpowers/plans/2026-11-18-app-cloud.md`.

Checks at `app-cloud` HEAD (before this note): root `cargo fmt`, `cargo clippy` clean;
`cargo test --locked` 151 + 19 + 1 passed (13 cloud end-to-end tests ignored, they need Docker);
in `gui/`: fmt and clippy clean, `cargo test` 65 passed.

## Hosted back end

- Supabase project "keyman", ref `ukcjavxqyfjklagzkuzg`, region ap-southeast-1.
- The URL and the publishable key are in `src/cloud/config.rs`. No secret keys are in the repo.
- Migrations applied: `20261117000000_cloud_sync`, `20261117000100_vault_name`,
  `20261118000000_private_helpers`, `20261118000100_review_fixes`.
- Auth: GitHub provider on, Email provider **off**, redirect URLs for `http://127.0.0.1:47821-47825`.
- State on 2026-11-18: 1 user, 1 profile (encrypted keys only), 1 personal vault,
  1 owner membership, 0 invites, **0 projects** (nothing linked yet).

## Manual test results so far

- GitHub sign-in from the app: **pass**.
- Vault password and recovery code: **pass** (profile and personal vault rows are correct).
- Link and Sync: **not done**. The test projects showed "Not in the cloud", so Sync sent nothing.

## Remaining work, in order

1. **Fix the Sync message when no project is linked.**
   Now: Sync with no linked project (and nothing in the cloud) shows
   "All projects are up to date." This is wrong: nothing was sent.
   Fix: in `gui/src/cloud.rs`, `summarize` sets a new `SyncSummary` flag when
   `report.results` and `pushed` are both empty. `toast_words` then shows
   "No project is linked to the cloud. Select a project and click Link to cloud."
   (Thai: "ยังไม่มีโปรเจกต์ที่เชื่อมกับคลาวด์ เลือกโปรเจกต์แล้วคลิก เชื่อมกับคลาวด์").
   Add a case to `toast_text_rules`.
2. **Make Link to cloud easy to find.** In a narrow window the button row above
   the Variables table can go off the right edge. Check at the minimum size
   (820 × 560) and let the row wrap, or move **Link to cloud** next to the
   "Not in the cloud" label.
3. **Finish the manual test** (`cd gui && cargo run`):
   link a test project to **Personal**, Sync, check that the `projects` row has only
   `sealed` text; change a value, Reload, Sync again (version goes up);
   optional second home folder (unlock with password, project shows as Added);
   cancel one Sync mid-way.
4. **Merge `app-cloud` into `desktop-app`.** `gui/Cargo.lock` will conflict
   (both sides added the cloud dependencies; `app-cloud` also adds `zeroize`):
   take the `app-cloud` side, then run `cargo build` in `gui/`.
   `gui/src/main.rs` may conflict near `AppSpec`: keep both the font lines and the cloud lines.
   Run all checks again after the merge.
5. **Do not release** until the user says so.

## Deferred (not blocking the merge)

- The keyring read at app start runs on the UI thread. Move it to the cloud thread
  before a macOS or Linux release (a keyring prompt there would freeze the window).
- Some rare library messages in `src/cloud/*` still name CLI commands
  (for example "Run 'keyman unlink web'"). Give the app its own words.
- The master-file problem page has no ⋯ project menu.
- The 13 CLI cloud end-to-end tests have never run. They need Docker and a local Supabase.
