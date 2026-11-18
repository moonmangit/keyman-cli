# Keyman CLI

Keep every project's secrets in one private YAML file, and get the right `.env` in any project folder with one command.

```sh
cd ~/code/my-app
keyman inject        # writes .env for "my-app"
```

No more copying `.env` files between machines, digging through chat history for an API key, or committing secrets by accident.

## Install

```sh
npm install -g keyman-cli
```

Works on Windows, macOS (Intel and Apple Silicon), and Linux (x64 and ARM64). Rust is not needed. The command is `keyman` (or `keyman-cli`).

## Quick start

```sh
keyman init          # 1. create ~/.keyman.yml with an example project
keyman edit          # 2. add your projects and secrets
cd ~/code/my-app
keyman inject        # 3. write .env for this project
```

The master file is plain YAML. Each top-level key is a project, and each project lists its variables:

```yaml
my-app:
  DATABASE_URL: "postgres://user:pass@localhost:5432/my_app"
  API_KEY: "secret_key_123"

other-app:
  DEBUG: "true"
```

**Name projects after their folders.** When you leave out the project name, Keyman uses the name of the folder you are in. That is why `keyman inject` in `~/code/my-app` uses the `my-app` project.

Already have a master file? Run `keyman load /path/to/master.yml` instead of `keyman init`.

## Everyday use

### Start work on a project

```sh
cd ~/code/my-app
keyman inject
```
```
✓ Created ~/code/my-app/.env · 2 variables from my-app
```

Run it again whenever the master file changes. Keyman tells you whether `.env` was created, updated, or already up to date, and warns you if Git does not ignore `.env` in that repository.

### You changed `.env` — save it back

While you work, you add a variable or rotate a key in the project's `.env`. Compare it with the master file, like `git diff`:

```sh
keyman diff
```
```
diff my-app → ~/code/my-app/.env

  DATABASE_URL=••••••••
- API_KEY=••••••••
+ API_KEY=••••••••  (value changed)
+ NEW_FLAG=••••••••

1 changed · 1 only in .env
Run keyman save to update the master file, or keyman inject to overwrite .env.
```

Red `-` lines are the master file, green `+` lines are `.env`. Values stay hidden; add `--reveal` to see them. Then copy the changes into the master file:

```sh
keyman save --dry-run   # preview only
keyman save
```
```
Saving ~/code/my-app/.env → my-app

  ~ API_KEY       changed
  + NEW_FLAG      added
  1 variable unchanged

✓ Saved 2 changes to ~/.keyman.yml
```

`save` changes only that project's lines, so your comments and other projects stay as they are. Variables that are only in the master file are kept; add `--replace` to remove them too.

### Add a new project

Already have a `.env` in a new project folder? Save it as a new project, named after the folder:

```sh
cd ~/code/new-service
keyman save
```

Or add it by hand with `keyman edit`.

### Use variables without a `.env` file

Run a command with the project's variables, without writing anything to disk:

```sh
keyman run -- npm start
keyman run my-app -- python manage.py runserver
```

Or load them into your current shell (bash, zsh):

```sh
eval "$(keyman export)"
```

On Windows, run `.cmd` and `.bat` scripts such as `npm` through the shell: `keyman run -- cmd /c npm start`.

### Stages: production, staging, and more

A project can have stages. Variables directly under the project are shared by every stage. A stage lists only the variables it adds or changes:

```yaml
my-app:
  DEBUG: "false"          # shared by every stage
  LOG_LEVEL: "info"
  production:
    DATABASE_URL: "postgres://prod.internal/app"
  staging:
    DATABASE_URL: "postgres://staging.internal/app"
    DEBUG: "true"           # replaces the shared value
  pre-production: {}        # only the shared variables
```

Name a stage as `project:stage` in any command that takes a project:

```sh
keyman inject my-app:staging
keyman run my-app:production -- npm start
keyman show my-app            # shared variables and each stage
keyman show my-app:staging    # what staging writes
keyman diff :staging          # in a folder named my-app
```

A project with stages needs a stage: `keyman inject my-app` stops and lists them. `keyman save my-app:staging` writes into that stage only what differs from the shared values. Add `--new` to create a stage. Inside a folder named after the project, `:staging` is short for `my-app:staging`. Stages are optional; a project without them works as before.

### Check what you have

```sh
keyman status        # every project, its variable count, and any problems
keyman show          # this project's variable names (add --reveal for values)
keyman list          # project names only, one per line
```
```
~/.keyman.yml · 2 projects

  ✓ my-app     3 variables  ← current folder
  ✓ other-app  1 variable
```

## Cloud sync (optional)

