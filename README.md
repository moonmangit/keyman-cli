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

## Commands

| Command | What it does |
| --- | --- |
| `keyman init [file]` | Create a starter master file (default `~/.keyman.yml`) and load it |
| `keyman load <file>` | Use an existing `.yml` or `.yaml` master file |
| `keyman edit` | Open the master file in your editor, then check it |
| `keyman status` | Show every project and any problems |
| `keyman list` | List project names |
| `keyman show [project] [--reveal]` | Show a project's variables, masked by default |
| `keyman inject [project] [directory]` | Write the project's variables to `.env` |
| `keyman diff [project] [directory] [--reveal]` | Compare `.env` with the master file |
| `keyman save [project] [directory]` | Save `.env` back into the master file (`--dry-run`, `--replace`, `--new`) |
| `keyman export [project]` | Print `export` lines for your shell |
| `keyman run [project] [--] <command>...` | Run a command with the project's variables |
| `keyman completions <shell>` | Print a shell completion script |
| `keyman help [command]` | Show help; `keyman <command> --help` also works |

`[project]` defaults to the current folder name, and `[directory]` defaults to the current directory. Mistyped commands, options, and project names get a "Did you mean" suggestion.

## Rules for the master file

- Quote every value, including numbers and `true`/`false`: `PORT: "3000"`.
- Variable names use letters, digits, and underscores, and cannot start with a digit.
- Values cannot contain line breaks.

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

### Try in Git Bash on Windows

The `keyman` shell script in this repository runs the Linux binary in Ubuntu WSL and keeps the configuration in your Windows home directory. Build the binary in Ubuntu first with `~/.cargo/bin/cargo build --release`, then from Git Bash:

```sh
mkdir -p ~/keyman-demo
./keyman init
./keyman inject my-app ~/keyman-demo
```

### Releases

Update the version in `Cargo.toml` and `npm/package.json` together, commit and push, then push a `vX.Y.Z` tag. The [release workflow](.github/workflows/release.yml) tests the Rust CLI, builds binaries for five targets, smoke tests the npm package on Linux and Windows, creates a GitHub release, and publishes the npm package through a configured npm trusted publisher. The package's version must match the tag.
