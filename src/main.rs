use std::collections::HashMap;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{ExitCode, Output};

use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "cer", version, about = "Run a command with another process's environment variables")]
#[command(group = clap::ArgGroup::new("env_source").required(true).args(["pid", "pname", "systemd"]))]
struct Cli {
    /// Target command to execute
    target: String,

    /// Reference process PID
    #[arg(long, group = "env_source")]
    pid: Option<u32>,

    /// Process name to find reference PID
    #[arg(long, group = "env_source")]
    pname: Option<String>,

    /// Use systemd environment variables (default: user)
    #[arg(long, group = "env_source", num_args = 0..=1, default_missing_value = "user")]
    systemd: Option<SystemdScope>,

    /// Remove environment variable (can be used multiple times)
    #[arg(long)]
    unset_env: Vec<String>,

    /// Remove environment variables separated by ':' (e.g., ENV1:ENV2:ENV3)
    #[arg(long)]
    unset_envs: Option<String>,

    /// Set/override environment variable as KEY=VALUE (can be used multiple times)
    #[arg(long)]
    set_env: Vec<String>,

    /// Print final environment variables without executing the target
    #[arg(long)]
    dry_run: bool,

    /// Arguments passed to the target command
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    target_args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
enum SystemdScope {
    User,
    System,
}

fn read_systemd_envs(scope: &SystemdScope) -> Result<HashMap<String, String>, String> {
    let mut cmd = std::process::Command::new("systemctl");
    if *scope == SystemdScope::User {
        cmd.arg("--user");
    }
    cmd.arg("show-environment");

    let Output { status, stdout, stderr } = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "systemctl not found".to_string()
        } else {
            format!("Failed to run systemctl: {}", e)
        }
    })?;

    if !status.success() {
        let stderr_str = String::from_utf8_lossy(&stderr);
        let hint = if stderr_str.contains("Failed to connect to bus") {
            "\nHint: Ensure DBUS_SESSION_BUS_ADDRESS or XDG_RUNTIME_DIR is set for --user scope."
        } else if stderr_str.contains("not running") {
            "\nHint: systemd does not appear to be running."
        } else {
            ""
        };
        return Err(format!(
            "systemctl show-environment failed: {}{}",
            stderr_str.trim(),
            hint
        ));
    }

    let mut envs = HashMap::new();
    let output = String::from_utf8(stdout)
        .map_err(|e| format!("Invalid UTF-8 from systemctl output: {}", e))?;
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            envs.insert(key.to_string(), value.to_string());
        }
    }
    Ok(envs)
}

fn read_environ(pid: u32) -> Result<HashMap<String, String>, String> {
    let path = format!("/proc/{}/environ", pid);
    let data = fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("PID {} does not exist", pid)
        } else if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Permission denied reading {}", path)
        } else {
            format!("Failed to read {}: {}", path, e)
        }
    })?;

    let mut envs = HashMap::new();
    for entry in data.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        let entry_str =
            String::from_utf8(entry.to_vec()).map_err(|e| format!("Invalid UTF-8 in environ: {}", e))?;
        if let Some((key, value)) = entry_str.split_once('=') {
            envs.insert(key.to_string(), value.to_string());
        }
    }
    Ok(envs)
}

fn parse_set_env(spec: &str) -> Result<(String, String), String> {
    spec.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("Invalid --set-env format (missing '='): {}", spec))
}

fn resolve_pname(pname: &str) -> Result<u32, String> {
    let entries = fs::read_dir("/proc").map_err(|e| format!("Failed to read /proc: {}", e))?;
    for entry in entries.flatten() {
        let comm_path = entry.path().join("comm");
        if !comm_path.exists() {
            continue;
        }
        if let Ok(name) = fs::read_to_string(&comm_path) {
            if name.trim() == pname {
                if let Some(pid_str) = entry.file_name().to_str() {
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        return Ok(pid);
                    }
                }
            }
        }
    }
    Err(format!("No process found with name '{}'", pname))
}

