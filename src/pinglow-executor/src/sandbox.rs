use std::{
    ffi::{CString, OsStr, OsString},
    fs,
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
};

use anyhow::{anyhow, bail, Context, Result};
use landlock::{
    Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, Scope, ABI,
};
use nix::unistd::User;
use tokio::process::Command;

pub const CHECK_USER_NAME: &str = "pinglow-check";
const WORK_DIR_NAME: &str = "work-sandboxed";
const LEGACY_WORK_DIR_NAMES: [&str; 2] = ["work", "work-isolated"];
const CHECK_LAUNCHER_NAME: &str = "pinglow-check-launcher";
const LANDLOCK_ABI: ABI = ABI::V6;
const CHECK_PATH: &str = "/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin";
#[cfg(target_arch = "x86_64")]
const SECCOMP_AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const SECCOMP_AUDIT_ARCH: u32 = 0xc000_00b7;

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("Check sandbox seccomp policy supports x86_64 and aarch64 only");

#[derive(Debug)]
pub(crate) struct CheckSandbox {
    pub check_dir: PathBuf,
    pub work_dir: PathBuf,
    pub identity: Option<CheckIdentity>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CheckIdentity {
    uid: u32,
    gid: u32,
}

pub fn initialize_check_workspaces(base_path: &str, drop_privileges: bool) -> Result<()> {
    validate_landlock_support()?;
    find_check_launcher()?;
    if !drop_privileges {
        return Ok(());
    }

    let identity = resolve_check_identity()?;
    let base_path = Path::new(base_path);
    fs::create_dir_all(base_path)
        .with_context(|| format!("creating checks directory {}", base_path.display()))?;
    validate_owned_directory(base_path)?;
    fs::set_permissions(base_path, fs::Permissions::from_mode(0o750))
        .with_context(|| format!("setting permissions on {}", base_path.display()))?;
    set_named_user_acl(base_path, identity.uid, 0b111, 0b001)?;

    for entry in fs::read_dir(base_path)
        .with_context(|| format!("reading checks directory {}", base_path.display()))?
    {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("check-") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!(
                "check directory {} is owned by UID {}, expected UID {}; repair its ownership before restarting",
                entry.path().display(),
                metadata.uid(),
                unsafe { libc::geteuid() }
            );
        }

        for name in LEGACY_WORK_DIR_NAMES {
            lock_legacy_workspace(&entry.path().join(name))?;
        }
        let sandbox_work_dir = entry.path().join(WORK_DIR_NAME);
        if sandbox_work_dir.exists() {
            prepare_work_directory(&sandbox_work_dir, identity.uid)?;
        }
    }

    Ok(())
}

pub(crate) fn prepare_check_sandbox(
    base_path: &str,
    check_name: &str,
    drop_privileges: bool,
) -> Result<CheckSandbox> {
    if check_name.is_empty()
        || !check_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!(
            "Check name must contain only ASCII letters, digits, hyphens, underscores, or periods"
        );
    }

    let identity = if drop_privileges {
        Some(resolve_check_identity()?)
    } else {
        None
    };
    let base_path = Path::new(base_path);
    fs::create_dir_all(base_path)
        .with_context(|| format!("creating checks directory {}", base_path.display()))?;
    validate_owned_directory(base_path)?;
    let check_dir = base_path.join(format!("check-{check_name}"));
    create_owned_directory(&check_dir, 0o750)?;
    let work_dir = check_dir.join(WORK_DIR_NAME);
    if let Some(identity) = identity {
        set_named_user_acl(base_path, identity.uid, 0b111, 0b001)?;
        set_named_user_acl(&check_dir, identity.uid, 0b111, 0b001)?;
        prepare_work_directory(&work_dir, identity.uid)?;
    } else {
        create_owned_directory(&work_dir, 0o700)?;
    }

    Ok(CheckSandbox {
        check_dir,
        work_dir,
        identity,
    })
}

