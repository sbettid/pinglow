use anyhow::Error;
use kube::ResourceExt;
use log::debug;
use log::error;
use log::info;
use pinglow_common::PinglowCheck;
use redis::Client as RedisClient;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;

use tokio::{sync::mpsc, time::Instant};

use crate::{
    check::{Check, SharedPinglowChecks},
    config::PinglowConfig,
    load_single_runnable_check,
};
use pinglow_common::error::SerializeError;

#[derive(Clone, Debug)]
struct ScheduledCheck {
    definition: Arc<Check>,
    next_run: Instant,
}

pub enum RunnableCheckEvent {
    AddOrUpdate {
        definition: Arc<Check>,
        runnable: Option<Arc<PinglowCheck>>,
    },
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
        RunnableCheckEvent::AddOrUpdate {
            definition,
            runnable,
        } => {
            let check_name = definition.name_any();
            if let Some(runnable) = runnable {
                shared_checks
                    .write()
                    .await
                    .insert(check_name.clone(), runnable);
            }

            let removed: Option<ScheduledCheck> = queue
                .extract_if(.., |_, scheduled| {
                    scheduled.definition.name_any() == check_name
                })
                .map(|(_, scheduled)| scheduled)
                .next();

            // Skip putting in queue passive checks
            if definition.spec.passive {
                return;
            }

            let interval = if let Some(interval) = definition.spec.interval {
                interval
            } else {
                return;
            };

            let next_run = if let Some(removed) = removed {
                removed.next_run
            } else {
                Instant::now() + Duration::from_secs(interval)
            };

            queue.insert(
                next_run,
                ScheduledCheck {
                    definition,
                    next_run,
                },
            );
        }
        RunnableCheckEvent::Remove(check_name) => {
            shared_checks.write().await.remove(&check_name);
            queue.retain(|_i, scheduled_check| scheduled_check.definition.name_any() != check_name);
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
    redis_stream_max_len: usize,
    client: kube::Client,
    config: PinglowConfig,
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
                    let check_name = scheduled_check.definition.name_any();
                    if !shared_checks
                        .read()
                        .await
                        .contains_key(&check_name)
                    {
                        continue; // Skip deleted check
                    }

                    // Skip checks if interval is not defined
                    let check_interval = if let Some(interval) = scheduled_check.definition.spec.interval {
                        Duration::from_secs(interval)
                    } else {
                        continue;
                    };

                    // Remove the check since it is being executed
                    queue.retain(|_i, check_in_queue| {
                        check_in_queue.definition.name_any() != check_name
                    });

                    match load_single_runnable_check(&scheduled_check.definition, &client, &config).await {
                        Ok(runnable) => {
                            let runnable = Arc::new(runnable);
                            shared_checks.write().await.insert(check_name.clone(), runnable.clone());

                            match redis_client.get_multiplexed_async_connection().await {
                                Ok(mut redis_conn) => {
                                    redis_conn.set_response_timeout(Duration::from_secs(30));

                                    if let Err(e) = enqueue_check(&mut redis_conn, &runnable, redis_stream_max_len).await {
                                        error!("Error sending check {check_name} to execution queue: {e}");
                                    }
                                }
                                Err(e) => error!("Error connecting to Redis to enqueue check {check_name}: {e}"),
                            }
                        }
                        Err(e) => error!("Error resolving check {check_name} before execution: {e}"),
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
    redis_stream_max_len: usize,
) -> Result<String, Error> {
    let payload = serde_json::to_string(check.as_ref())
        .map_err(|e| SerializeError::SerializationError(format!("Error serializing check: {e}")))?;

    // XADD pinglow:tasks * payload "<json>"
    let id: String = redis::cmd("XADD")
        .arg("pinglow:checks")
        .arg("MAXLEN")
        .arg("~")
        .arg(redis_stream_max_len)
        .arg("*")
        .arg("payload")
        .arg(payload)
        .query_async(conn)
        .await?;

    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::CheckSpec;
    use std::collections::HashMap;
    use tokio::sync::RwLock;

    fn definition(name: &str, interval: Option<u64>, passive: bool) -> Arc<Check> {
        Arc::new(Check::new(
            name,
            CheckSpec {
                scriptRef: None,
                interval,
                secretRefs: None,
                telegramChannelRefs: None,
                muteNotifications: None,
                muteNotificationsUntil: None,
                passive,
            },
        ))
    }

    fn runnable(name: &str) -> Arc<PinglowCheck> {
        Arc::new(PinglowCheck {
            passive: false,
            script: None,
            interval: Some(60),
            check_name: name.to_owned(),
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        })
    }

    fn shared_checks() -> SharedPinglowChecks {
        Arc::new(RwLock::new(HashMap::new()))
    }

    #[tokio::test]
    async fn update_replaces_definition_and_preserves_schedule_time() {
        let mut queue = BTreeMap::new();
        let shared_checks = shared_checks();
        let initial = definition("check-a", Some(60), false);

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: initial,
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks.clone(),
        )
        .await;

        let original_run = queue.values().next().unwrap().next_run;
        let updated = definition("check-a", Some(120), false);

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: updated,
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks,
        )
        .await;

        assert_eq!(queue.len(), 1);
        let scheduled = queue.values().next().unwrap();
        assert_eq!(scheduled.next_run, original_run);
        assert_eq!(scheduled.definition.spec.interval, Some(120));
    }

    #[tokio::test]
    async fn unresolved_update_schedules_latest_definition_without_stale_runnable_update() {
        let mut queue = BTreeMap::new();
        let shared_checks = shared_checks();

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: definition("check-a", Some(60), false),
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks.clone(),
        )
        .await;

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: definition("check-a", Some(30), false),
                runnable: None,
            },
            &mut queue,
            shared_checks.clone(),
        )
        .await;

        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.values().next().unwrap().definition.spec.interval,
            Some(30)
        );
        assert_eq!(
            shared_checks.read().await.get("check-a").unwrap().interval,
            Some(60)
        );
    }

    #[tokio::test]
    async fn passive_update_removes_scheduled_check() {
        let mut queue = BTreeMap::new();
        let shared_checks = shared_checks();

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: definition("check-a", Some(60), false),
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks.clone(),
        )
        .await;

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: definition("check-a", Some(60), true),
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks,
        )
        .await;

        assert!(queue.is_empty());
    }

    #[tokio::test]
    async fn update_without_interval_removes_scheduled_check() {
        let mut queue = BTreeMap::new();
        let shared_checks = shared_checks();

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: definition("check-a", Some(60), false),
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks.clone(),
        )
        .await;

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: definition("check-a", None, false),
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks,
        )
        .await;

        assert!(queue.is_empty());
    }

    #[tokio::test]
    async fn remove_event_clears_shared_and_scheduled_check() {
        let mut queue = BTreeMap::new();
        let shared_checks = shared_checks();

        handle_check_event(
            RunnableCheckEvent::AddOrUpdate {
                definition: definition("check-a", Some(60), false),
                runnable: Some(runnable("check-a")),
            },
            &mut queue,
            shared_checks.clone(),
        )
        .await;

        handle_check_event(
            RunnableCheckEvent::Remove("check-a".to_owned()),
            &mut queue,
            shared_checks.clone(),
        )
        .await;

        assert!(queue.is_empty());
        assert!(shared_checks.read().await.is_empty());
    }
}
