use std::{collections::BTreeMap, ffi::CString, fs, process::Stdio, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use pinglow_common::{BuiltinCollector, CheckResult, CheckResultStatus, PinglowCheck};
use tokio::{process::Command, time::timeout};

mod sandbox;

use sandbox::{check_command, prepare_check_sandbox, write_check_script};
pub use sandbox::{initialize_check_workspaces, run_sandbox_launcher, CHECK_USER_NAME};

#[derive(Clone, Debug)]
pub struct ExecutorConfig {
    pub base_path: String,
    pub drop_privileges: bool,
    pub timeout: Duration,
}

pub async fn execute_check(check: PinglowCheck, config: &ExecutorConfig) -> CheckResult {
    let check_name = check.check_name.clone();
    let mute_notifications = check.mute_notifications;
    let mute_notifications_until = check.mute_notifications_until;
    let telegram_channels = check.telegram_channels.clone();

    let execution = timeout(config.timeout, async {
        match (check.script, check.collector) {
            (Some(script), None) => {
                execute_script(&check_name, script, check.secrets, config).await
            }
            (None, Some(collector)) => execute_collector(collector).await,
            _ => bail!("A check must contain exactly one script or built-in collector"),
        }
    })
    .await;

    let (output, status) = match execution {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => (error.to_string(), CheckResultStatus::CheckError),
        Err(_) => (
            format!(
                "Check exceeded its {} second execution timeout",
                config.timeout.as_secs()
            ),
            CheckResultStatus::CheckError,
        ),
    };

    CheckResult {
        check_name,
        output,
        status,
        timestamp: Some(Utc::now()),
        telegram_channels: telegram_channels.into(),
        mute_notifications,
        mute_notifications_until,
    }
}

async fn execute_script(
    check_name: &str,
    script: pinglow_common::ScriptSpec,
    secrets: Option<std::collections::HashMap<String, String>>,
    config: &ExecutorConfig,
) -> Result<(String, CheckResultStatus)> {
    let sandbox = prepare_check_sandbox(&config.base_path, check_name, config.drop_privileges)?;
    let script_path = write_check_script(&sandbox, &script.content)?;
    let venv_path = sandbox.work_dir.join("venv");

    let command = check_command(
        &sandbox,
        "python3",
        vec![
            "-m".into(),
            "venv".into(),
            "--without-pip".into(),
            venv_path.clone().into_os_string(),
        ],
    )?;
    ensure_success(command, "creating Python virtual environment").await?;

    let command = check_command(
        &sandbox,
        venv_path.join("bin/python").as_os_str(),
        vec![
            "-m".into(),
            "ensurepip".into(),
            "--upgrade".into(),
            "--default-pip".into(),
        ],
    )?;
    ensure_success(command, "installing pip in Python virtual environment").await?;

    if let Some(requirements) = script.python_requirements {
        if !requirements.is_empty() {
            let mut args = vec![std::ffi::OsString::from("install")];
            args.extend(requirements.into_iter().map(std::ffi::OsString::from));
            let command = check_command(&sandbox, venv_path.join("bin/pip").as_os_str(), args)?;
            ensure_success(command, "installing Python requirements").await?;
        }
    }

    let mut command = check_command(
        &sandbox,
        venv_path.join("bin/python").as_os_str(),
        vec![script_path.into_os_string()],
    )?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(secrets) = secrets {
        command.envs(secrets);
    }
    let output = command
        .output()
        .await
        .context("running Python check script")?;
    let status = output
        .status
        .code()
        .map(CheckResultStatus::from)
        .unwrap_or(CheckResultStatus::CheckError);

    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned();
    if !output.status.success() && stdout.is_empty() {
        bail!(
            "Python check exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok((stdout, status))
}

async fn ensure_success(mut command: Command, action: &str) -> Result<()> {
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("{action}"))?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Error {action} (status: {}): {}{}",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok(())
}

async fn execute_collector(collector: BuiltinCollector) -> Result<(String, CheckResultStatus)> {
    match collector {
        BuiltinCollector::Disk {
            paths,
            exclude_mount_points,
            exclude_filesystem_types,
            warning_percent,
            critical_percent,
        } => {
            validate_thresholds(warning_percent, critical_percent)?;
            if paths.is_empty() {
                bail!("Disk collector requires at least one path");
            }
            if paths.iter().any(|path| !path.starts_with('/')) {
                bail!("Disk collector paths must be absolute");
            }
            let mounts = read_mounts()?;
            let mut seen = std::collections::HashSet::new();
            let mut metrics = Vec::new();
            let mut summaries = Vec::new();
            let mut status = CheckResultStatus::Ok;

            for path in paths {
                let mount = mounts
                    .iter()
                    .filter(|mount| path_matches_mount(&path, &mount.mount_point))
                    .max_by_key(|mount| mount.mount_point.len())
                    .ok_or_else(|| anyhow!("No mounted filesystem contains '{path}'"))?;
                if exclude_mount_points
                    .iter()
                    .any(|excluded| excluded == &mount.mount_point)
                    || exclude_filesystem_types
                        .iter()
                        .any(|excluded| excluded == &mount.filesystem_type)
                    || !seen.insert(mount.mount_point.clone())
                {
                    continue;
                }

                let (used_percent, free_gib) = disk_usage(&path)?;
                status = worst_status(
                    status,
                    threshold_status(used_percent, warning_percent, critical_percent),
                );
                let label = metric_label(&mount.mount_point);
                summaries.push(format!(
                    "{}: {:.1}% used, {:.1} GiB free",
                    mount.mount_point, used_percent, free_gib
                ));
                metrics.push(format!("disk_{label}_used_pct={used_percent:.1}"));
                metrics.push(format!("disk_{label}_free_gib={free_gib:.1}"));
            }

            if summaries.is_empty() {
                bail!("Disk collector found no filesystems after exclusions");
            }
            Ok((
                format!("Disk {}|{}", summaries.join("; "), metrics.join(",")),
                status,
            ))
        }
        BuiltinCollector::Memory {
            warning_percent,
            critical_percent,
        } => {
            validate_thresholds(warning_percent, critical_percent)?;
            let (used_percent, available_gib) = memory_usage()?;
            let status = threshold_status(used_percent, warning_percent, critical_percent);
            Ok((
                format!(
                    "Memory: {used_percent:.1}% used, {available_gib:.1} GiB available|memory_used_pct={used_percent:.1},memory_available_gib={available_gib:.1}"
                ),
                status,
            ))
        }
        BuiltinCollector::Load {
            period_minutes,
            warning_load,
            critical_load,
        } => {
            validate_load_thresholds(warning_load, critical_load)?;
            let averages = read_load_averages()?;
            let (metric_name, threshold_value) = load_average_for_period(averages, period_minutes)?;
            let status = threshold_status(threshold_value, warning_load, critical_load);
            Ok((
                format!(
                    "Load average ({period_minutes} min): {threshold_value:.2}|{metric_name}={threshold_value:.2}"
                ),
                status,
            ))
        }
    }
}

fn validate_load_thresholds(warning: f64, critical: f64) -> Result<()> {
    if !warning.is_finite() || !critical.is_finite() || warning < 0.0 || warning > critical {
        bail!("Load thresholds must satisfy 0 <= warningLoad <= criticalLoad");
    }
    Ok(())
}

fn validate_thresholds(warning: f64, critical: f64) -> Result<()> {
    if !warning.is_finite()
        || !critical.is_finite()
        || warning < 0.0
        || critical > 100.0
        || warning > critical
    {
        bail!("Thresholds must satisfy 0 <= warningPercent <= criticalPercent <= 100");
    }
    Ok(())
}

fn threshold_status(value: f64, warning: f64, critical: f64) -> CheckResultStatus {
    if value >= critical {
        CheckResultStatus::Critical
    } else if value >= warning {
        CheckResultStatus::Warning
    } else {
        CheckResultStatus::Ok
    }
}

fn worst_status(current: CheckResultStatus, next: CheckResultStatus) -> CheckResultStatus {
    match (current, next) {
        (CheckResultStatus::Critical, _) | (_, CheckResultStatus::Critical) => {
            CheckResultStatus::Critical
        }
        (CheckResultStatus::Warning, _) | (_, CheckResultStatus::Warning) => {
            CheckResultStatus::Warning
        }
        _ => CheckResultStatus::Ok,
    }
}

fn memory_usage() -> Result<(f64, f64)> {
    let contents = fs::read_to_string("/proc/meminfo").context("reading /proc/meminfo")?;
    let values: BTreeMap<_, _> = contents
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let value = value.split_whitespace().next()?.parse::<u64>().ok()?;
            Some((key, value))
        })
        .collect();
    let total = *values
        .get("MemTotal")
        .ok_or_else(|| anyhow!("MemTotal missing from /proc/meminfo"))?;
    let available = *values
        .get("MemAvailable")
        .ok_or_else(|| anyhow!("MemAvailable missing from /proc/meminfo"))?;
    if total == 0 {
        bail!("MemTotal is zero");
    }
    let used_percent = 100.0 * total.saturating_sub(available) as f64 / total as f64;
    Ok((used_percent, available as f64 / 1024.0 / 1024.0))
}

