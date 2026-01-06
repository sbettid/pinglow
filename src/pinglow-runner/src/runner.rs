use std::time::Duration;

use log::{debug, error, info};
use pinglow_common::{
    error::SerializeError,
    redis::{init_streams, redis_client},
};
use redis::AsyncConnectionConfig;
use tokio_util::sync::CancellationToken;

use crate::{config::get_config_from_env, executor::execute_check, queue::fetch_task};

pub async fn run() -> anyhow::Result<()> {
    let redis_client = redis_client()?;

    // Init streams (short-lived connection)
    {
        let mut conn = redis_client.get_multiplexed_async_connection().await?;
        init_streams(&mut conn).await;
    } // conn dropped here

    let runner_config = get_config_from_env();

    let shutdown = CancellationToken::new();
    let shutdown_signal = shutdown.clone();

    // Ctrl-C listener
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown_signal.cancel();
    });

    let mut async_connection = AsyncConnectionConfig::new();
    async_connection = async_connection.set_connection_timeout(Some(Duration::from_secs(30)));
    async_connection = async_connection.set_response_timeout(Some(Duration::from_secs(30)));

    info!("Runner started");

    loop {
        if shutdown.is_cancelled() {
            info!("Shutdown signal received, exiting...");
            break;
        }

        let mut redis_conn = redis_client
            .get_multiplexed_async_connection_with_config(&async_connection)
            .await?;

        let base_path = runner_config.checks_base_path.clone();
        let namespace = runner_config.target_namespace.clone();
        let connection_config = async_connection.clone();

        match fetch_task(&mut redis_conn, &runner_config.runner_name).await {
            Ok(Some((id, check))) => {
                debug!("Received check to execute");
                let redis_client = redis_client.clone();
                tokio::spawn(async move {
                    // Execute check
                    let result = match execute_check(check, &base_path, &namespace).await {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Error executing check: {e}");
                            return;
                        }
                    };

                    let mut redis_conn = match redis_client
                        .get_multiplexed_async_connection_with_config(&connection_config)
                        .await
                    {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Error getting connection to redis: {e}");
                            return;
                        }
                    };

                    // Ack in redis
                    if let Err(e) = redis::cmd("XACK")
                        .arg("pinglow:checks")
                        .arg("workers")
                        .arg(id)
                        .query_async::<()>(&mut redis_conn)
                        .await
                    {
                        error!("Error sending ack to redis for check: {e}");
                    }

                    let payload = match serde_json::to_string(&result).map_err(|e| {
                        SerializeError::SerializationError(format!("Error serializing check: {e}"))
                    }) {
                        Ok(p) => p,
                        Err(e) => {
                            error!("Error serializing check result: {e}");
                            return;
                        }
                    };

                    // Send back the result
                    debug!("Sending back the result");
                    if let Err(e) = redis::cmd("XADD")
                        .arg("pinglow:results")
                        .arg("*")
                        .arg("payload")
                        .arg(payload)
                        .query_async::<()>(&mut redis_conn)
                        .await
                    {
                        error!("Error sending check result to redis: {e}");
                    }
                });
            }
            Ok(None) => {
                // No task, sleep a bit to avoid busy loop
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
            Err(e) => {
                if e.to_string().contains("timed out") {
                    // Not really an error, just no message yet
                    debug!("No messages yet, continuing to wait...");
                } else {
                    error!("Error waiting for task: {e}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
        }
    }
    info!("Runner stopped successfully");
    Ok(())
}
