use std::{
    collections::BTreeMap,
    ffi::CString,
    fs,
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use pinglow_common::{BuiltinCollector, CheckResult, CheckResultStatus, PinglowCheck};
use tokio::{process::Command, time::timeout};

pub const DEFAULT_CHECK_UID_BASE: u32 = 200_000;
pub const DEFAULT_CHECK_UID_COUNT: u32 = 100_000;
const CHECK_UID_MAP_FILE: &str = ".check-uids.json";
const WORK_DIR_NAME: &str = "work-isolated";
const LINUX_CAPABILITY_VERSION_3: u32 = 0x20080522;
static CHECK_UID_ALLOCATOR_LOCK: Mutex<()> = Mutex::new(());

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: libc::pid_t,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

#[derive(Clone, Debug)]
pub struct ExecutorConfig {
    pub base_path: String,
    pub drop_privileges: bool,
    pub check_uid_base: u32,
    pub check_uid_count: u32,
    pub timeout: Duration,
}

pub fn initialize_check_workspaces(
    base_path: &str,
    drop_privileges: bool,
    uid_base: u32,
    uid_count: u32,
) -> Result<()> {
    if !drop_privileges {
        return Ok(());
    }
    validate_check_uid_range(uid_base, uid_count)?;

    fs::create_dir_all(base_path)
        .with_context(|| format!("creating checks directory {base_path}"))?;
    fs::set_permissions(base_path, fs::Permissions::from_mode(0o750))
        .with_context(|| format!("setting permissions on checks directory {base_path}"))?;

    let expected_owner = unsafe { libc::geteuid() };
    let uid_end = uid_base + uid_count;
    let uid_map = read_check_uid_map(&Path::new(base_path).join(CHECK_UID_MAP_FILE))?;
    if uid_map
        .values()
        .any(|uid| !(uid_base..uid_end).contains(uid))
    {
        bail!(
            "check UID mapping contains UIDs outside the configured range; remove the entire checks directory and restart the service"
        );
    }
    let passwd_uids = read_passwd_uids()?;
    if uid_map.values().any(|uid| passwd_uids.contains(uid)) {
        bail!(
            "check UID mapping conflicts with an account in /etc/passwd; reserve a different UID range and reset the entire checks directory"
        );
    }
    if !uid_map.is_empty() {
        let process_uids = read_process_uids()?;
        if uid_map.values().any(|uid| process_uids.contains(uid)) {
            bail!(
                "a mapped check UID is already in use by a process; reserve a different UID range or stop the conflicting process"
            );
        }
    }

    for entry in
        fs::read_dir(base_path).with_context(|| format!("reading checks directory {base_path}"))?
    {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("check-") {
            continue;
        }

        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }

        for name in ["work", WORK_DIR_NAME] {
            let path = entry.path().join(name);
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("reading {}", path.display()))
                }
            };
            if !metadata.is_dir() {
                bail!("check workspace {} is not a directory", path.display());
            }
            if name == WORK_DIR_NAME {
                let directory_name = entry.file_name();
                let Some(check_name) = directory_name
                    .to_string_lossy()
                    .strip_prefix("check-")
                    .map(str::to_owned)
                else {
                    bail!(
                        "check workspace {} has an invalid parent directory name",
                        path.display()
                    );
                };
                if !uid_map.contains_key(&check_name) {
                    bail!(
                        "check workspace {} has no UID mapping; remove the entire checks directory and restart the service",
                        path.display()
                    );
                }
            }
            if metadata.uid() != expected_owner {
                bail!(
                    "check workspace {} is owned by UID {}, expected UID {}; remove the old checks directory and restart the service",
                    path.display(),
                    metadata.uid(),
                    expected_owner
                );
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("restricting legacy workspace {}", path.display()))?;
        }
    }

    Ok(())
}

