use check::{Check, CheckResult};
use env_logger::{self, Builder};
use log::info;
use tokio::sync::mpsc;

use kube::{Api, Client};

use crate::{
    check::{RunnableCheck, Script},
    config::{get_config_from_env, PinglowConfig},
    error::ReconcileError,
    runner::scheduler_loop,
};

mod check;
mod config;
mod error;
mod job;
mod runner;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize the logger
    Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Get the configuration
    let config = get_config_from_env();

    // Load all the available checks
    let checks = load_checks(&config).await?;

    // Prepare the channel to send back the results
    let (result_tx, mut result_rx) = mpsc::channel::<CheckResult>(100);

    // Spawn the task which will schedule the checks in a continuous way
    tokio::spawn(scheduler_loop(checks, result_tx, config.target_namespace));

    // Wait for the results and log them
    while let Some(result) = result_rx.recv().await {
        info!(
            "[Result] {} @ {}: {:?}: {:?}",
            result.check_name,
            result.timestamp.unwrap_or("".to_string()),
            result.status,
            result.output
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
        };

        // Add it to our queue of checks
        runnable_checks.push(runnable_check);
    }

    info!("Loaded {:?} check(s)", runnable_checks.len());

    Ok(runnable_checks)
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