pub(crate) fn write_check_script(sandbox: &CheckSandbox, content: &str) -> Result<PathBuf> {
    let script_path = sandbox.check_dir.join("script.py");
    let mut options = fs::OpenOptions::new();
    options
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o640)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(&script_path)
        .with_context(|| format!("writing Check script {}", script_path.display()))?;
    let mut file = file;
    file.write_all(content.as_bytes())
        .with_context(|| format!("writing Check script {}", script_path.display()))?;
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o640))
        .with_context(|| format!("setting permissions on {}", script_path.display()))?;
    if let Some(identity) = sandbox.identity {
        set_named_user_acl(&script_path, identity.uid, 0b110, 0b100)?;
    }
    Ok(script_path)
}

pub(crate) fn check_command(
    sandbox: &CheckSandbox,
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = OsString>,
) -> Result<Command> {
    let helper = find_check_launcher()?;
    let check_dir = fs::canonicalize(&sandbox.check_dir)
        .with_context(|| format!("resolving {}", sandbox.check_dir.display()))?;
    let work_dir = fs::canonicalize(&sandbox.work_dir)
        .with_context(|| format!("resolving {}", sandbox.work_dir.display()))?;

    let mut command = Command::new(helper);
    command
        .arg("--check-dir")
        .arg(check_dir)
        .arg("--work-dir")
        .arg(&work_dir);
    if let Some(identity) = sandbox.identity {
        command
            .arg("--drop-privileges")
            .arg(identity.uid.to_string())
            .arg(identity.gid.to_string());
    } else {
        command.arg("--no-drop-privileges");
    }
    command.arg("--").arg(program).args(args);
    command.env_clear();
    command
        .env("PATH", CHECK_PATH)
        .env("HOME", &work_dir)
        .env("TMPDIR", &work_dir)
        .current_dir(&work_dir)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

fn find_check_launcher() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PINGLOW_CHECK_LAUNCHER") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "PINGLOW_CHECK_LAUNCHER does not name a regular file: {}",
            path.display()
        );
    }

    let executable = std::env::current_exe().context("locating current executable")?;
    let sibling = executable.with_file_name(CHECK_LAUNCHER_NAME);
    if sibling.is_file() {
        return Ok(sibling);
    }
    if let Some(target_dir) = executable.parent().and_then(Path::parent) {
        let test_build_sibling = target_dir.join(CHECK_LAUNCHER_NAME);
        if test_build_sibling.is_file() {
            return Ok(test_build_sibling);
        }
    }
    bail!(
        "could not find {CHECK_LAUNCHER_NAME} beside {}; install the helper or set PINGLOW_CHECK_LAUNCHER",
        executable.display()
    )
}

fn resolve_check_identity() -> Result<CheckIdentity> {
    let user = User::from_name(CHECK_USER_NAME)
        .with_context(|| format!("looking up Check execution user {CHECK_USER_NAME}"))?
        .ok_or_else(|| anyhow!("required Check execution user {CHECK_USER_NAME} does not exist"))?;
    if user.uid.as_raw() == 0
        || user.uid.as_raw() == unsafe { libc::geteuid() }
        || user.gid.as_raw() == 0
        || user.gid.as_raw() == unsafe { libc::getegid() }
    {
        bail!(
            "Check execution user {CHECK_USER_NAME} must have a non-root UID and a dedicated non-root GID, both different from the service identity"
        );
    }
    Ok(CheckIdentity {
        uid: user.uid.as_raw(),
        gid: user.gid.as_raw(),
    })
}

fn validate_landlock_support() -> Result<()> {
    build_landlock_ruleset().context("Landlock ABI v6 is required to run Checks securely")?;
    Ok(())
}

fn build_landlock_ruleset() -> Result<landlock::RulesetCreated> {
    Ok(Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(LANDLOCK_ABI))
        .context("handling required Landlock filesystem rights")?
        .scope(Scope::from_all(LANDLOCK_ABI))
        .context("handling required Landlock scopes")?
        .create()
        .context("creating Landlock ruleset")?)
}

