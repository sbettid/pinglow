use chrono::Local;
use std::time::Duration;

use check::{map_command_exit_code_to_check_result, Check, CheckResult};

use tokio::{
    process::Command,
    sync::mpsc,
    time::{sleep_until, Instant},
};

use crate::check::{self, RunnableCheck};

pub async fn run_check_loop(check: RunnableCheck, result_tx: mpsc::Sender<CheckResult>) {
    let mut next_run = Instant::now();

    loop {
        sleep_until(next_run).await;

        let output = Command::new("sh")
            .arg("-c")
            .arg(&check.script)
            .output()
            .await;

        let (exit_code, stdout) = match output {
            Ok(out) => (
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).to_string(),
            ),
            Err(e) => (Some(3), format!("Error: {e}")),
        };

        let timestamp = Local::now().to_rfc3339();
        let check_result = map_command_exit_code_to_check_result(exit_code);

        let result: CheckResult = CheckResult {
            check_name: check.check_name.clone(),
            output: stdout,
            status: check_result,
            timestamp,
        };

        result_tx.send(result).await.unwrap();

        next_run += Duration::from_secs(check.interval);
    }
}
