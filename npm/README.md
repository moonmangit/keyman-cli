# keyman-cli

Install the Rust-based Keyman CLI with npm:

```sh
npm install -g keyman-cli
```

Create a private `.yml` file with one mapping per project:

```yaml
my-app:
  API_KEY: "secret-value"
  DEBUG: "true"
```

Then run:

```sh
keyman load /path/to/master.yml
keyman inject my-app /path/to/existing/project
```

`load` remembers the master file path. `inject` replaces the project's `.env` file. Keep the master file private and ignore `.env` in Git.

Prebuilt binaries are included for Windows x64, macOS Intel and Apple Silicon, and Linux x64 and ARM64. No Rust installation is needed. [Source and full documentation](https://github.com/moonmangit/keyman-cli).
