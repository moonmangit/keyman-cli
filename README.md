# Keyman CLI

Keyman reads project variables from one master `.yml` file and writes a `.env` file in a chosen project directory.

## Install

```sh
npm install -g keyman-cli
```

The npm package includes binaries for Windows x64, macOS Intel and Apple Silicon, and Linux x64 and ARM64. It does not require Rust or WSL. The `keyman` command is then available in any terminal.

## Build

Install Rust, then run:

```sh
cargo build --release
```

The Unix executable is `target/release/keyman`. To put `keyman` on your path, run `cargo install --path .`.

## Use

Create a private master file:

```yaml
project-a:
  DATABASE_URL: "postgres://user:pass@localhost:5432/db_a"
  API_KEY: "secret_key_123"

project-b:
  DEBUG: "true"
```

Then run:

```sh
keyman load /absolute/path/to/master.yml
keyman inject project-a /path/to/project
```

`load` stores the absolute master path in `~/.keyman_config` and sets the master file's permissions to `600` on Unix. `inject` replaces the target directory's `.env` file with the selected project's variables. Both generated files use `600` permissions on Unix. Windows uses the inherited Windows file permissions.

## Try in Git Bash on Windows

Use the `keyman` shell launcher in this repository. It runs the Rust binary in Ubuntu WSL and writes the configuration to your Windows home directory. From Git Bash, create a sample master file outside the repository and run:

```sh
mkdir -p ~/keyman-demo
cat > ~/keyman-master.yml <<'YAML'
demo:
  API_KEY: "test-secret"
  DEBUG: "true"
YAML

./keyman load ~/keyman-master.yml
./keyman inject demo ~/keyman-demo
cat ~/keyman-demo/.env
```

The launcher needs the `target/release/keyman` binary built in Ubuntu WSL. If it is missing, build it from the repository in Ubuntu with `~/.cargo/bin/cargo build --release`.

Project values must be YAML strings. Quote values such as `true` and numbers. Variable names must use letters, digits, and underscores and cannot start with a digit. Values cannot contain line breaks or NUL bytes. The target directory must already exist.

Keep the master file outside public directories. Add `.env` to your global Git ignore file (typically `~/.config/git/ignore`) so new repositories do not accidentally track secrets:

```sh
mkdir -p ~/.config/git
printf '.env\n' >> ~/.config/git/ignore
```

Existing tracked `.env` files must be removed from Git separately.

## Releases

Update the version in `Cargo.toml` and `npm/package.json` together, commit and push, then push a `vX.Y.Z` tag. The [release workflow](.github/workflows/release.yml) tests the Rust CLI, builds binaries for five targets, smoke tests the npm package on Linux and Windows, creates a GitHub release, and publishes the npm package through a configured npm trusted publisher. The package's version must match the tag.