pub fn run_sandbox_launcher() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mut check_dir = None;
    let mut work_dir = None;
    let mut identity = None;
    loop {
        match args.next().as_deref() {
            Some(value) if value == OsStr::new("--check-dir") => {
                check_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("missing Check directory"))?,
                ));
            }
            Some(value) if value == OsStr::new("--work-dir") => {
                work_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("missing Check work directory"))?,
                ));
            }
            Some(value) if value == OsStr::new("--drop-privileges") => {
                let uid = parse_id(args.next(), "UID")?;
                let gid = parse_id(args.next(), "GID")?;
                identity = Some(CheckIdentity { uid, gid });
            }
            Some(value) if value == OsStr::new("--no-drop-privileges") => {}
            Some(value) if value == OsStr::new("--") => break,
            Some(value) => bail!("unexpected Check launcher argument {:?}", value),
            None => bail!("missing -- separator before Check command"),
        }
    }

    let check_dir = check_dir.ok_or_else(|| anyhow!("missing Check directory"))?;
    let work_dir = work_dir.ok_or_else(|| anyhow!("missing Check work directory"))?;
    let program = args
        .next()
        .ok_or_else(|| anyhow!("missing Check command"))?;
    close_inherited_file_descriptors()?;
    clear_ambient_capabilities()?;
    if let Some(identity) = identity {
        drop_privileges(identity)?;
    }
    clear_capabilities()?;
    set_no_new_privileges()?;
    deny_process_inspection()?;
    apply_landlock(&check_dir, &work_dir)?;

    let error = StdCommand::new(program).args(args).exec();
    Err(error).context("executing Check command")
}

fn parse_id(value: Option<OsString>, name: &str) -> Result<u32> {
    value
        .ok_or_else(|| anyhow!("missing Check {name}"))?
        .to_string_lossy()
        .parse()
        .with_context(|| format!("invalid Check {name}"))
}

fn close_inherited_file_descriptors() -> Result<()> {
    let result = unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 0_u32) };
    if result == -1 {
        return Err(std::io::Error::last_os_error()).context("closing inherited file descriptors");
    }
    Ok(())
}

fn drop_privileges(identity: CheckIdentity) -> Result<()> {
    if unsafe { libc::setgroups(0, std::ptr::null()) } == -1 {
        return Err(std::io::Error::last_os_error()).context("clearing supplementary groups");
    }
    if unsafe { libc::setresgid(identity.gid, identity.gid, identity.gid) } == -1 {
        return Err(std::io::Error::last_os_error()).context("changing Check GID");
    }
    if unsafe { libc::setresuid(identity.uid, identity.uid, identity.uid) } == -1 {
        return Err(std::io::Error::last_os_error()).context("changing Check UID");
    }

    Ok(())
}

fn clear_ambient_capabilities() -> Result<()> {
    if unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error()).context("clearing ambient capabilities");
    }
    Ok(())
}

fn clear_capabilities() -> Result<()> {
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
    let header = CapHeader {
        version: 0x20080522,
        pid: 0,
    };
    let data = [CapData::default(); 2];
    if unsafe { libc::syscall(libc::SYS_capset, &header, data.as_ptr()) } == -1 {
        return Err(std::io::Error::last_os_error()).context("clearing Check capabilities");
    }
    Ok(())
}

fn set_no_new_privileges() -> Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } == -1 {
        return Err(std::io::Error::last_os_error()).context("setting no_new_privs");
    }
    Ok(())
}

