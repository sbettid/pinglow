use std::time::Duration;

use check::{Check, CheckResult, CheckResultStatus};
use chrono::Local;
use tokio::{process::Command, sync::mpsc, time::{sleep_until, Instant}};

mod check;


async fn run_check_loop(check: Check, mut result_tx: mpsc::Sender<CheckResult>) {
    let mut next_run = Instant::now();

    loop {
        sleep_until(next_run).await;

        let output = Command::new("sh")
            .arg("-c")
            .arg(&check.command)
            .output()
            .await;

        let (success, stdout) = match output {
            Ok(out) => (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).to_string(),
            ),
            Err(e) => (false, format!("Error: {e}")),
        };

        let timestamp = Local::now().to_rfc3339();
        let mut check_result = CheckResultStatus::SUCCESS;

        if !success {
            let alert_msg = format!(
                "🚨 *Check Failed*\n*{}* at `{}`\n\n```\n{}\n```",
                check.check_name, timestamp, stdout.trim()
            );
            check_result = CheckResultStatus::FAILURE;
            //send_telegram_alert(&bot_token, &chat_id, &alert_msg).await;
        }

        let result = CheckResult {
            check_name: check.check_name.clone(),
            output: stdout,
            status: check_result,
            timestamp,
        };

        result_tx.send(result).await.unwrap();

        next_run += Duration::from_secs(check.check_interval);
    }
}

#[tokio::main]
async fn main() {
    let (result_tx, mut result_rx) = mpsc::channel::<CheckResult>(100);

    // let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").expect("Bot token not set");
    // let chat_id = std::env::var("TELEGRAM_CHAT_ID").expect("Chat ID not set");

    let checks = vec![
        Check { check_name: "test1".to_string(), command: "/bin/true".to_string(), check_interval: 10 },
        Check { check_name: "test2".to_string(), command: "/bin/false".to_string(), check_interval: 5 }
    ];

    for check in checks {
        let tx = result_tx.clone();
        // let bot_token = bot_token.clone();
        // let chat_id = chat_id.clone();

        tokio::spawn(run_check_loop(check, tx));
    }

    while let Some(result) = result_rx.recv().await {
        println!(
            "[Result] {} @ {}: {:?}",
            result.check_name,
            result.timestamp,
            result.status
        );
    }
}