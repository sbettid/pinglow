use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Local, Utc};
use dashmap::DashMap;
use env_logger::{self, Builder};
use log::{error, info};
use pinglow::check::{Check, CheckResult};
use pinglow::load_single_runnable_check;
use tokio::signal::unix::signal;
use tokio::sync::mpsc::Sender;
use tokio::{
    signal::unix::SignalKind,
    sync::{mpsc, RwLock},
};

use kube::{Api, Client};
use tokio_postgres::NoTls;

use pinglow::api::start_rocket;
use pinglow::check::{CheckResultStatus, SharedRunnableChecks};
use pinglow::controller::watch_resources;
use pinglow::runner::RunnableCheckEvent;
use pinglow::{
    check::SharedChecks,
    config::{get_config_from_env, PinglowConfig},
    error::ReconcileError,
    runner::scheduler_loop,
};

mod embedded {
    use refinery::embed_migrations;
    embed_migrations!("./db_migrations");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the logger
    Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Get the configuration
    let config = get_config_from_env();

    // Connect to the DB
    let (mut client, connection) = tokio_postgres::connect(
        &format!(
            "host={} user={} password={} dbname={}",
            config.db_host, config.db_user, config.db_user_password, config.db
        ),
        NoTls,
    )
    .await?;

    // The connection object performs the actual communication with the database,
    // so spawn it off to run on its own.
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            error!("Error when connecting to TimescaleDB: {e}");
        }
    });

    // Apply migrations
    embedded::migrations::runner()
        .run_async(&mut client)
        .await?;

    let client_arc = Arc::new(client);

    // Hashmap that holds the checks currently loaded
    let shared_checks: SharedRunnableChecks = Arc::new(RwLock::new(HashMap::new()));

    let shared_original_checks: SharedChecks = Arc::new(DashMap::new());

    // Channels to communicate checks update events and result of checks
    let (event_tx, event_rx) = mpsc::channel::<RunnableCheckEvent>(100);
    let (result_tx, mut result_rx) = mpsc::channel::<CheckResult>(100);

    // Load all the available checks
    load_checks(&config, event_tx.clone(), shared_original_checks.clone()).await?;

    // Thread to watch for
    //tokio::spawn(watch_checks(config.clone(), event_tx.clone()));
    tokio::spawn(watch_resources(
        config.clone(),
        event_tx,
        shared_original_checks,
    ));
    // Spawn the task which will schedule the checks in a continuous way
    let scheduler_handle = tokio::spawn(scheduler_loop(
        event_rx,
        result_tx,
        shared_checks.clone(),
        config.target_namespace.clone(),
    ));

    // Spawn the task to host Rocket to handle API requests
    let (rocket, rocket_shutdown) =
        start_rocket(config, shared_checks.clone(), client_arc.clone()).await?;
    let rocket_handle = tokio::spawn(async move {
        rocket.launch().await?;
        Ok::<(), rocket::Error>(())
    });

    // Wait for the results and log them
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    let http_client = reqwest::Client::new();

    loop {
        tokio::select! {
            Some(result) = result_rx.recv() => {
                // Write result to DB
                result.write_to_db(client_arc.clone()).await?;

                // Send result to telegram channels
                if result.status != CheckResultStatus::Ok &&
                match result.mute_notifications {
                    Some(true) => {
                        match result.mute_notifications_until {
                            Some(until) => until <= Utc::now(), // check if mute until is still valid
                            None => false,                      // muted forever: don't send
                        }
                    }
                    _ => true, // if mute_notifications is None or false we send the notification
                }

                {

                    for channel in result.telegram_channels.iter() {

                    let url = format!("https://api.telegram.org/bot{}/sendMessage", channel.bot_token);
                    let timestamp_local = result.timestamp.unwrap().with_timezone(&Local);

                    match  http_client.post(&url).form(&[
                        ("chat_id", channel.chat_id.clone()),
                        ("text", format!("<b>Date</b>: {0}\n<b>Check name</b>: {1} \n<b>Status</b>: {2:?}\n<b>Output</b>\n<pre>{3}</pre>", timestamp_local.format("%Y-%m-%d %H:%M:%S %Z"), result.check_name, result.status, result.get_output())),
                        ("parse_mode", "HTML".to_string()),
                    ]).send().await {
                        Ok(_) => {},
                        Err(e) => error!("Error when sending check result to Telegram channel: {e}"),
                    }
                }


                }

            }
            // In case we receive a sigterm we exit to teardown our jobs in a clean way (especially rocket)
            _ = sigint.recv() => {
                break
            }
            _ = sigterm.recv() => {
                break;
            }

        }
    }

    info!("Shutting down...");
    rocket_shutdown.notify();

    scheduler_handle.abort();
    let _ = rocket_handle.await?;

    Ok(())
}

/**
 * This function is used to load all the checks from the CR of the pinglow namespace
 */
async fn load_checks(
    config: &PinglowConfig,
    event_rx: Sender<RunnableCheckEvent>,
    shared_checks: SharedChecks,
) -> Result<(), ReconcileError> {
    // Create the kube client
    let client = Client::try_default().await?;

    // Get all checks and scripts from the target namespace
    let checks: Api<Check> = Api::namespaced(client.clone(), &config.target_namespace);

    let check_list = checks.list(&Default::default()).await?;

    for check in check_list.iter() {
        let check_name =
            check
                .metadata
                .name
                .as_ref()
                .ok_or(ReconcileError::PropertyExtractionError(
                    "Cannot extract check name".to_string(),
                ))?;

        // TODO: avoid cloning here
        shared_checks.insert(check_name.to_owned(), Arc::new(check.clone()));

        let runnable_check = load_single_runnable_check(check, &client, config).await?;

        event_rx
            .send(RunnableCheckEvent::AddOrUpdate(Arc::new(runnable_check)))
            .await
            .ok();
    }

    info!("Loaded {:?} check(s)", check_list.items.len());

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    #[test]
    fn check_formatting() {
        let status = Command::new("cargo")
            .args(["fmt", "--all", "--", "--check"])
            .status()
            .expect("failed to run cargo fmt");

        assert!(status.success(), "Code is not properly formatted");
    }
}