pub fn validate_check_uid_range(uid_base: u32, uid_count: u32) -> Result<()> {
    if uid_base < 1_000 || uid_count == 0 || uid_base.checked_add(uid_count).is_none() {
        bail!("check UID range must start at 1000 or higher and contain valid UIDs");
    }
    Ok(())
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
    let check_dir = format!("{}/check-{check_name}", config.base_path);
    let script_path = format!("{check_dir}/script.py");
    let legacy_work_dir = format!("{check_dir}/work");
    let work_dir = if config.drop_privileges {
        format!("{check_dir}/{WORK_DIR_NAME}")
    } else {
        legacy_work_dir
    };
    let venv_path = format!("{work_dir}/venv");

    fs::create_dir_all(&config.base_path)?;
    fs::set_permissions(&config.base_path, fs::Permissions::from_mode(0o750))?;
    fs::create_dir_all(&check_dir)?;
    fs::set_permissions(&check_dir, fs::Permissions::from_mode(0o750))?;
    let check_uid = if config.drop_privileges {
        let uid = allocate_check_uid(
            &config.base_path,
            check_name,
            config.check_uid_base,
            config.check_uid_count,
        )?;
        prepare_check_work_dir(&work_dir, uid)?;
        uid
    } else {
        fs::create_dir_all(&work_dir)?;
        unsafe { libc::geteuid() }
    };
    fs::write(&script_path, &script.content)?;
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o640))?;

    let mut command = Command::new("python3");
    configure_check_process(&mut command, &work_dir, config.drop_privileges, check_uid);
    command.args(["-m", "venv", &venv_path]);
    ensure_success(command, "creating Python virtual environment").await?;

    if let Some(requirements) = script.python_requirements {
        if !requirements.is_empty() {
            let mut command = Command::new(format!("{venv_path}/bin/pip"));
            configure_check_process(&mut command, &work_dir, config.drop_privileges, check_uid);
            command.arg("install").args(requirements);
            ensure_success(command, "installing Python requirements").await?;
        }
    }

    let mut command = Command::new(format!("{venv_path}/bin/python"));
    configure_check_process(&mut command, &work_dir, config.drop_privileges, check_uid);
    command
        .arg(script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
        bail!(
            "Error {action} (status: {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn configure_check_process(
    command: &mut Command,
    work_dir: &str,
    drop_privileges: bool,
    check_uid: u32,
) {
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("HOME", work_dir)
        .env("TMPDIR", work_dir)
        .kill_on_drop(true);

    if drop_privileges {
        unsafe {
            command.as_std_mut().pre_exec(move || {
                if libc::setuid(check_uid) == -1 {
                    return Err(std::io::Error::last_os_error());
                }

                if libc::prctl(
                    libc::PR_CAP_AMBIENT,
                    libc::PR_CAP_AMBIENT_CLEAR_ALL,
                    0,
                    0,
                    0,
                ) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }

                let header = CapHeader {
                    version: LINUX_CAPABILITY_VERSION_3,
                    pid: 0,
                };
                let data = [CapData::default(); 2];
                if libc::syscall(libc::SYS_capset, &header, data.as_ptr()) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

fn allocate_check_uid(
    base_path: &str,
    check_name: &str,
    uid_base: u32,
    uid_count: u32,
) -> Result<u32> {
    validate_check_uid_range(uid_base, uid_count)?;
    let uid_end = uid_base + uid_count;

    let _guard = CHECK_UID_ALLOCATOR_LOCK
        .lock()
        .map_err(|_| anyhow!("check UID allocator lock is poisoned"))?;
    let base_path = Path::new(base_path);
    let map_path = base_path.join(CHECK_UID_MAP_FILE);
    let mut uid_map = read_check_uid_map(&map_path)?;

    if uid_map
        .values()
        .any(|uid| !(uid_base..uid_end).contains(uid))
    {
        bail!(
            "check UID mapping {} contains UIDs outside the configured range; remove the entire checks directory and restart the service",
            map_path.display()
        );
    }
    let passwd_uids = read_passwd_uids()?;
    if uid_map.values().any(|uid| passwd_uids.contains(uid)) {
        bail!(
            "check UID mapping {} conflicts with an account in /etc/passwd",
            map_path.display()
        );
    }
    if let Some(uid) = uid_map.get(check_name).copied() {
        return Ok(uid);
    }

    let mut unavailable_uids = passwd_uids;
    unavailable_uids.extend(read_process_uids()?);
    unavailable_uids.extend(uid_map.values().copied());

    let uid = (uid_base..uid_end)
        .find(|uid| !unavailable_uids.contains(uid))
        .ok_or_else(|| anyhow!("no available UID remains in the configured check UID range"))?;
    uid_map.insert(check_name.to_owned(), uid);
    write_check_uid_map(&map_path, &uid_map)?;
    Ok(uid)
}

fn read_check_uid_map(path: &Path) -> Result<BTreeMap<String, u32>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if !metadata.is_file() {
        bail!("check UID mapping {} is not a regular file", path.display());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))?;
    let contents =
        fs::read(path).with_context(|| format!("reading check UID mapping {}", path.display()))?;
    let uid_map: BTreeMap<String, u32> = serde_json::from_slice(&contents)
        .with_context(|| format!("parsing check UID mapping {}", path.display()))?;
    let mut unique_uids = std::collections::BTreeSet::new();
    if uid_map.values().any(|uid| !unique_uids.insert(*uid)) {
        bail!(
            "check UID mapping {} contains duplicate UIDs",
            path.display()
        );
    }
    Ok(uid_map)
}

fn write_check_uid_map(path: &Path, uid_map: &BTreeMap<String, u32>) -> Result<()> {
    let temp_path = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let contents = serde_json::to_vec(uid_map).context("serializing check UID mapping")?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp_path)
        .with_context(|| format!("creating check UID mapping {}", temp_path.display()))?;
    file.write_all(&contents)
        .with_context(|| format!("writing check UID mapping {}", temp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing check UID mapping {}", temp_path.display()))?;
    drop(file);
    fs::rename(&temp_path, path)
        .with_context(|| format!("installing check UID mapping {}", path.display()))?;
    Ok(())
}

fn read_passwd_uids() -> Result<std::collections::BTreeSet<u32>> {
    let mut uids = std::collections::BTreeSet::new();
    let passwd = fs::read_to_string("/etc/passwd")
        .context("reading /etc/passwd for check UID allocation")?;
    for line in passwd.lines() {
        if let Some(uid) = line.split(':').nth(2).and_then(|field| field.parse().ok()) {
            uids.insert(uid);
        }
    }
    Ok(uids)
}

fn read_process_uids() -> Result<std::collections::BTreeSet<u32>> {
    let mut uids = std::collections::BTreeSet::new();
    for entry in fs::read_dir("/proc").context("reading /proc for check UID allocation")? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let status_path = entry.path().join("status");
        let status = match fs::read_to_string(&status_path) {
            Ok(status) => status,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", status_path.display()))
            }
        };
        if let Some(line) = status.lines().find(|line| line.starts_with("Uid:")) {
            for uid in line
                .split_whitespace()
                .skip(1)
                .filter_map(|value| value.parse().ok())
            {
                uids.insert(uid);
            }
        }
    }

    Ok(uids)
}

fn prepare_check_work_dir(path: &str, check_uid: u32) -> Result<()> {
    let path = PathBuf::from(path);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                bail!("check workspace {} is not a directory", path.display());
            }
            let expected_owner = unsafe { libc::geteuid() };
            if metadata.uid() != expected_owner {
                bail!(
                    "check workspace {} is owned by UID {}, expected UID {}; remove the old checks directory and restart the service",
                    path.display(),
                    metadata.uid(),
                    expected_owner
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&path)
                .with_context(|| format!("creating check workspace {}", path.display()))?;
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    }

    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("setting permissions on {}", path.display()))?;
    set_named_user_acl(&path, check_uid)
}

