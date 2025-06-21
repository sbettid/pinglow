use std::sync::{Arc, Mutex};

use check::{Check, CheckResult};
use env_logger;
use log::info;
use tokio::sync::mpsc;
use worker::run_check_loop;

use kube::{runtime::controller::Action, Api, Client};

use crate::{
    check::{RunnableCheck, Script},
    config::{get_config_from_env, PinglowConfig},
    error::ReconcileError,
};

mod check;
mod config;
mod error;
mod worker;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the logger
    env_logger::init();

    // Get the configuration
    let config = get_config_from_env();

    // Load all the available checks
    let checks = load_checks(&config).await?;

    let (result_tx, mut result_rx) = mpsc::channel::<CheckResult>(100);

    for check in checks {
        let tx = result_tx.clone();

        tokio::spawn(run_check_loop(check, tx));
    }

    while let Some(result) = result_rx.recv().await {
        info!(
            "[Result] {} @ {}: {:?}",
            result.check_name, result.timestamp, result.status
        );
    }

    Ok(())
}

/**
 * This function is used to load all the checks from the CR of the pinglow namespace
 */
async fn load_checks(config: &PinglowConfig) -> Result<Vec<RunnableCheck>, ReconcileError> {
    // Create the kube client
    let client = Client::try_default().await?;

    // Get all checks and scripts from the target namespace
    let checks: Api<Check> = Api::namespaced(client.clone(), &config.target_namespace);

    let check_list = checks.list(&Default::default()).await?;

    let scripts: Api<Script> = Api::namespaced(client.clone(), &config.target_namespace);

    // Create a struct where we can accumulate the actual check + script objects
    let mut runnable_checks = vec![];

    for check in check_list {
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

        // Build the runnable check object
        let runnable_check = RunnableCheck {
            script: script.spec.content,
            interval: check.spec.interval,
            language: script.spec.language,
            check_name,
        };

        runnable_checks.push(runnable_check);
    }

    Ok(runnable_checks)
}
