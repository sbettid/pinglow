use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use env_logger::{self, Builder};
use log::{error, info};
use pinglow::check::Check;
use pinglow::{load_single_runnable_check, results};
use pinglow_common::redis::init_streams;
use tokio::signal::unix::signal;
use tokio::sync::mpsc::Sender;
use tokio::{
    signal::unix::SignalKind,
    sync::{mpsc, RwLock},
};

use kube::{Api, Client};
use tokio_postgres::NoTls;

use pinglow::api::start_rocket;
use pinglow::check::SharedPinglowChecks;
use pinglow::controller::watch_resources;
use pinglow::scheduler::RunnableCheckEvent;
use pinglow::{
    check::SharedChecks,
    config::{get_config_from_env, PinglowConfig},
    error::ReconcileError,
    scheduler::scheduler_loop,
};
use pinglow_common::redis::redis_client;

mod embedded {
    use refinery::embed_migrations;
    embed_migrations!("db_migrations");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the logger
    Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Get the configuration
    let config = get_config_from_env();

    info!("Connecting to timescaledb");

    // Connect to the DB
    let (mut postgres_client, connection) = tokio_postgres::connect(
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
        .run_async(&mut postgres_client)
        .await?;

    let postgres_client_arc = Arc::new(postgres_client);

    info!("Connecting to redis");

    let redis_client = redis_client()?;

    // Init streams (short-lived connection)
    {
        let mut conn = redis_client.get_multiplexed_async_connection().await?;
        init_streams(&mut conn).await;
    } // conn dropped here

    // Hashmap that holds the checks currently loaded
    let shared_checks: SharedPinglowChecks = Arc::new(RwLock::new(HashMap::new()));
    let shared_original_checks: SharedChecks = Arc::new(DashMap::new());

    // Channels to communicate checks update events
    let (event_tx, event_rx) = mpsc::channel::<RunnableCheckEvent>(100);

    // Load all the available checks
    load_checks(&config, event_tx.clone(), shared_original_checks.clone()).await?;

    // Thread to watch for the changes in Pinglow resources
    tokio::spawn(watch_resources(
        config.clone(),
        event_tx,
        shared_original_checks,
    ));

    // Spawn the task which will schedule the checks in a continuous way
    let mut scheduler = tokio::spawn(scheduler_loop(
        event_rx,
        shared_checks.clone(),
        redis_client.clone(),
    ));

    // Spawn the task that will process the results
    let mut result_consumer = tokio::spawn(results::run(
        redis_client.clone(),
        postgres_client_arc.clone(),
    ));

    // Spawn the task to host Rocket to handle API requests
    let (rocket, rocket_shutdown) =
        start_rocket(config, shared_checks.clone(), postgres_client_arc.clone()).await?;
    let rocket_handle = tokio::spawn(async move {
        rocket.launch().await?;
        Ok::<(), rocket::Error>(())
    });

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    tokio::select! {
        _ = &mut scheduler => {
            info!("Scheduler terminated");
        }
        _ = &mut result_consumer => {
            info!("Result thread terminated");
        }

        // In case we receive a sigterm we exit to teardown our jobs in a clean way (especially rocket)
        _ = sigint.recv() => {
        }
        _ = sigterm.recv() => {
        }

    }

    info!("Shutting down...");
    rocket_shutdown.notify();

    scheduler.abort();
    result_consumer.abort();
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
