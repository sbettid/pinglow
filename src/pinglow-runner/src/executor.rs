use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};

use anyhow::{bail, Error};
use chrono::Utc;
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};
use pinglow_common::{CheckResult, CheckResultStatus, PinglowCheck};

pub async fn execute_check(
    check: PinglowCheck,
    base_path: &str,
    namespace: &str,
) -> Result<CheckResult, Error> {
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
    let output = Command::new("python3")
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

        let output = Command::new(format!("{venv_path}/bin/pip"))
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
    command.arg(script_path).stdout(Stdio::piped());

    // Check if we have secrets
    if let Some(secrets_refs) = check.secrets_refs {
        let secrets = fetch_secrets(namespace, &secrets_refs).await?;

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

async fn fetch_secrets(
    namespace: &str,
    secret_names: &[String],
) -> Result<HashMap<String, String>, Error> {
    let client = Client::try_default().await?;
    let secrets_api: Api<Secret> = Api::namespaced(client, namespace);

    let mut map = HashMap::new();

    for secret_name in secret_names {
        if let Ok(secret) = secrets_api.get(secret_name).await {
            if let Some(data) = secret.data {
                for (key, value) in data {
                    // Secrets are base64 encoded
                    let decoded = std::str::from_utf8(&value.0)?;
                    map.insert(key.clone(), decoded.to_string());
                }
            }
        }
    }

    Ok(map)
}