fn deny_process_inspection() -> Result<()> {
    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_JMP_JSET_K: u16 = 0x45;
    const BPF_RET_K: u16 = 0x06;
    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

    let mut filter = Vec::with_capacity(6 + 2 * 4 + 1);
    filter.push(libc::sock_filter {
        code: BPF_LD_W_ABS,
        jt: 0,
        jf: 0,
        k: 4,
    });
    filter.push(libc::sock_filter {
        code: BPF_JMP_JEQ_K,
        jt: 1,
        jf: 0,
        k: SECCOMP_AUDIT_ARCH,
    });
    filter.push(libc::sock_filter {
        code: BPF_RET_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_KILL_PROCESS,
    });
    filter.push(libc::sock_filter {
        code: BPF_LD_W_ABS,
        jt: 0,
        jf: 0,
        k: 0,
    });
    filter.push(libc::sock_filter {
        code: BPF_JMP_JSET_K,
        jt: 0,
        jf: 1,
        k: 0x4000_0000,
    });
    filter.push(libc::sock_filter {
        code: BPF_RET_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_KILL_PROCESS,
    });
    // Keep process-memory and descriptor inspection unavailable across every Check.
    for syscall in [
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_getfd,
    ] {
        filter.push(libc::sock_filter {
            code: BPF_JMP_JEQ_K,
            jt: 0,
            jf: 1,
            k: syscall as u32,
        });
        filter.push(libc::sock_filter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ERRNO | libc::EPERM as u32,
        });
    }
    filter.push(libc::sock_filter {
        code: BPF_RET_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    });
    let mut program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    if unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &mut program,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error())
            .context("blocking cross-process inspection syscalls");
    }
    Ok(())
}

fn apply_landlock(check_dir: &Path, work_dir: &Path) -> Result<()> {
    let read_access = AccessFs::ReadFile | AccessFs::ReadDir | AccessFs::Execute;
    let write_access = AccessFs::WriteFile
        | AccessFs::RemoveDir
        | AccessFs::RemoveFile
        | AccessFs::MakeDir
        | AccessFs::MakeReg
        | AccessFs::MakeFifo
        | AccessFs::MakeSym
        | AccessFs::Refer
        | AccessFs::Truncate
        | AccessFs::Execute
        | AccessFs::ReadFile
        | AccessFs::ReadDir;

    let mut ruleset = build_landlock_ruleset()?;
    for path in [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/etc/ssl/certs",
        "/etc/pki/ca-trust",
        "/etc/pki/tls/certs",
        "/etc/ca-certificates",
        "/sys",
    ] {
        add_path_rule(&mut ruleset, Path::new(path), read_access)?;
    }
    add_path_rule(
        &mut ruleset,
        Path::new("/proc/self"),
        AccessFs::ReadFile | AccessFs::ReadDir,
    )?;
    for path in [
        "/proc/cpuinfo",
        "/proc/loadavg",
        "/proc/meminfo",
        "/proc/stat",
        "/proc/uptime",
    ] {
        add_path_rule(&mut ruleset, Path::new(path), AccessFs::ReadFile)?;
    }
    for path in [
        "/etc/hosts",
        "/etc/resolv.conf",
        "/etc/nsswitch.conf",
        "/etc/passwd",
        "/etc/group",
        "/etc/ld.so.cache",
        "/etc/localtime",
        "/etc/mime.types",
        "/etc/os-release",
    ] {
        add_path_rule(&mut ruleset, Path::new(path), AccessFs::ReadFile)?;
    }
    for path in ["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"] {
        add_path_rule(
            &mut ruleset,
            Path::new(path),
            AccessFs::ReadFile | AccessFs::WriteFile,
        )?;
    }
    add_path_rule(&mut ruleset, check_dir, read_access)?;
    add_path_rule(&mut ruleset, work_dir, write_access)?;
    ruleset
        .restrict_self()
        .context("applying Check Landlock restrictions")?;
    Ok(())
}

fn add_path_rule(
    ruleset: &mut landlock::RulesetCreated,
    path: &Path,
    access: impl Into<landlock::BitFlags<AccessFs>>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let path_fd =
        PathFd::new(path).with_context(|| format!("opening Landlock path {}", path.display()))?;
    let _ = ruleset
        .add_rule(PathBeneath::new(path_fd, access))
        .with_context(|| format!("adding Landlock rule for {}", path.display()))?;
    Ok(())
}

fn lock_legacy_workspace(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "legacy Check workspace {} is not a directory",
            path.display()
        );
    }
    if metadata.uid() != unsafe { libc::geteuid() } {
        bail!(
            "legacy Check workspace {} is owned by UID {}, expected UID {}; repair its ownership before restarting",
            path.display(),
            metadata.uid(),
            unsafe { libc::geteuid() }
        );
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting legacy workspace {}", path.display()))?;
    Ok(())
}

