# Copy Environment Runner

Run a command with another process's environment variables.

## Why?

In some cases, like in a VSCode Remote session, one might need to run a target executable with the same environment variables as another existing running process. Maybe reusing `XAUTHORITY` and `DISPLAY`, maybe `WAYLAND_DISPLAY`, and maybe other environment variables. This tool can be handy for such use case.

This tool also allows user to override environment variables or remove environment variables if needed.

## Usage

```plain
Run a command with another process's environment variables

Usage: cer [OPTIONS] <--pid <PID>|--pname <PNAME>|--systemd [<SYSTEMD>]> <TARGET> [TARGET_ARGS]...

Arguments:
  <TARGET>          Target command to execute
  [TARGET_ARGS]...  Arguments passed to the target command

Options:
      --pid <PID>                Reference process PID
      --pname <PNAME>            Process name to find reference PID
      --systemd [<SYSTEMD>]      Use systemd environment variables (default: user) [possible values: user, system]
      --unset-env <UNSET_ENV>    Remove environment variable (can be used multiple times)
      --unset-envs <UNSET_ENVS>  Remove environment variables separated by ':' (e.g., ENV1:ENV2:ENV3)
      --set-env <SET_ENV>        Set/override environment variable as KEY=VALUE (can be used multiple times)
      --dry-run                  Print final environment variables without executing the target
  -h, --help                     Print help
  -V, --version                  Print version
```

### Sample Usage

Run `nm-applet` with `dde-shell`'s environ, with `GDK_BACKEND` set to `x11`

```shell
$ cer --pname dde-shell nm-applet --set-env="GDK_BACKEND=x11"
```

Run `nm-applet` with current systemd user environ, with `GDK_BACKEND` set to `x11`.

```shell
$ cer --systemd nm-applet --set-env="GDK_BACKEND=x11"
```

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
