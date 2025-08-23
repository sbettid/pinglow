use std::collections::HashMap;
use std::sync::Arc;

use check::{Check, CheckResult};
use chrono::Local;
use env_logger::{self, Builder};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::runtime::watcher::{watcher, Event};
use log::{error, info};
use rocket::{routes, Rocket, Shutdown};
use tokio::signal::unix::signal;
use tokio::sync::mpsc::Sender;
use tokio::{
    signal::unix::SignalKind,
    sync::{mpsc, RwLock},
};

use kube::{Api, Client};
use tokio_postgres::NoTls;

use crate::api::{get_check_status, get_performance_data};
use crate::check::{CheckResultStatus, ConcreteTelegramChannel, TelegramChannel};
use crate::runner::RunnableCheckEvent;
use crate::{
    api::get_checks,
    check::{RunnableCheck, Script, SharedChecks},
    config::{get_config_from_env, PinglowConfig},
    error::ReconcileError,
    runner::scheduler_loop,
};

mod api;
mod check;
mod config;
mod error;
mod job;
mod runner;

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

    let shared_checks: SharedChecks = Arc::new(RwLock::new(HashMap::new()));

    // Load all the available checks
    let (event_tx, event_rx) = mpsc::channel::<RunnableCheckEvent>(100);
    let (result_tx, mut result_rx) = mpsc::channel::<CheckResult>(100);

    load_checks(&config, event_tx.clone()).await?;

    tokio::spawn(watch_checks(config.clone(), event_tx.clone()));

    // Spawn the task which will schedule the checks in a continuous way
    let scheduler_handle = tokio::spawn(scheduler_loop(
        event_rx,
        result_tx,
        shared_checks.clone(),
        config.target_namespace.clone(),
    ));

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
                if result.status != CheckResultStatus::Ok {

                    for channel in result.telegram_channels.iter() {

                    let url = format!("https://api.telegram.org/bot{}/sendMessage", channel.bot_token);
                    let timestamp_local = result.timestamp.unwrap().with_timezone(&Local);

                    let mut output = String::new();

                    output.push_str(&result.get_output());

                    for (key, value) in result.get_perf_data() {
                        output.push_str(&format!("{key} = {value}\n"));
                    }

                    match  http_client.post(&url).form(&[
                        ("chat_id", channel.chat_id.clone()),
                        ("text", format!("{0} - {1} is {2:?}: {3}", timestamp_local.format("%Y-%m-%d %H:%M:%S %Z"), result.check_name, result.status, output)),
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

async fn watch_checks(
    config: PinglowConfig,
    event_rx: Sender<RunnableCheckEvent>,
) -> Result<(), ReconcileError> {
    let client = Client::try_default().await?;
    let checks: Api<Check> = Api::namespaced(client.clone(), &config.target_namespace);
    let watcher = watcher(checks, Default::default());

    // Pin the watcher stream
    let mut watcher = Box::pin(watcher);

    while let Some(event) = watcher.next().await {
        match event {
            Ok(Event::Apply(check)) => {
                info!("Check definition updated {:?}", check.metadata.name);
                // handle added or updated Check
                let maybe_runnable = load_single_runnable_check(&check, &client, &config).await;

                if let Ok(runnable_check) = maybe_runnable {
                    event_rx
                        .send(RunnableCheckEvent::AddOrUpdate(Arc::new(runnable_check)))
                        .await
                        .ok();
                }
            }
            Ok(Event::Delete(check)) => {
                if let Some(name) = check.metadata.name {
                    event_rx.send(RunnableCheckEvent::Remove(name)).await.ok();
                }
            }
            Err(e) => {
                // Watch failed temporarily
                eprintln!("watcher error: {e}");
            }
            _ => {}
        }
    }

    Ok(())
}

async fn load_single_runnable_check(
    check: &Check,
    client: &Client,
    config: &PinglowConfig,
) -> Result<RunnableCheck, ReconcileError> {
    let scripts: Api<Script> = Api::namespaced(client.clone(), &config.target_namespace);

    let secrets: Api<Secret> = Api::namespaced(client.clone(), &config.target_namespace);

    let telegram_channels_api: Api<TelegramChannel> =
        Api::namespaced(client.clone(), &config.target_namespace);

    // Get the script name from the check specification
    let script_name = &check.spec.scriptRef;

    // Retrieve the check name and use a default one if not found (unlikely)
    let check_name = check
        .metadata
        .name
        .clone()
        .unwrap_or("Unnamed check".to_string());

    // Retrieve the corresponding script
    let script = scripts
        .get(script_name)
        .await
        .map_err(|_| ReconcileError::ScriptNotFound(script_name.clone()))?;

    let mut telegram_channels = vec![];

    if let Some(channels) = &check.spec.telegramChannelRefs {
        for channel in channels.iter() {
            // Get concrete channel
            let channel = telegram_channels_api
                .get(channel)
                .await
                .map_err(|_| ReconcileError::TelegramChannelNotFound(channel.to_string()))?;

            let bot_secret = secrets
                .get(&channel.spec.botTokenRef)
                .await
                .map_err(|_| ReconcileError::SecretNotFound(channel.spec.botTokenRef.clone()))?;

            let bot_token = bot_secret
                .data
                .and_then(|d| d.get("botToken").cloned())
                .ok_or("Cannot find botToken")
                .map_err(|_| ReconcileError::SecretNotFound("botToken".to_owned()))?;

            telegram_channels.push(ConcreteTelegramChannel {
                chat_id: channel.spec.chatId.clone(),
                bot_token: String::from_utf8_lossy(&bot_token.0).to_string(),
            });
        }
    }

    let secrets_refs = &check.spec.secretRefs;

    let python_requirements = &script.spec.python_requirements;

    // Build the runnable check object
    let runnable_check = RunnableCheck {
        script: script.spec.content,
        interval: check.spec.interval,
        language: script.spec.language,
        check_name,
        secrets_refs: secrets_refs.clone(),
        python_requirements: python_requirements.clone(),
        telegram_channels,
    };

    Ok(runnable_check)
}

/**
 * This function is used to load all the checks from the CR of the pinglow namespace
 */
async fn load_checks(
    config: &PinglowConfig,
    event_rx: Sender<RunnableCheckEvent>,
) -> Result<(), ReconcileError> {
    // Create the kube client
    let client = Client::try_default().await?;

    // Get all checks and scripts from the target namespace
    let checks: Api<Check> = Api::namespaced(client.clone(), &config.target_namespace);

    let check_list = checks.list(&Default::default()).await?;

    for check in check_list.iter() {
        let runnable_check = load_single_runnable_check(check, &client, config).await?;

        event_rx
            .send(RunnableCheckEvent::AddOrUpdate(Arc::new(runnable_check)))
            .await
            .ok();
    }

    info!("Loaded {:?} check(s)", check_list.items.len());

    Ok(())
}

async fn start_rocket(
    pinglow_config: PinglowConfig,
    shared_checks: SharedChecks,
    client: Arc<tokio_postgres::Client>,
) -> Result<(Rocket<rocket::Ignite>, Shutdown), rocket::Error> {
    let figment = rocket::Config::figment()
        .merge(("address", "0.0.0.0"))
        .merge(("port", 8000));

    let rocket = rocket::custom(figment)
        .manage(pinglow_config)
        .manage(shared_checks)
        .manage(client)
        .mount(
            "/",
            routes![get_checks, get_check_status, get_performance_data],
        );

    let rocket = rocket.ignite().await?;

    let shutdown = rocket.shutdown();

    Ok((rocket, shutdown))
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
