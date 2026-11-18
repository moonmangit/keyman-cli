# keyman-cli

Install the Rust-based Keyman CLI with npm:

```sh
npm install -g keyman-cli
```

Get started:

```sh
keyman init          # create ~/.keyman.yml with an example project and load it
keyman edit          # add your projects and secrets
cd ~/code/my-app
keyman inject        # write .env for the project named after this folder
```

The master file maps project names to quoted string values:

```yaml
my-app:
  API_KEY: "secret-value"
  DEBUG: "true"
```

More commands (leave out the project to use the current folder name):

```sh
keyman load /path/to/master.yml   # use an existing master file
keyman status                     # check every project
keyman show my-app [--reveal]     # variable names, values masked by default
keyman diff [--reveal]            # git-style diff of .env against the master file
keyman save [--dry-run]           # save this folder's .env back to the master file
eval "$(keyman export my-app)"    # load variables into a POSIX shell
keyman run my-app -- npm start    # run a command with the variables, no .env file
keyman completions bash           # also zsh, fish, powershell
```

Run `keyman help <command>` for details. Keep the master file private and ignore `.env` in Git.

A prebuilt binary is included for Windows x64. No Rust installation is needed. For macOS and Linux, build from source: `cargo install --git https://github.com/moonmangit/keyman-cli`. The executable is available as both `keyman` and `keyman-cli`. [Source and full documentation](https://github.com/moonmangit/keyman-cli).
