use std::time::Duration;

use log::{debug, error, info};
use pinglow_common::{
    error::SerializeError,
    redis::{init_streams, redis_client},
};
use tokio::select;

use crate::{config::get_config_from_env, executor::execute_check, queue::fetch_task};

pub async fn run() -> anyhow::Result<()> {
    let redis_client = redis_client()?;

    // Init streams (short-lived connection)
    {
        let mut conn = redis_client.get_multiplexed_async_connection().await?;
        init_streams(&mut conn).await;
    } // conn dropped here

    let runner_config = get_config_from_env();

    info!("Runner started");

    loop {
        let mut redis_conn = redis_client.get_multiplexed_async_connection().await?;
        redis_conn.set_response_timeout(Duration::MAX);
        let base_path = runner_config.checks_base_path.clone();
        let namespace = runner_config.target_namespace.clone();
        select! {
            // Handle shutdown signal
            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received, exiting...");
                break;
            }

            maybe_check = fetch_task(&mut redis_conn, &runner_config.runner_name) => {
                match maybe_check {
                    Ok(Some((id, check))) => {

                        tokio::spawn(async move {

                            // Execute check
                            let result = match execute_check(check, &base_path, &namespace).await {
                                Ok(r) => r,
                                Err(e) => {
                                    error!("Error executing check: {e}");
                                    return;
                                },
                            };

                            // Ack in redis
                            if let Err(e) = redis::cmd("XACK")
                            .arg("pinglow:checks")
                            .arg("workers")
                            .arg(id)
                            .query_async::<()>(&mut redis_conn)
                            .await {
                                error!("Error sending ack to redis for check: {e}");
                            }

                            let payload = match serde_json::to_string(&result)
                                .map_err(|e| SerializeError::SerializationError(format!("Error serializing check: {e}"))) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        error!("Error serializing check result: {e}");
                                        return;
                                    },
                                };

                            // Send back the result
                            if let Err(e) = redis::cmd("XADD")
                                .arg("pinglow:results")
                                .arg("*")
                                .arg("payload")
                                .arg(payload)
                                .query_async::<()>(&mut redis_conn)
                                .await {
                                    error!("Error sending check result to redis: {e}");

                                }
                        });
                    }
                    Ok(None) => {
                        // No task, sleep a bit to avoid busy loop
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    Err(e) => {
                        if e.to_string().contains("timed out") {
                            // Not really an error, just no message yet
                            debug!("No messages yet, continuing to wait...");
                        } else {
                            error!("Error waiting for task: {e}");
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
    info!("Runner stopped successfully");
    Ok(())
}