fn read_load_averages() -> Result<[f64; 3]> {
    let contents = fs::read_to_string("/proc/loadavg").context("reading /proc/loadavg")?;
    parse_load_averages(&contents)
}

fn parse_load_averages(contents: &str) -> Result<[f64; 3]> {
    let values: Vec<f64> = contents
        .split_whitespace()
        .take(3)
        .map(|value| value.parse::<f64>().context("parsing load average"))
        .collect::<std::result::Result<_, _>>()?;
    if values.len() != 3
        || values
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
    {
        bail!("Invalid load averages in /proc/loadavg");
    }
    Ok([values[0], values[1], values[2]])
}

fn load_average_for_period(averages: [f64; 3], period_minutes: u8) -> Result<(&'static str, f64)> {
    match period_minutes {
        1 => Ok(("load_1m", averages[0])),
        5 => Ok(("load_5m", averages[1])),
        15 => Ok(("load_15m", averages[2])),
        _ => bail!("Load periodMinutes must be one of 1, 5, or 15"),
    }
}

#[derive(Debug)]
struct Mount {
    mount_point: String,
    filesystem_type: String,
}

fn read_mounts() -> Result<Vec<Mount>> {
    let contents = fs::read_to_string("/proc/self/mountinfo").context("reading mount table")?;
    contents
        .lines()
        .map(|line| {
            let (before, after) = line
                .split_once(" - ")
                .ok_or_else(|| anyhow!("Invalid mountinfo entry"))?;
            let mount_point = before
                .split_whitespace()
                .nth(4)
                .ok_or_else(|| anyhow!("Mount point missing from mountinfo entry"))?;
            let filesystem_type = after
                .split_whitespace()
                .next()
                .ok_or_else(|| anyhow!("Filesystem type missing from mountinfo entry"))?;
            Ok(Mount {
                mount_point: decode_mount_field(mount_point),
                filesystem_type: filesystem_type.to_owned(),
            })
        })
        .collect()
}

