use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pinglow_common::{CheckResultStatus, PinglowCheck, ScriptSpec};
use pinglow_executor::{execute_check, initialize_check_workspaces, ExecutorConfig};

#[tokio::test]
async fn active_check_runs_with_network_but_without_executor_or_sibling_access() {
    std::env::set_var(
        "PINGLOW_CHECK_LAUNCHER",
        env!("CARGO_BIN_EXE_pinglow-check-launcher"),
    );
    std::env::set_var("PINGLOW_EXECUTOR_TEST_AGENT_SECRET", "agent-secret");

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base_path = std::env::temp_dir().join(format!(
        "pinglow-active-check-{}-{nonce}",
        std::process::id()
    ));
    initialize_check_workspaces(&base_path.to_string_lossy(), false).unwrap();
    let sibling_secret = base_path.join("check-sibling/secret.txt");
    fs::create_dir_all(sibling_secret.parent().unwrap()).unwrap();
    fs::write(&sibling_secret, "sibling-secret").unwrap();

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "Check did not connect");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accepting Check connection: {error}"),
            }
        };
        let mut request = [0; 2];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(b"ok").unwrap();
    });

    let script = format!(
        r#"import ctypes, errno, os, pathlib, socket
assert os.environ.get('CHECK_TOKEN') == 'check-secret'
assert 'PINGLOW_EXECUTOR_TEST_AGENT_SECRET' not in os.environ
assert 'REDIS_PASSWORD' not in os.environ
libc = ctypes.CDLL(None, use_errno=True)
libc.ptrace.argtypes = [ctypes.c_uint, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p]
libc.ptrace.restype = ctypes.c_long
assert libc.ptrace(0x7fffffff, 0, None, None) == -1
assert ctypes.get_errno() == errno.EPERM
if any(line in pathlib.Path('/etc/os-release').read_text().splitlines()
       for line in ('ID=debian', 'ID=ubuntu')):
    assert pathlib.Path('/etc/debian_version').read_text()
work = pathlib.Path(os.environ['HOME'])
(work / 'writable.txt').write_text('check workspace writable')
assert b'CapEff:' in pathlib.Path('/proc/self/status').read_bytes()
for protected_file in ('/etc/passwd', '/etc/group'):
    try:
        pathlib.Path(protected_file).read_text()
        raise AssertionError(f'read {{protected_file}}')
    except PermissionError:
        pass
try:
    pathlib.Path({sibling_secret:?}).read_text()
    raise AssertionError('read sibling check workspace')
except PermissionError:
    pass
try:
    pathlib.Path(f'/proc/{{os.getppid()}}/environ').read_bytes()
    raise AssertionError('read parent process environment')
except PermissionError:
    pass
with socket.create_connection(('127.0.0.1', {port}), timeout=3) as sock:
    sock.sendall(b'ok')
    assert sock.recv(2) == b'ok'
print('active check succeeded')
"#
    );
    let check = PinglowCheck {
        passive: false,
        script: Some(ScriptSpec {
            content: script,
            python_requirements: Some(vec![]),
        }),
        collector: None,
        interval: Some(60),
        check_name: "active-sandbox-test".to_owned(),
        run_on: "cluster".to_owned(),
        expires_at: None,
        group_id: None,
        group_name: None,
        secrets: Some(HashMap::from([(
            "CHECK_TOKEN".to_owned(),
            "check-secret".to_owned(),
        )])),
        telegram_channels: vec![],
        mute_notifications: None,
        mute_notifications_until: None,
    };
    let result = execute_check(
        check,
        &ExecutorConfig {
            base_path: base_path.to_string_lossy().into_owned(),
            drop_privileges: false,
            timeout: Duration::from_secs(30),
        },
    )
    .await;
    std::env::remove_var("PINGLOW_EXECUTOR_TEST_AGENT_SECRET");

    assert_eq!(result.status, CheckResultStatus::Ok, "{}", result.output);
    assert_eq!(result.output, "active check succeeded");
    let server_result = server.join();
    assert!(server_result.is_ok(), "test TCP server failed");
    let writable_file: PathBuf =
        base_path.join("check-active-sandbox-test/work-sandboxed/writable.txt");
    assert_eq!(
        fs::read_to_string(writable_file).unwrap(),
        "check workspace writable"
    );
    fs::remove_dir_all(base_path).unwrap();
}

#[tokio::test]
#[ignore = "requires root CAP_SETUID/CAP_SETGID and the pinglow-check account"]
async fn privileged_check_uses_the_dedicated_uid_without_capabilities() {
    std::env::set_var(
        "PINGLOW_CHECK_LAUNCHER",
        env!("CARGO_BIN_EXE_pinglow-check-launcher"),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base_path = std::env::temp_dir().join(format!(
        "pinglow-privileged-check-{}-{nonce}",
        std::process::id()
    ));
    let parent_uid = unsafe { libc::geteuid() };
    let check = PinglowCheck {
        passive: false,
        script: Some(ScriptSpec {
            content: r#"import os
uid = os.getuid()
assert uid != 0 and uid != int(os.environ['PARENT_UID'])
assert os.getgroups() == []
status = open('/proc/self/status').read()
capabilities = dict(line.split(':', 1) for line in status.splitlines() if line.startswith(('CapEff:', 'CapPrm:', 'CapInh:')))
assert int(capabilities['CapEff'].strip(), 16) == 0
assert int(capabilities['CapPrm'].strip(), 16) == 0
assert int(capabilities['CapInh'].strip(), 16) == 0
try:
    os.setuid(int(os.environ['PARENT_UID']))
    raise AssertionError('regained the executor UID')
except PermissionError:
    pass
print('dedicated Check identity verified')
"#
            .to_owned(),
            python_requirements: Some(vec![]),
        }),
        collector: None,
        interval: Some(60),
        check_name: "privileged-sandbox-test".to_owned(),
        run_on: "cluster".to_owned(),
        expires_at: None,
        group_id: None,
        group_name: None,
        secrets: Some(HashMap::from([(
            "PARENT_UID".to_owned(),
            parent_uid.to_string(),
        )])),
        telegram_channels: vec![],
        mute_notifications: None,
        mute_notifications_until: None,
    };
    let executor_config = ExecutorConfig {
        base_path: base_path.to_string_lossy().into_owned(),
        drop_privileges: true,
        timeout: Duration::from_secs(30),
    };
    for _ in 0..2 {
        let result = execute_check(check.clone(), &executor_config).await;
        assert_eq!(result.status, CheckResultStatus::Ok, "{}", result.output);
    }
    fs::remove_dir_all(base_path).unwrap();
}
