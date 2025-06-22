use chrono::Local;
use std::{collections::BinaryHeap, time::Duration};

use check::{map_command_exit_code_to_check_result, CheckResult};

use tokio::{
    process::Command,
    sync::mpsc,
    time::{sleep_until, Instant},
};

use crate::check::{self, RunnableCheck, ScheduledCheck};

/**
 * This function continuously schedule checks based on the interval
 */
pub async fn scheduler_loop(checks: Vec<RunnableCheck>, result_tx: mpsc::Sender<CheckResult>) {
    // Create the queue of checks
    let mut queue = BinaryHeap::new();

    let now = Instant::now();

    // Add all checks in the queue. The queue will be automatically ordered by next run
    for check in checks {
        queue.push(ScheduledCheck {
            next_run: now + Duration::from_secs(check.interval),
            check,
        });
    }

    // Continuosly loop
    loop {
        if let Some(mut scheduled) = queue.pop() {
            let now = Instant::now();
            // If the next check must not yet be run just sleep
            if scheduled.next_run > now {
                sleep_until(scheduled.next_run).await;
            }

            let check_interval = Duration::from_secs(scheduled.check.interval);

            // Run the check asynchronously
            let tx = result_tx.clone();
            tokio::spawn(run_check(scheduled.check.clone(), tx));

            // Schedule the next run
            scheduled.next_run += check_interval;
            queue.push(scheduled);
        } else {
            // If no checks are scheduled, just sleep briefly to prevent tight loop
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/**
 * This function runs a check, parses the result to a check result and returns it to the main thread
 */
pub async fn run_check(check: RunnableCheck, result_tx: mpsc::Sender<CheckResult>) {
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
}
