use anyhow::Error;
use log::debug;
use log::error;
use log::info;
use pinglow_common::{PinglowCheck, ScheduledCheck};
use redis::Client as RedisClient;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;

use tokio::{sync::mpsc, time::Instant};

use crate::check::SharedPinglowChecks;
use pinglow_common::error::SerializeError;

pub enum RunnableCheckEvent {
    AddOrUpdate(Arc<PinglowCheck>),
    Remove(String), // check_name
}

/**
 * This function handles the addition/update and removal of checks when an events on the Kube side occurs
 */
async fn handle_check_event(
    event: RunnableCheckEvent,
    queue: &mut BTreeMap<Instant, ScheduledCheck>,
    shared_checks: SharedPinglowChecks,
) {
    match event {
        RunnableCheckEvent::AddOrUpdate(check) => {
            let check_name = check.check_name.clone();

            shared_checks
                .write()
                .await
                .insert(check_name.clone(), check.clone());

            // Skip putting in queue passive checks
            if check.passive {
                return;
            }

            // Skip check where check interval is node defined (should not happen though)
            let interval = if let Some(interval) = check.interval {
                interval
            } else {
                return;
            };

            // Update the scheduled check
            let removed: Option<ScheduledCheck> = queue
                .extract_if(.., |_, sc| sc.check.check_name == check_name)
                .map(|(_, sc)| sc)
                .next();

            let next_run = if let Some(removed) = removed {
                removed.next_run
            } else {
                Instant::now() + Duration::from_secs(interval)
            };

            queue.insert(next_run, ScheduledCheck { next_run, check });
        }
        RunnableCheckEvent::Remove(check_name) => {
            shared_checks.write().await.remove(&check_name);
            queue.retain(|_i, scheduled_check| scheduled_check.check.check_name != check_name);
        }
    }
}

/**
 * This function continuously schedule checks based on the interval
 */
pub async fn scheduler_loop(
    mut event_rx: mpsc::Receiver<RunnableCheckEvent>,
    shared_checks: SharedPinglowChecks,
    redis_client: RedisClient,
) {
    let mut queue: BTreeMap<Instant, ScheduledCheck> = BTreeMap::new();

    info!("Starting checks scheduling");

    // Continuosly loop
    loop {
        // Check if there's a scheduled task
        if let Some((_check_instant, mut scheduled_check)) =
            queue.iter().next().map(|(k, v)| (*k, v.clone()))
        {
            debug!("Next check is {scheduled_check:?}");

            let now = Instant::now();
            let delay = scheduled_check.next_run.saturating_duration_since(now);

            select! {
                maybe_event = event_rx.recv() => {
                    if let Some(event) = maybe_event {
                        handle_check_event(event, &mut queue, shared_checks.clone()).await
                    }
                }
                _ = tokio::time::sleep(delay) => {
                    // Check still valid? (not removed)
                    if !shared_checks
                        .read()
                        .await
                        .contains_key(&scheduled_check.check.check_name)
                    {
                        continue; // Skip deleted check
                    }

                    // Skip checks if interval is not defined
                    let check_interval = if let Some(interval) = scheduled_check.check.interval {
                        Duration::from_secs(interval)
                    } else {
                        continue;
                    };

                    // Remove the check since it is being executed
                    queue.retain(|_i, check_in_queue| check_in_queue.check.check_name != scheduled_check.check.check_name);

                    let mut redis_conn = redis_client
                    .get_multiplexed_async_connection()
                    .await
                    .expect("Cannot get connection to redis");

                    redis_conn.set_response_timeout(Duration::from_secs(30));

                    // Send the task in the queue
                    if let Err(e) = enqueue_check(&mut redis_conn, &scheduled_check.check).await {
                        error!("Error sending check to execution queue: {e}")
                    }

                    // Schedule the next run
                    scheduled_check.next_run += check_interval;
                    queue.insert(scheduled_check.next_run, scheduled_check);
                }
            }
        } else {
            // No scheduled checks, wait for events
            if let Some(event) = event_rx.recv().await {
                handle_check_event(event, &mut queue, shared_checks.clone()).await
            }
        }
    }
}

pub async fn enqueue_check(
    conn: &mut redis::aio::MultiplexedConnection,
    check: &Arc<PinglowCheck>,
) -> Result<String, Error> {
    let payload = serde_json::to_string(check.as_ref())
        .map_err(|e| SerializeError::SerializationError(format!("Error serializing check: {e}")))?;

    // XADD pinglow:tasks * payload "<json>"
    let id: String = redis::cmd("XADD")
        .arg("pinglow:checks")
        .arg("*")
        .arg("payload")
        .arg(payload)
        .query_async(conn)
        .await?;

    Ok(id)
}