fn prepare_work_directory(path: &Path, check_uid: u32) -> Result<()> {
    create_owned_directory(path, 0o700)?;
    set_named_user_acl(path, check_uid, 0b111, 0b111)
}

fn create_owned_directory(path: &Path, mode: u32) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!("Check workspace {} is not a directory", path.display());
            }
            if metadata.uid() != unsafe { libc::geteuid() } {
                bail!(
                    "Check workspace {} is owned by UID {}, expected UID {}; repair its ownership before restarting",
                    path.display(),
                    metadata.uid(),
                    unsafe { libc::geteuid() }
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .with_context(|| format!("creating Check workspace {}", path.display()))?;
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting permissions on {}", path.display()))?;
    Ok(())
}

fn validate_owned_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading Check directory {}", path.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("Check directory {} is not a directory", path.display());
    }
    if metadata.uid() != unsafe { libc::geteuid() } {
        bail!(
            "Check directory {} is owned by UID {}, expected UID {}",
            path.display(),
            metadata.uid(),
            unsafe { libc::geteuid() }
        );
    }
    Ok(())
}

fn set_named_user_acl(path: &Path, check_uid: u32, owner: u16, user: u16) -> Result<()> {
    const ACL_USER_OBJ: u16 = 0x01;
    const ACL_USER: u16 = 0x02;
    const ACL_GROUP_OBJ: u16 = 0x04;
    const ACL_MASK: u16 = 0x10;
    const ACL_OTHER: u16 = 0x20;
    const ACL_UNDEFINED_ID: u32 = u32::MAX;

    let mut acl = Vec::with_capacity(4 + 5 * 8);
    acl.extend_from_slice(&2_u32.to_le_bytes());
    for (tag, permissions, id) in [
        (ACL_USER_OBJ, owner, ACL_UNDEFINED_ID),
        (ACL_USER, user, check_uid),
        (ACL_GROUP_OBJ, 0_u16, ACL_UNDEFINED_ID),
        (ACL_MASK, user, ACL_UNDEFINED_ID),
        (ACL_OTHER, 0_u16, ACL_UNDEFINED_ID),
    ] {
        acl.extend_from_slice(&tag.to_le_bytes());
        acl.extend_from_slice(&permissions.to_le_bytes());
        acl.extend_from_slice(&id.to_le_bytes());
    }

    let path_cstr = CString::new(path.as_os_str().as_bytes())
        .with_context(|| format!("encoding Check workspace path {}", path.display()))?;
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
            .with_context(|| format!("setting Check workspace ACL on {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt, time::SystemTime};

    use super::*;

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn check_workspace_acl_limits_access_to_the_named_user() {
        let path = temp_path("pinglow-check-acl");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();

        set_named_user_acl(&path, 1_500_000_000, 0b111, 0b001).unwrap();

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
        assert_eq!(u16::from_le_bytes(acl[6..8].try_into().unwrap()), 0b111);
        assert_eq!(u16::from_le_bytes(acl[14..16].try_into().unwrap()), 0b001);
        assert_eq!(
            u32::from_le_bytes(acl[16..20].try_into().unwrap()),
            1_500_000_000
        );
        assert_eq!(u16::from_le_bytes(acl[22..24].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(acl[30..32].try_into().unwrap()), 0b001);
        assert_eq!(u16::from_le_bytes(acl[38..40].try_into().unwrap()), 0);

        let script = path.join("script.py");
        fs::write(&script, "pass\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o640)).unwrap();
        set_named_user_acl(&script, 1_500_000_000, 0b110, 0b100).unwrap();
        assert_eq!(
            fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o640
        );

        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn legacy_workspaces_are_locked_without_reusing_them() {
        let path = temp_path("pinglow-legacy-workspace");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o770)).unwrap();

        lock_legacy_workspace(&path).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn landlock_v6_is_available_and_enforced() {
        validate_landlock_support().unwrap();
    }
}