fn set_named_user_acl(path: &Path, check_uid: u32) -> Result<()> {
    const ACL_USER_OBJ: u16 = 0x01;
    const ACL_USER: u16 = 0x02;
    const ACL_GROUP_OBJ: u16 = 0x04;
    const ACL_MASK: u16 = 0x10;
    const ACL_OTHER: u16 = 0x20;
    const ACL_UNDEFINED_ID: u32 = u32::MAX;

    let mut acl = Vec::with_capacity(4 + 5 * 8);
    acl.extend_from_slice(&2_u32.to_le_bytes());
    for (tag, permissions, id) in [
        (ACL_USER_OBJ, 0b111_u16, ACL_UNDEFINED_ID),
        (ACL_USER, 0b111_u16, check_uid),
        (ACL_GROUP_OBJ, 0_u16, ACL_UNDEFINED_ID),
        (ACL_MASK, 0b111_u16, ACL_UNDEFINED_ID),
        (ACL_OTHER, 0_u16, ACL_UNDEFINED_ID),
    ] {
        acl.extend_from_slice(&tag.to_le_bytes());
        acl.extend_from_slice(&permissions.to_le_bytes());
        acl.extend_from_slice(&id.to_le_bytes());
    }

    let path_cstr = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("encoding workspace path {}", path.display()))?;
    let result = unsafe {
        libc::setxattr(
            path_cstr.as_ptr(),
            b"system.posix_acl_access\0".as_ptr().cast(),
            acl.as_ptr().cast(),
            acl.len(),
            0,
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("setting per-check ACL on {}", path.display()));
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
    async fn child_environment_excludes_executor_secrets() {
        let mut command = Command::new("env");
        command.env("AGENT_API_KEY", "agent-secret");
        configure_check_process(&mut command, "/tmp", false, 1_500_000_000);
        command.env("CHECK_TOKEN", "check-secret");
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        let environment = String::from_utf8_lossy(&output.stdout);
        assert!(!environment
            .lines()
            .any(|line| line.starts_with("AGENT_API_KEY=")));
        assert!(environment
            .lines()
            .any(|line| line == "CHECK_TOKEN=check-secret"));
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
                check_uid_base: DEFAULT_CHECK_UID_BASE,
                check_uid_count: DEFAULT_CHECK_UID_COUNT,
                timeout: Duration::from_secs(5),
            },
        )
        .await;

        assert_ne!(result.status, CheckResultStatus::CheckError);
        assert_ne!(result.status, CheckResultStatus::Pending);
        assert!(result.output.contains("|memory_used_pct="));
        assert!(result.output.contains("memory_available_gib="));
    }

    #[test]
    #[ignore = "requires the runner's SETUID capability"]
    fn check_script_cannot_regain_executor_uid() {
        let executor_uid = unsafe { libc::geteuid() };
        let script = format!(
            "import os, sys\ntry:\n os.setuid({executor_uid})\nexcept PermissionError:\n pass\nelse:\n sys.exit('regained executor uid')"
        );
        let mut command = Command::new("python3");
        configure_check_process(&mut command, "/tmp", true, 1_500_000_000);
        command.arg("-c").arg(script);
        let output = command.as_std_mut().output().unwrap();
        assert!(output.status.success());
    }

    #[test]
    fn check_uid_allocation_is_stable_and_unique() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base_path =
            std::env::temp_dir().join(format!("pinglow-check-uids-{}-{nonce}", std::process::id()));
        fs::create_dir(&base_path).unwrap();
        let base_path = base_path.to_string_lossy().into_owned();
        let uid_base = 1_500_000_000;

        let first = allocate_check_uid(&base_path, "disk-check", uid_base, 100).unwrap();
        assert_eq!(
            allocate_check_uid(&base_path, "disk-check", uid_base, 100).unwrap(),
            first
        );
        let second = allocate_check_uid(&base_path, "memory-check", uid_base, 100).unwrap();
        assert_ne!(first, second);

        let map_metadata = fs::metadata(Path::new(&base_path).join(CHECK_UID_MAP_FILE)).unwrap();
        assert_eq!(map_metadata.permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(base_path).unwrap();
    }

    #[test]
    fn check_workspace_acl_grants_only_the_named_uid() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("pinglow-check-acl-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();

        set_named_user_acl(&path, 1_500_000_000).unwrap();

        let path_cstr = CString::new(path.as_os_str().as_bytes()).unwrap();
        let mut acl = [0_u8; 44];
        let acl_len = unsafe {
            libc::getxattr(
                path_cstr.as_ptr(),
                b"system.posix_acl_access\0".as_ptr().cast(),
                acl.as_mut_ptr().cast(),
                acl.len(),
            )
        };
        assert_eq!(acl_len, acl.len() as isize);
        assert_eq!(u32::from_le_bytes(acl[0..4].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(acl[14..16].try_into().unwrap()), 0b111);
        assert_eq!(
            u32::from_le_bytes(acl[16..20].try_into().unwrap()),
            1_500_000_000
        );
        assert_eq!(u16::from_le_bytes(acl[22..24].try_into().unwrap()), 0);

        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn check_uid_range_rejects_root_and_overflow() {
        assert!(validate_check_uid_range(999, 1).is_err());
        assert!(validate_check_uid_range(u32::MAX - 1, 2).is_err());
        assert!(validate_check_uid_range(200_000, 100_000).is_ok());
    }

    #[test]
    fn initialization_locks_legacy_workspaces() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base_path = std::env::temp_dir().join(format!(
            "pinglow-legacy-workspace-{}-{nonce}",
            std::process::id()
        ));
        let old_work = base_path.join("check-old/work");
        fs::create_dir_all(&old_work).unwrap();
        fs::set_permissions(&old_work, fs::Permissions::from_mode(0o770)).unwrap();

        initialize_check_workspaces(&base_path.to_string_lossy(), true, 1_500_000_000, 100)
            .unwrap();

        assert_eq!(
            fs::metadata(&old_work).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(base_path).unwrap();
    }

    #[test]
    fn initialization_fails_closed_when_uid_mapping_is_missing() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base_path = std::env::temp_dir().join(format!(
            "pinglow-missing-uid-map-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(base_path.join("check-disk/work-isolated")).unwrap();

        let error =
            initialize_check_workspaces(&base_path.to_string_lossy(), true, 1_500_000_000, 100)
                .unwrap_err();
        assert!(error.to_string().contains("has no UID mapping"));
        fs::remove_dir_all(base_path).unwrap();
    }
}