fn decode_mount_field(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn path_matches_mount(path: &str, mount_point: &str) -> bool {
    path == mount_point
        || mount_point == "/"
        || path
            .strip_prefix(mount_point)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn disk_usage(path: &str) -> Result<(f64, f64)> {
    let path_c = CString::new(path).context("disk path contains a NUL byte")?;
    let mut stats = unsafe { std::mem::zeroed::<libc::statvfs>() };
    if unsafe { libc::statvfs(path_c.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context(format!("reading disk usage for {path}"));
    }
    let block_size = stats.f_frsize as f64;
    let total = stats.f_blocks as f64 * block_size;
    let free = stats.f_bavail as f64 * block_size;
    if total <= 0.0 {
        bail!("Filesystem containing '{path}' has no usable capacity");
    }
    Ok((100.0 * (total - free) / total, free / 1024.0_f64.powi(3)))
}

fn metric_label(value: &str) -> String {
    let label: String = value
        .trim_matches('/')
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if label.is_empty() {
        "root".to_owned()
    } else {
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn threshold_boundaries_are_inclusive() {
        assert_eq!(
            threshold_status(80.0, 80.0, 90.0),
            CheckResultStatus::Warning
        );
        assert_eq!(
            threshold_status(90.0, 80.0, 90.0),
            CheckResultStatus::Critical
        );
        assert!(validate_thresholds(91.0, 90.0).is_err());
    }

    #[test]
    fn mount_fields_decode_linux_escapes() {
        assert_eq!(decode_mount_field("/mnt/my\\040disk"), "/mnt/my disk");
    }

    #[test]
    fn disk_path_matching_respects_mount_boundaries() {
        assert!(path_matches_mount("/data/archive", "/data"));
        assert!(!path_matches_mount("/database", "/data"));
    }

    #[test]
    fn metric_labels_are_stable_and_root_is_named() {
        assert_eq!(metric_label("/"), "root");
        assert_eq!(metric_label("/mnt/data-disk"), "mnt_data_disk");
    }

    #[tokio::test]
    async fn disk_collector_rejects_relative_paths() {
        let result = execute_collector(BuiltinCollector::Disk {
            paths: vec!["relative/path".to_owned()],
            exclude_mount_points: vec![],
            exclude_filesystem_types: vec![],
            warning_percent: 80.0,
            critical_percent: 90.0,
        })
        .await;

        assert!(result.is_err());
    }

    #[test]
    fn parses_memory_and_load_averages_from_linux_procfs() {
        assert!(memory_usage().unwrap().0 >= 0.0);
        assert_eq!(
            parse_load_averages("1.25 0.75 0.5 2/200 1234").unwrap(),
            [1.25, 0.75, 0.5]
        );
        assert!(parse_load_averages("1.25 0.75").is_err());
        assert!(parse_load_averages("1.25 NaN 0.5").is_err());
        assert!(read_load_averages()
            .unwrap()
            .iter()
            .all(|value| *value >= 0.0));
    }

    #[test]
    fn load_collector_selects_the_configured_average_and_rejects_invalid_periods() {
        let averages = [1.0, 5.0, 15.0];
        assert_eq!(
            load_average_for_period(averages, 1).unwrap(),
            ("load_1m", 1.0)
        );
        assert_eq!(
            load_average_for_period(averages, 5).unwrap(),
            ("load_5m", 5.0)
        );
        assert_eq!(
            load_average_for_period(averages, 15).unwrap(),
            ("load_15m", 15.0)
        );
        assert!(load_average_for_period(averages, 10).is_err());
        assert!(validate_load_thresholds(3.0, 2.0).is_err());
    }

    #[tokio::test]
    async fn load_collector_reports_only_the_selected_average() {
        let (output, status) = execute_collector(BuiltinCollector::Load {
            period_minutes: 5,
            warning_load: 0.0,
            critical_load: f64::MAX,
        })
        .await
        .unwrap();

        assert_eq!(status, CheckResultStatus::Warning);
        assert!(output.starts_with("Load average (5 min): "));
        assert!(output.contains("|load_5m="));
        assert!(!output.contains("load_1m="));
        assert!(!output.contains("load_15m="));
    }

    #[tokio::test]
    async fn memory_collector_emits_numeric_performance_data() {
        let check = PinglowCheck {
            passive: false,
            script: None,
            collector: Some(BuiltinCollector::Memory {
                warning_percent: 80.0,
                critical_percent: 90.0,
            }),
            interval: Some(60),
            check_name: "memory".to_owned(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: None,
            group_name: None,
            secrets: Some(HashMap::new()),
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        };
        let result = execute_check(
            check,
            &ExecutorConfig {
                base_path: "/tmp/pinglow-executor-test".to_owned(),
                drop_privileges: false,
                timeout: Duration::from_secs(5),
            },
        )
        .await;

        assert_ne!(result.status, CheckResultStatus::CheckError);
        assert_ne!(result.status, CheckResultStatus::Pending);
        assert!(result.output.contains("|memory_used_pct="));
        assert!(result.output.contains("memory_available_gib="));
    }
}
