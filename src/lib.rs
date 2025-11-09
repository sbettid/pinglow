use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};

use crate::{
    check::{Check, ConcreteTelegramChannel, PinglowCheck, Script, TelegramChannel},
    config::PinglowConfig,
    error::ReconcileError,
};

pub mod api;
pub mod check;
pub mod config;
pub mod controller;
pub mod error;
pub mod job;
pub mod runner;

pub async fn load_single_runnable_check(
    check: &Check,
    client: &Client,
    config: &PinglowConfig,
) -> Result<PinglowCheck, ReconcileError> {
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
    let mut script = None;

    if let Some(script_name) = script_name {
        script = Some(
            scripts
                .get(script_name)
                .await
                .map_err(|_| ReconcileError::ScriptNotFound(script_name.clone()))?,
        );
    }
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

    // Build the runnable check object
    let runnable_check = PinglowCheck {
        passive: check.spec.passive,
        script: script.map(|s| s.spec),
        interval: check.spec.interval,
        check_name,
        secrets_refs: secrets_refs.clone(),
        telegram_channels,
        mute_notifications: check.spec.muteNotifications,
        mute_notifications_until: check.spec.muteNotificationsUntil,
    };

    Ok(runnable_check)
}