Cloud sync lets you use your projects on more than one computer. It also lets you share projects with a group. If you do not sign in, Keyman works as before and does not use the network.

### What the cloud keeps

Keyman encrypts your projects on your computer. The server keeps only encrypted data. Encrypted data includes project names, variable names, and values.

The server can see these items:

- Email addresses.
- Who is in which group.
- The number and size of your projects.
- The times of changes.

Sync is manual. Keyman syncs only when you run `keyman push` or `keyman pull`. Keyman does not sync `.env` files. It syncs only the projects in the master file.

### Sign in

```sh
keyman login
```

Keyman opens your browser. Sign in with GitHub or Google. You can add `--github` or `--google` to skip the question.

The first time, Keyman asks you to make a vault password. Keyman uses it to encrypt your secrets. Keyman cannot reset it. Then Keyman shows a recovery code one time. Write the code down and keep it in a safe place.

**Warning:** If you lose the vault password and the recovery code, nobody can get your cloud data back.

On a new computer, run `keyman login` and type your vault password. If you forget the password, run `keyman recover` and type the recovery code. To change the password, run `keyman password`. Run `keyman logout` to remove the keys from this computer. Your projects and links stay.

### Send a project to the cloud

```sh
keyman link my-app
keyman push
```

`keyman link` encrypts the project, with all its stages, and sends it. Keyman remembers the link on this computer. `keyman push` sends later changes. It sends every linked project, or only the project you name. If the cloud has newer changes, Keyman stops. Run `keyman pull` first.

### Get a project from the cloud

On another computer, sign in, then run:

```sh
keyman pull
```

Keyman adds new cloud projects to the master file. It also merges the changes of projects that are linked. If the master file has a project with the same name, run `keyman pull my-app` to merge the two. Use `--as <name>` to give a new project a different local name.

### Conflicts

Keyman merges changes variable by variable, and stage by stage. A conflict occurs when you and another person change the same variable. Keyman then asks which value to keep. The values stay hidden unless you ask to see them. A stage's variable shows as `staging:DATABASE_URL`.

To choose without questions, add `--keep-local` or `--keep-cloud`. If Keyman cannot ask you and you add no option, Keyman stops and changes nothing.

### Share with a group

```sh
keyman group create team
keyman group invite team sam@example.com
keyman link my-app --group team
```

You are the owner of a group you make. Only the owner can invite and remove people. All members can read and change the projects of the group.

The invited person signs in with the same email address and runs `keyman pull`. The person then waits. Keyman gives the person access when a member of the group runs `keyman pull`. Then the invited person runs `keyman pull` again.

Other group commands:

```sh
keyman group list                          # your groups
keyman group remove team sam@example.com   # remove a person or cancel an invite
keyman group delete team                   # delete the group and its projects
```

When you remove a person, Keyman makes a new key and encrypts the projects again.

**Accepted risk:** Keyman gives access when the email address matches. A person who controls the server could give a false key.

### Check and remove links

```sh
keyman cloud status         # each link and its state, without the network
keyman unlink my-app        # remove the link on this computer only
keyman cloud delete my-app  # delete the project from the cloud for all members
```

## Commands

| Command | What it does |
| --- | --- |
| `keyman init [file]` | Create a starter master file (default `~/.keyman.yml`) and load it |
| `keyman load <file>` | Use an existing `.yml` or `.yaml` master file |
| `keyman edit` | Open the master file in your editor, then check it |
| `keyman status` | Show every project and any problems |
| `keyman list` | List project names, and `project:stage` for each stage |
| `keyman show [project[:stage]] [--reveal]` | Show a project's variables, masked by default |
| `keyman inject [project[:stage]] [directory]` | Write the project's variables to `.env` |
| `keyman diff [project[:stage]] [directory] [--reveal]` | Compare `.env` with the master file |
| `keyman save [project[:stage]] [directory]` | Save `.env` back into the master file (`--dry-run`, `--replace`, `--new`) |
| `keyman export [project[:stage]]` | Print `export` lines for your shell |
| `keyman run [project[:stage]] [--] <command>...` | Run a command with the project's variables |
| `keyman login [--github] [--google]` | Sign in to Keyman Cloud |
| `keyman logout` | Sign out and remove the keys from this computer |
| `keyman recover [--github] [--google]` | Set a new vault password with your recovery code |
| `keyman password` | Change your vault password |
| `keyman link [project] [--group <name>]` | Connect a project to Keyman Cloud |
| `keyman unlink [project]` | Remove the cloud link on this computer |
| `keyman push [project]` | Send local changes to the cloud |
| `keyman pull [project] [--as <name>] [--keep-local] [--keep-cloud]` | Get changes from the cloud and merge them |
| `keyman cloud <status\|delete> [project]` | Show the cloud links, or delete a project from the cloud |
| `keyman group <create\|invite\|remove\|list\|delete> [group] [email]` | Share projects with a group |
| `keyman completions <shell>` | Print a shell completion script |
| `keyman help [command]` | Show help; `keyman <command> --help` also works |

