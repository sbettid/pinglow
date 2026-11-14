use std::fs;
use std::process::{Command, Stdio};

use anyhow::{bail, Error};
use chrono::Utc;
use pinglow_common::{CheckResult, CheckResultStatus, PinglowCheck};

fn clear_child_env(command: &mut Command) {
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
}

pub async fn execute_check(check: PinglowCheck, base_path: &str) -> Result<CheckResult, Error> {
    // Get the script
    let script = check
        .script
        .ok_or(pinglow_common::error::ScriptError::NoScriptFound(
            check.check_name.clone(),
        ))?;

    // Ensure we have a folder for this check
    let check_dir = format!("{}/check-{}", base_path, check.check_name);
    let script_path = format!("{}/script.py", &check_dir);
    let venv_path = format!("{}/venv", &check_dir);

    fs::create_dir_all(&check_dir)?;

    // Write the script in the check dir
    fs::write(&script_path, &script.content)?;

    // Create the venv
    let mut command = Command::new("python3");
    clear_child_env(&mut command);
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .args(["-m", "venv", &venv_path])
        .output()?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Error creating venv (status: {}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout,
            stderr
        );
    }

    // Install dependencies, if any
    if let Some(reqs) = script.python_requirements {
        let mut args = vec!["install"];
        args.extend(reqs.iter().map(|s| s.as_str()));

        let mut command = Command::new(format!("{venv_path}/bin/pip"));
        clear_child_env(&mut command);
        let output = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(args)
            .output()?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "Error installing dependencies (status: {}):\nstdout:\n{}\nstderr:\n{}",
                output.status,
                stdout,
                stderr
            );
        }
    }

    // Run check in the venv
    let mut command = Command::new(format!("{venv_path}/bin/python"));
    clear_child_env(&mut command);
    command.arg(script_path).stdout(Stdio::piped());

    // Check if we have secrets
    if let Some(secrets) = check.secrets {
        // Inject secrets
        for (k, v) in secrets.iter() {
            command.env(k, v);
        }
    }

    let output = command.output()?;

    // Wait for completion
    let exit_status =
        output
            .status
            .code()
            .ok_or(pinglow_common::error::ExecutionError::ExitCodeError(
                "Cannot extract exit code".to_string(),
            ))?;

    // Return the check result object
    let result = CheckResult {
        check_name: check.check_name,
        output: String::from_utf8(output.stdout)?,
        status: CheckResultStatus::from(exit_status),
        timestamp: Some(Utc::now()),
        telegram_channels: check.telegram_channels.into(),
        mute_notifications: check.mute_notifications,
        mute_notifications_until: check.mute_notifications_until,
    };

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::clear_child_env;
    use std::process::Command;

    #[test]
    fn child_environment_excludes_runner_secrets_and_keeps_check_secrets() {
        let mut command = Command::new("env");
        command.env("REDIS_PASSWORD", "runner-secret");
        command.env("REDIS_HOST", "redis");
        clear_child_env(&mut command);
        command.env("CHECK_TOKEN", "check-secret");

        let output = command
            .output()
            .expect("failed to inspect child environment");
        assert!(output.status.success());

        let environment = String::from_utf8_lossy(&output.stdout);
        assert!(!environment
            .lines()
            .any(|line| line.starts_with("REDIS_PASSWORD=")));
        assert!(!environment
            .lines()
            .any(|line| line.starts_with("REDIS_HOST=")));
        assert!(environment
            .lines()
            .any(|line| line == "CHECK_TOKEN=check-secret"));
        assert!(environment.lines().any(|line| line.starts_with("PATH=")));
    }
}
