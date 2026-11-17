use std::{fs, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use log::{error, info, warn};
use pinglow_common::{CheckResultStatus, PinglowCheck};
use pinglow_executor::{execute_check, initialize_check_workspaces, ExecutorConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentConfig {
    agent_id: String,
    controller_url: String,
    api_key_file: PathBuf,
    allowed_checks: Vec<String>,
    checks_base_path: PathBuf,
    #[serde(default = "default_timeout")]
    execution_timeout_seconds: u64,
    #[serde(default = "default_drop_privileges")]
    drop_privileges: bool,
    #[serde(default, rename = "checkUidBase")]
    _legacy_check_uid_base: Option<u32>,
    #[serde(default, rename = "checkUidCount")]
    _legacy_check_uid_count: Option<u32>,
}

fn default_timeout() -> u64 {
    120
}

fn default_drop_privileges() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentJob {
    id: String,
    target: String,
    expires_at: DateTime<Utc>,
    check: PinglowCheck,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentResult {
    target: String,
    check_name: String,
    output: String,
    status: CheckResultStatus,
    timestamp: DateTime<Utc>,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let config_path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Usage: pinglow-agent <config.yaml>"))?;
    let config: AgentConfig = serde_yaml::from_slice(
        &fs::read(&config_path).with_context(|| format!("reading {config_path}"))?,
    )
    .with_context(|| format!("parsing {config_path}"))?;

    validate_config(&config)?;
    if config._legacy_check_uid_base.is_some() || config._legacy_check_uid_count.is_some() {
        warn!("Ignoring legacy checkUidBase/checkUidCount settings; script Checks now share the pinglow-check account");
    }
    initialize_check_workspaces(
        &config.checks_base_path.to_string_lossy(),
        config.drop_privileges,
    )?;
    let api_key = fs::read_to_string(&config.api_key_file)
        .with_context(|| format!("reading API key file {}", config.api_key_file.display()))?
        .trim()
        .to_owned();
    if api_key.is_empty() {
        bail!("API key file is empty");
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()?;
    let base_url = config.controller_url.trim_end_matches('/');
    info!("Starting Pinglow agent {}", config.agent_id);

    loop {
        let response = client
            .post(format!("{base_url}/agent/jobs/next"))
            .header("x-api-key", &api_key)
            .send()
            .await;

        let response = match response {
            Ok(response) if response.status() == reqwest::StatusCode::NO_CONTENT => continue,
            Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
                bail!("Agent credentials were rejected by the controller")
            }
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                warn!("Agent poll failed with HTTP {}", response.status());
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
            Err(error) => {
                warn!("Cannot reach controller: {error}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        let job: AgentJob = match response.json().await {
            Ok(job) => job,
            Err(error) => {
                error!("Controller returned an invalid job: {error}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        let check_name = job.check.check_name.clone();
        let target = job.target.clone();

        let result = if job.expires_at <= Utc::now() {
            AgentResult {
                target: target.clone(),
                check_name,
                output: "Job expired before execution".to_owned(),
                status: CheckResultStatus::CheckError,
                timestamp: Utc::now(),
            }
        } else if job.check.run_on != format!("agent/{target}") {
            AgentResult {
                target: target.clone(),
                check_name,
                output: "Job target does not match its queued Check".to_owned(),
                status: CheckResultStatus::CheckError,
                timestamp: Utc::now(),
            }
        } else if !config.allowed_checks.iter().any(|name| name == &check_name) {
            AgentResult {
                target: target.clone(),
                check_name,
                output: "Check is not present in the local agent allowlist".to_owned(),
                status: CheckResultStatus::CheckError,
                timestamp: Utc::now(),
            }
        } else {
            let result = execute_check(
                job.check,
                &ExecutorConfig {
                    base_path: config.checks_base_path.to_string_lossy().into_owned(),
                    drop_privileges: config.drop_privileges,
                    timeout: Duration::from_secs(config.execution_timeout_seconds),
                },
            )
            .await;
            AgentResult {
                target,
                check_name: result.check_name,
                output: result.output,
                status: result.status,
                timestamp: result.timestamp.unwrap_or_else(Utc::now),
            }
        };

        let submit_url = format!("{base_url}/agent/jobs/{}/result", job.id);
        if let Err(error) = submit_result(&client, &submit_url, &api_key, &result).await {
            warn!("Could not submit result for job {}: {error}", job.id);
        }
    }
}

async fn submit_result(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    result: &AgentResult,
) -> Result<()> {
    loop {
        match client
            .post(url)
            .header("x-api-key", api_key)
            .json(result)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response)
                if matches!(
                    response.status(),
                    reqwest::StatusCode::UNAUTHORIZED
                        | reqwest::StatusCode::FORBIDDEN
                        | reqwest::StatusCode::BAD_REQUEST
                        | reqwest::StatusCode::NOT_FOUND
                ) =>
            {
                bail!("Controller rejected result with HTTP {}", response.status())
            }
            Ok(response) => warn!("Result submission returned HTTP {}", response.status()),
            Err(error) => warn!("Result submission failed: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn validate_config(config: &AgentConfig) -> Result<()> {
    if config.agent_id.is_empty()
        || !config
            .agent_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("agentId must contain only ASCII letters, digits, or hyphens");
    }
    if config.allowed_checks.is_empty() {
        bail!("allowedChecks must contain at least one check name");
    }
    if config.execution_timeout_seconds == 0 || config.execution_timeout_seconds > 3600 {
        bail!("executionTimeoutSeconds must be between 1 and 3600");
    }
    let url = reqwest::Url::parse(&config.controller_url)?;
    let is_loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback) {
        bail!("controllerUrl must use HTTPS except for localhost development");
    }
    if !config.drop_privileges && !is_loopback {
        bail!("dropPrivileges may only be disabled for a localhost development controller");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(controller_url: &str, drop_privileges: bool) -> AgentConfig {
        AgentConfig {
            agent_id: "nas".to_owned(),
            controller_url: controller_url.to_owned(),
            api_key_file: "/tmp/api-key".into(),
            allowed_checks: vec!["disk".to_owned()],
            checks_base_path: "/tmp/checks".into(),
            execution_timeout_seconds: 120,
            drop_privileges,
            _legacy_check_uid_base: None,
            _legacy_check_uid_count: None,
        }
    }

    #[test]
    fn local_development_may_disable_privilege_drop() {
        assert!(validate_config(&config("http://127.0.0.1:8000", false)).is_ok());
        assert!(validate_config(&config("https://controller.example", false)).is_err());
    }

    #[test]
    fn remote_agents_require_https_and_privilege_drop() {
        assert!(validate_config(&config("https://controller.example", true)).is_ok());
        assert!(validate_config(&config("http://controller.example", true)).is_err());
    }

    #[test]
    fn agent_config_ignores_legacy_uid_range_fields() {
        let parsed: AgentConfig = serde_yaml::from_str(
            "agentId: nas\n\
             controllerUrl: https://controller.example\n\
             apiKeyFile: /etc/pinglow-agent/api-key\n\
             allowedChecks: [disk]\n\
             checksBasePath: /var/lib/pinglow-agent/checks\n\
             checkUidBase: 200000\n\
             checkUidCount: 100000\n",
        )
        .unwrap();

        assert_eq!(parsed._legacy_check_uid_base, Some(200_000));
        assert_eq!(parsed._legacy_check_uid_count, Some(100_000));
    }

    #[test]
    fn parses_targeted_gateway_job_wire_format() {
        let job: AgentJob = serde_json::from_value(serde_json::json!({
            "id": "1-0",
            "target": "linux-hosts",
            "expiresAt": "2026-11-15T17:00:00Z",
            "check": {
                "passive": false,
                "script": null,
                "collector": {
                    "type": "memory",
                    "warningPercent": 80.0,
                    "criticalPercent": 90.0
                },
                "interval": 60,
                "check_name": "host-memory",
                "run_on": "agent/linux-hosts",
                "expires_at": "2026-11-15T17:00:00Z",
                "group_id": null,
                "group_name": null,
                "secrets": null,
                "telegram_channels": [],
                "mute_notifications": null,
                "mute_notifications_until": null
            }
        }))
        .unwrap();

        assert_eq!(job.target, "linux-hosts");
        assert_eq!(job.check.run_on, "agent/linux-hosts");
    }
}