`[project]` defaults to the current folder name; add `:stage` for a stage (`my-app:staging`), and `[directory]` defaults to the current directory. Mistyped commands, options, and project names get a "Did you mean" suggestion.

## Rules for the master file

- Quote every value, including numbers and `true`/`false`: `PORT: "3000"`.
- Variable names use letters, digits, and underscores, and cannot start with a digit.
- Values cannot contain line breaks.
- A key whose value is a mapping (or empty) is a stage. Stage names use letters, digits, `-`, `_` and `.`.

`keyman edit`, `keyman load`, and `keyman status` tell you exactly which project breaks a rule.

## Setup tips

### Shell completion

Complete commands, options, and your project names with Tab. Add one line to your shell's startup file:

```sh
eval "$(keyman completions bash)"                                # ~/.bashrc
source <(keyman completions zsh)                                 # ~/.zshrc, after compinit
keyman completions fish | source                                 # ~/.config/fish/config.fish
keyman completions powershell | Out-String | Invoke-Expression   # $PROFILE
```

### Editor

`keyman edit` uses `$VISUAL` or `$EDITOR`, and falls back to Notepad on Windows and vi elsewhere. For editors that return immediately, add their wait flag:

```sh
export EDITOR="code --wait"
```

### Colors

Output uses your terminal's own colors, so it matches light and dark themes. Colors turn off when output is piped. Set `NO_COLOR=1` to turn them off everywhere, or `FORCE_COLOR=1` to keep them when piping.

## Keeping secrets safe

- Keep the master file out of public folders and out of Git.
- On macOS and Linux, Keyman sets the master file and every `.env` it writes to `600` (only you can read them). Windows uses the folder's normal permissions.
- `show` and `diff` hide values unless you add `--reveal`.
- Ignore `.env` in every repository at once with a global Git ignore file:

  ```sh
  mkdir -p ~/.config/git
  printf '.env\n' >> ~/.config/git/ignore
  ```

  A `.env` that Git already tracks must be removed from Git separately.

## Development

### Build from source

Install Rust, then run:

```sh
cargo build --release
cargo install --path .    # put keyman on your PATH
```

### Tests

`cargo test` runs the unit tests and needs no network. The cloud end-to-end tests in `tests/cloud_e2e.rs` are ignored by default, so they never count as passed by mistake. They need Docker and a local Supabase stack, and they refuse to run against a server that is not on `127.0.0.1` or `localhost`:

```sh
SUPABASE_AUTH_EMAIL_ENABLED=true supabase start
set -a; eval "$(supabase status -o env)"; set +a   # API_URL, ANON_KEY, SERVICE_ROLE_KEY
KEYMAN_E2E=1 cargo test --test cloud_e2e -- --ignored --test-threads=1
```

The database tests run with `supabase test db` (file `supabase/tests/rls_test.sql`).

### Try in Git Bash on Windows

The `keyman` shell script in this repository runs the Linux binary in Ubuntu WSL and keeps the configuration in your Windows home directory. Build the binary in Ubuntu first with `~/.cargo/bin/cargo build --release`, then from Git Bash:

```sh
mkdir -p ~/keyman-demo
./keyman init
./keyman inject my-app ~/keyman-demo
```

### Releases

Every pull request runs [CI](.github/workflows/ci.yml):
- the CLI's format, lint and tests on Linux, macOS and Windows
- a shell completion check
- the cloud end-to-end and database tests against a local Supabase stack
- the desktop app's lint, tests and build on macOS and Windows

The app builds are kept as downloadable run artifacts for 14 days.

Every merge to `main` runs the [release workflow](.github/workflows/release.yml). It runs the same CI, builds the CLI for five targets, and smoke tests the npm package on Linux and Windows. When the version in `Cargo.toml` has no `vX.Y.Z` tag yet, the same run releases it: it tags the merged commit, creates the GitHub release with the notes from `CHANGELOG.md`, and publishes the npm package through the configured npm trusted publisher.

To release:

1. Set the new version in `Cargo.toml` and `npm/package.json`. They must match.
2. Add a `## X.Y.Z` section to `CHANGELOG.md`.
3. Merge to `main`.

A merge that keeps the version only tests and packages. Pushing a `vX.Y.Z` tag by hand also releases. To approve each release before it is published, add required reviewers to the `release` environment in the repository settings.
