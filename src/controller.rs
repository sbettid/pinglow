use std::sync::Arc;

use crate::{
    check::{Check, Script, SharedChecks, TelegramChannel},
    config::PinglowConfig,
    error::ReconcileError,
    load_single_runnable_check,
    runner::RunnableCheckEvent,
};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::runtime::{controller::Action, reflector::ObjectRef};
use kube::{
    runtime::{watcher, Controller},
    Api, Client,
};
use log::{debug, error};
use std::time::Duration;
use tokio::sync::mpsc::Sender;

#[derive(Clone)]
struct ContextData {
    client: Client,
    config: PinglowConfig,
    event_rx: Sender<RunnableCheckEvent>,
    shared_checks: SharedChecks,
}

pub async fn watch_resources(
    pinglow_config: PinglowConfig,
    event_rx: Sender<RunnableCheckEvent>,
    shared_original_checks: SharedChecks,
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let client = Client::try_default().await?;
    let checks: Api<Check> = Api::namespaced(client.clone(), &pinglow_config.target_namespace);
    let scripts: Api<Script> = Api::namespaced(client.clone(), &pinglow_config.target_namespace);

    let secrets: Api<Secret> = Api::namespaced(client.clone(), &pinglow_config.target_namespace);

    let telegram_channels: Api<TelegramChannel> =
        Api::namespaced(client.clone(), &pinglow_config.target_namespace);

    let config = watcher::Config::default();

    let context = Arc::new(ContextData {
        client: client.clone(),
        config: pinglow_config.clone(),
        event_rx,
        shared_checks: shared_original_checks.clone(),
    });

    // Set up the controller
    Controller::new(checks, watcher::Config::default())
        .watches(scripts, config.clone(), {
            let shared = Arc::clone(&shared_original_checks);
            move |script| map_script_to_checks(script, shared.clone())
        })
        .watches(secrets, config.clone(), {
            let shared = Arc::clone(&shared_original_checks);
            move |secret| map_secret_to_checks(secret, shared.clone())
        })
        .watches(telegram_channels, config, {
            let shared = Arc::clone(&shared_original_checks);
            move |channel| map_channel_to_checks(channel, shared.clone())
        })
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok((o, _)) => debug!("Check update send {o:?}"),
                Err(e) => error!("Reconcile loop error: {e}"),
            }
        })
        .await;

    Ok(())
}

/// The reconciler that will be called when either object change
async fn reconcile(check: Arc<Check>, ctx: Arc<ContextData>) -> Result<Action, ReconcileError> {
    let check_name =
        check
            .metadata
            .name
            .as_ref()
            .ok_or(ReconcileError::PropertyExtractionError(format!(
                "Error in extracting the name for {check:?}",
            )))?;

    if check.metadata.deletion_timestamp.is_some() {
        ctx.shared_checks.remove(check_name);
        ctx.event_rx
            .send(RunnableCheckEvent::Remove(check_name.to_string()))
            .await
            .map_err(|e| {
                ReconcileError::SendError(format!("Error sending the check event {e:?}"))
            })?;
        return Ok(Action::await_change());
    }

    let runnable_check = load_single_runnable_check(&check, &ctx.client, &ctx.config).await?;

    ctx.shared_checks.insert(check_name.clone(), check);

    ctx.event_rx
        .send(RunnableCheckEvent::AddOrUpdate(Arc::new(runnable_check)))
        .await
        .ok();
    Ok(Action::await_change())
}
/// an error handler that will be called when the reconciler fails with access to both the
/// object that caused the failure and the actual error
fn error_policy(obj: Arc<Check>, error: &ReconcileError, _ctx: Arc<ContextData>) -> Action {
    error!(
        "Reconciliation error for check {:?}: {:?}",
        obj.metadata.name, error
    );
    Action::requeue(Duration::from_secs(60))
}

fn map_script_to_checks(
    script: Script,
    shared_original_checks: SharedChecks,
) -> Vec<ObjectRef<Check>> {
    let script_name = script.metadata.name.unwrap_or_default();

    let matching_checks: Vec<_> = shared_original_checks
        .iter()
        .filter_map(|entry| {
            if *entry.spec.scriptRef == script_name {
                Some(entry.value().clone())
            } else {
                None
            }
        })
        .collect();

    let object_refs: Vec<ObjectRef<Check>> = matching_checks
        .iter()
        .map(|check| ObjectRef::from(check.as_ref()))
        .collect();

    object_refs
}

fn map_secret_to_checks(
    script: Secret,
    shared_original_checks: SharedChecks,
) -> Vec<ObjectRef<Check>> {
    let secret_name = script.metadata.name.unwrap_or_default();

    let matching_checks: Vec<_> = shared_original_checks
        .iter()
        .filter_map(|entry| {
            let matching_secrets: Vec<_> = entry
                .value()
                .spec
                .secretRefs
                .as_ref()?
                .iter()
                .filter(|s| s.to_string() == secret_name)
                .collect();

            if !matching_secrets.is_empty() {
                Some(entry.value().clone())
            } else {
                None
            }
        })
        .collect();

    let object_refs: Vec<ObjectRef<Check>> = matching_checks
        .iter()
        .map(|check| ObjectRef::from(check.as_ref()))
        .collect();

    object_refs
}

fn map_channel_to_checks(
    channel: TelegramChannel,
    shared_original_checks: SharedChecks,
) -> Vec<ObjectRef<Check>> {
    let channel_name = channel.metadata.name.unwrap_or_default();

    let matching_checks: Vec<_> = shared_original_checks
        .iter()
        .filter_map(|entry| {
            let matching_channels: Vec<_> = entry
                .value()
                .spec
                .telegramChannelRefs
                .as_ref()?
                .iter()
                .filter(|s| s.to_string() == channel_name)
                .collect();

            if !matching_channels.is_empty() {
                Some(entry.value().clone())
            } else {
                None
            }
        })
        .collect();

    let object_refs: Vec<ObjectRef<Check>> = matching_checks
        .iter()
        .map(|check| ObjectRef::from(check.as_ref()))
        .collect();

    object_refs
}
