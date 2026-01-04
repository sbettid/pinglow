use std::{sync::Arc, time::Duration};

use anyhow::Error;
use log::{debug, error};
use pinglow_common::redis::parse_stream_payload;
use pinglow_common::CheckResult;
use redis::Client as RedisClient;
use redis::{aio::MultiplexedConnection, AsyncConnectionConfig};
use tokio::signal::unix::{signal, SignalKind};
use tokio_postgres::Client;

use crate::process_check_result;

pub async fn run(redis_client: RedisClient, postgres_client: Arc<Client>) -> Result<(), Error> {
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    let http_client = reqwest::Client::new();

    let mut async_connection = AsyncConnectionConfig::new();
    async_connection = async_connection.set_connection_timeout(Some(Duration::from_secs(30)));
    async_connection = async_connection.set_response_timeout(Some(Duration::from_secs(30)));

    loop {
        let mut redis_conn = redis_client
            .get_multiplexed_async_connection_with_config(&async_connection)
            .await
            .expect("Cannot get connection to redis");

        tokio::select! {
        _ = sigint.recv() => {
        }
        _ = sigterm.recv() => {
        }

        res = wait_for_result(&mut redis_conn) => {
            match res {
                Ok(Some((id, result))) => {
                    // Process the result
                    process_check_result(result, &postgres_client, &http_client).await?;

                    // Ack in redis
                    redis::cmd("XACK")
                        .arg("pinglow:results")
                        .arg("controller")
                        .arg(id)
                        .query_async::<()>(&mut redis_conn)
                        .await?;
                },
                Ok(None) => {
                    // No task, sleep a bit to avoid busy loop
                     tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                },
                Err(e) => {
                    if e.to_string().contains("timed out") {
                        // Not really an error, just no message yet
                        debug!("No messages yet, continuing to wait...");
                    } else {
                        error!("Error waiting for result: {e}");
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                }
            }

        }
        }
    }
}

async fn wait_for_result(
    conn: &mut MultiplexedConnection,
) -> Result<Option<(String, CheckResult)>, Error> {
    let value: Option<redis::Value> = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg("controller")
        .arg("controller-1") // consumer name
        .arg("BLOCK")
        .arg(15000)
        .arg("COUNT")
        .arg(1) // fetch one message at a time
        .arg("STREAMS")
        .arg("pinglow:results")
        .arg(">")
        .query_async(conn)
        .await?;

    let Some(value) = value else { return Ok(None) };

    let (id, fields) = parse_stream_payload(value).ok_or(
        pinglow_common::error::SerializeError::DeserializationError(
            "Cannot extract id and fields from redis message".into(),
        ),
    )?;

    let payload = fields.get("payload").ok_or(
        pinglow_common::error::SerializeError::DeserializationError(
            "The expected payload field was not found".into(),
        ),
    )?;

    let result: CheckResult = serde_json::from_str(payload)?;

    Ok(Some((id, result)))
}
