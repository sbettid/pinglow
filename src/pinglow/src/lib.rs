use std::{collections::HashMap, sync::Arc};

use anyhow::Error;
use chrono::{Local, Utc};
use html_escape::encode_safe;
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};
use log::error;
use tokio_postgres::Client as PostgresClient;

use crate::{
    check::{Check, TelegramChannel},
    config::PinglowConfig,
    error::ReconcileError,
};

use pinglow_common::{
    CheckResult, CheckResultStatus, ConcreteTelegramChannel, PinglowCheck, Script,
};

pub mod api;
pub mod check;
pub mod config;
pub mod controller;
pub mod error;
pub mod results;
pub mod scheduler;

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

    // Check if we have secrets
    let secrets = if let Some(secrets_refs) = &check.spec.secretRefs {
        Some(
            fetch_secrets(&config.target_namespace, secrets_refs)
                .await
                .map_err(|e| {
                    ReconcileError::GeneralError(format!("Error fetching secrets: {e}"))
                })?,
        )
    } else {
        None
    };

    // Build the runnable check object
    let runnable_check = PinglowCheck {
        passive: check.spec.passive,
        script: script.map(|s| s.spec),
        interval: check.spec.interval,
        check_name,
        secrets,
        telegram_channels,
        mute_notifications: check.spec.muteNotifications,
        mute_notifications_until: check.spec.muteNotificationsUntil,
    };

    Ok(runnable_check)
}

async fn fetch_secrets(
    namespace: &str,
    secret_names: &[String],
) -> Result<HashMap<String, String>, Error> {
    let client = Client::try_default().await?;
    let secrets_api: Api<Secret> = Api::namespaced(client, namespace);

    let mut map = HashMap::new();

    for secret_name in secret_names {
        if let Ok(secret) = secrets_api.get(secret_name).await {
            if let Some(data) = secret.data {
                for (key, value) in data {
                    // Secrets are base64 encoded
                    let decoded = std::str::from_utf8(&value.0)?;
                    map.insert(key.clone(), decoded.to_string());
                }
            }
        }
    }

    Ok(map)
}

/**
 * This function is used to process a check result and write the result
 * to the DB and send it, if needed, to the notification channel
 */
pub async fn process_check_result(
    result: CheckResult,
    db_client: &Arc<PostgresClient>,
    http_client: &reqwest::Client,
) -> Result<(), Error> {
    // Write result to DB
    result.write_to_db(db_client.clone()).await?;

    // Send result to telegram channels
    if result.status != CheckResultStatus::Ok
        && result.status != CheckResultStatus::Pending
        && match result.mute_notifications {
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
            let url = format!(
                "https://api.telegram.org/bot{}/sendMessage",
                channel.bot_token
            );
            let timestamp_local = result
                .timestamp
                .unwrap_or_else(Utc::now)
                .with_timezone(&Local);

            match  http_client.post(&url).form(&[
                        ("chat_id", channel.chat_id.clone()),
                        ("text", format!("<b>Date</b>: {0}\n<b>Check name</b>: {1} \n<b>Status</b>: {2:?}\n<b>Output</b>\n<pre>{3}</pre>", timestamp_local.format("%Y-%m-%d %H:%M:%S %Z"), result.check_name, result.status, encode_safe(&result.get_output()))),
                        ("parse_mode", "HTML".to_string()),
                    ]).send().await {
                        Ok(_) => {},
                        Err(e) => error!("Error when sending check result to Telegram channel: {e}"),
                    }
        }
    }
    Ok(())
}