fn build_final_envs(cli: &Cli, base_envs: HashMap<String, String>) -> Result<HashMap<String, String>, String> {
    let mut envs = base_envs;

    for key in &cli.unset_env {
        envs.remove(key);
    }

    if let Some(ref unset_list) = cli.unset_envs {
        for key in unset_list.split(':') {
            let key = key.trim();
            if !key.is_empty() {
                envs.remove(key);
            }
        }
    }

    for spec in &cli.set_env {
        let (key, value) = parse_set_env(spec)?;
        envs.insert(key, value);
    }

    Ok(envs)
}

fn resolve_command(target: &str, envs: &HashMap<String, String>) -> Option<std::path::PathBuf> {
    let target_path = Path::new(target);
    if target_path.is_file() {
        return Some(target_path.to_path_buf());
    }
    if target.contains('/') {
        return None;
    }
    let path_var = envs.get("PATH")?;
    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(target);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

enum EnvSource {
    Pid(u32),
    Systemd(SystemdScope),
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let env_source = if cli.pid.is_some() {
        EnvSource::Pid(cli.pid.unwrap())
    } else if cli.pname.is_some() {
        let pid = match resolve_pname(cli.pname.as_ref().unwrap()) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error: {}", e);
                return ExitCode::from(1);
            }
        };
        EnvSource::Pid(pid)
    } else if let Some(ref scope) = cli.systemd {
        EnvSource::Systemd(scope.clone())
    } else {
        unreachable!()
    };

    let base_envs = match env_source {
        EnvSource::Pid(pid) => match read_environ(pid) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Error: {}", e);
                return ExitCode::from(2);
            }
        },
        EnvSource::Systemd(ref scope) => match read_systemd_envs(scope) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Error: {}", e);
                return ExitCode::from(2);
            }
        },
    };

    let final_envs = match build_final_envs(&cli, base_envs) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::from(3);
        }
    };

    if cli.dry_run {
        let mut keys: Vec<_> = final_envs.keys().collect();
        keys.sort();
        for key in keys {
            println!("{}={}", key, final_envs[key]);
        }
        return ExitCode::from(0);
    }

    let target = resolve_command(&cli.target, &final_envs);
    if target.is_none() {
        eprintln!("Error: Target command not found: {}", cli.target);
        return ExitCode::from(4);
    }

    let err = std::process::Command::new(target.unwrap())
        .args(&cli.target_args)
        .env_clear()
        .envs(&final_envs)
        .exec();

    eprintln!("Error: Failed to execute '{}': {}", cli.target, err);
    ExitCode::from(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_set_env_valid() {
        let (k, v) = parse_set_env("FOO=bar").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "bar");
    }

    #[test]
    fn test_parse_set_env_with_equals_in_value() {
        let (k, v) = parse_set_env("FOO=bar=baz").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "bar=baz");
    }

    #[test]
    fn test_parse_set_env_empty_value() {
        let (k, v) = parse_set_env("FOO=").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "");
    }

    #[test]
    fn test_parse_set_env_missing_equals() {
        assert!(parse_set_env("FOO").is_err());
    }

    #[test]
    fn test_parse_set_env_empty_key() {
        let (k, v) = parse_set_env("=value").unwrap();
        assert_eq!(k, "");
        assert_eq!(v, "value");
    }

    #[test]
    fn test_build_final_envs_unset_single() {
        let mut base = HashMap::new();
        base.insert("PATH".to_string(), "/usr/bin".to_string());
        base.insert("HOME".to_string(), "/home/user".to_string());

        let cli = Cli {
            target: "cmd".to_string(),
            pid: Some(1),
            pname: None,
            systemd: None,
            unset_env: vec!["PATH".to_string()],
            unset_envs: None,
            set_env: vec![],
            dry_run: false,
            target_args: vec![],
        };

        let result = build_final_envs(&cli, base).unwrap();
        assert!(!result.contains_key("PATH"));
        assert_eq!(result["HOME"], "/home/user");
    }

    #[test]
    fn test_build_final_envs_unset_list() {
        let mut base = HashMap::new();
        base.insert("PATH".to_string(), "/usr/bin".to_string());
        base.insert("HOME".to_string(), "/home/user".to_string());
        base.insert("LANG".to_string(), "en_US".to_string());

        let cli = Cli {
            target: "cmd".to_string(),
            pid: Some(1),
            pname: None,
            systemd: None,
            unset_env: vec!["HOME".to_string()],
            unset_envs: Some("PATH:LANG".to_string()),
            set_env: vec![],
            dry_run: false,
            target_args: vec![],
        };

        let result = build_final_envs(&cli, base).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_final_envs_set() {
        let mut base = HashMap::new();
        base.insert("PATH".to_string(), "/usr/bin".to_string());
        base.insert("HOME".to_string(), "/home/user".to_string());

        let cli = Cli {
            target: "cmd".to_string(),
            pid: Some(1),
            pname: None,
            systemd: None,
            unset_env: vec![],
            unset_envs: None,
            set_env: vec!["PATH=/custom/bin".to_string(), "MY_VAR=hello".to_string()],
            dry_run: false,
            target_args: vec![],
        };

        let result = build_final_envs(&cli, base).unwrap();
        assert_eq!(result["PATH"], "/custom/bin");
        assert_eq!(result["HOME"], "/home/user");
        assert_eq!(result["MY_VAR"], "hello");
    }

    #[test]
    fn test_build_final_envs_set_invalid_format() {
        let cli = Cli {
            target: "cmd".to_string(),
            pid: Some(1),
            pname: None,
            systemd: None,
            unset_env: vec![],
            unset_envs: None,
            set_env: vec!["INVALID_NO_EQUALS".to_string()],
            dry_run: false,
            target_args: vec![],
        };

        assert!(build_final_envs(&cli, HashMap::new()).is_err());
    }

    #[test]
    fn test_build_final_envs_unset_and_set_combined() {
        let mut base = HashMap::new();
        base.insert("PATH".to_string(), "/usr/bin".to_string());
        base.insert("HOME".to_string(), "/home/user".to_string());

        let cli = Cli {
            target: "cmd".to_string(),
            pid: Some(1),
            pname: None,
            systemd: None,
            unset_env: vec!["HOME".to_string()],
            unset_envs: None,
            set_env: vec!["PATH=/custom".to_string()],
            dry_run: false,
            target_args: vec![],
        };

        let result = build_final_envs(&cli, base).unwrap();
        assert_eq!(result["PATH"], "/custom");
        assert!(!result.contains_key("HOME"));
    }

    #[test]
    fn test_read_environ_self() {
        let envs = read_environ(std::process::id()).unwrap();
        assert!(!envs.is_empty());
    }

    #[test]
    fn test_read_environ_nonexistent_pid() {
        let result = read_environ(999999999);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_parse_systemd_output() {
        let output = "PATH=/usr/local/bin:/usr/bin\nHOME=/home/user\nLANG=en_US.UTF-8\n";
        let mut envs = HashMap::new();
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                envs.insert(key.to_string(), value.to_string());
            }
        }
        assert_eq!(envs.len(), 3);
        assert_eq!(envs["PATH"], "/usr/local/bin:/usr/bin");
        assert_eq!(envs["HOME"], "/home/user");
        assert_eq!(envs["LANG"], "en_US.UTF-8");
    }

    #[test]
    fn test_read_systemd_envs_user() {
        let result = read_systemd_envs(&SystemdScope::User);
        if result.is_err() {
            return;
        }
        let envs = result.unwrap();
        assert!(!envs.is_empty());
    }

    #[test]
    fn test_read_systemd_envs_system() {
        let result = read_systemd_envs(&SystemdScope::System);
        if result.is_err() {
            return;
        }
        let envs = result.unwrap();
        assert!(!envs.is_empty());
    }
}
