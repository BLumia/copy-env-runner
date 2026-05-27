# Copy Environment Runner

Run a command with another process's environment variables.

## Why?

In some cases, like in a VSCode Remote session, one might need to run a target executable with the same environment variables as another existing running process. Maybe reusing `XAUTHORITY` and `DISPLAY`, maybe `WAYLAND_DISPLAY`, and maybe other environment variables. This tool can be handy for such use case.

This tool also allows user to override environment variables or remove environment variables if needed.

## Installation

### Via `cargo-binstall` (suggested)

If you have [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) installed, you can use it to install this program.

```bash
cargo binstall copy-env-runner
```

### Via `cargo install`

```bash
cargo install copy-env-runner
```

## LICENSE

copy-env-runner as a whole is licensed under MIT license. Individual files may have a different, but compatible license.
