use std::collections::HashMap;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "cer", version, about = "Run a command with another process's environment variables")]
#[command(group = clap::ArgGroup::new("pid_source").required(true).args(["pid", "pname"]))]
struct Cli {
    /// Target command to execute
    target: String,

    /// Reference process PID
    #[arg(long, group = "pid_source")]
    pid: Option<u32>,

    /// Process name to find reference PID
    #[arg(long, group = "pid_source")]
    pname: Option<String>,

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

fn resolve_pid(cli: &Cli) -> Result<u32, String> {
    if let Some(pid) = cli.pid {
        return Ok(pid);
    }
    if let Some(ref pname) = cli.pname {
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
        return Err(format!("No process found with name '{}'", pname));
    }
    unreachable!()
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

fn main() -> ExitCode {
    let cli = Cli::parse();

    let pid = match resolve_pid(&cli) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::from(1);
        }
    };

    let base_envs = match read_environ(pid) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error: {}", e);
            return ExitCode::from(2);
        }
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
}
