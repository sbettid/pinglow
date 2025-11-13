use std::{cmp::Ordering, collections::HashMap, fmt::Display, sync::Arc};

use chrono::{DateTime, Utc};
use kube::CustomResource;
use log::warn;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{sync::RwLock, time::Instant};
use tokio_postgres::Client;

use dashmap::DashMap;
use utoipa::ToSchema;

pub type SharedPinglowChecks = Arc<RwLock<HashMap<String, Arc<PinglowCheck>>>>;
pub type SharedChecks = Arc<DashMap<String, Arc<Check>>>;

#[derive(Debug, Serialize, PartialEq, ToSchema)]
pub enum CheckResultStatus {
    Ok,
    Warning,
    Critical,
    CheckError,
    Pending,
}

impl From<i32> for CheckResultStatus {
    fn from(value: i32) -> Self {
        match value {
            0 => CheckResultStatus::Ok,
            1 => CheckResultStatus::Warning,
            2 => CheckResultStatus::Critical,
            4 => CheckResultStatus::Pending,
            _ => CheckResultStatus::CheckError,
        }
    }
}

impl From<i16> for CheckResultStatus {
    fn from(value: i16) -> Self {
        match value {
            0 => CheckResultStatus::Ok,
            1 => CheckResultStatus::Warning,
            2 => CheckResultStatus::Critical,
            4 => CheckResultStatus::Pending,
            _ => CheckResultStatus::CheckError,
        }
    }
}

impl CheckResultStatus {
    pub fn to_number(&self) -> i16 {
        match self {
            CheckResultStatus::Ok => 0,
            CheckResultStatus::Warning => 1,
            CheckResultStatus::Critical => 2,
            CheckResultStatus::CheckError => 3,
            CheckResultStatus::Pending => 4,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, ToSchema)]
pub enum ScriptLanguage {
    #[serde(rename = "Python")]
    Python,
    #[serde(rename = "Bash")]
    Bash,
}

impl Display for ScriptLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptLanguage::Python => write!(f, "python"),
            ScriptLanguage::Bash => write!(f, "bash"),
        }
    }
}

pub struct CheckResult {
    pub check_name: String,
    pub output: String,
    pub status: CheckResultStatus,
    pub timestamp: Option<DateTime<Utc>>,
    pub telegram_channels: Arc<[ConcreteTelegramChannel]>,
    pub mute_notifications: Option<bool>,
    pub mute_notifications_until: Option<DateTime<Utc>>,
}

impl CheckResult {
    pub fn set_check_result_timestamp(&mut self, timestamp: DateTime<Utc>) {
        self.timestamp = Some(timestamp);
    }

    pub fn map_to_check_error(
        check_name: &String,
        error_message: String,
        mute_notifications: Option<bool>,
        mute_notifications_until: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            check_name: check_name.to_string(),
            output: error_message,
            status: CheckResultStatus::CheckError,
            timestamp: None,
            telegram_channels: Arc::from(&[][..]),
            mute_notifications,
            mute_notifications_until,
        }
    }

    pub fn get_output(&self) -> String {
        let (output, _perf_data) = match self.output.split_once("|") {
            Some((out, perf)) => (out, perf),
            None => (self.output.as_ref(), ""),
        };

        output.to_string()
    }

    pub fn get_perf_data(&self) -> Vec<(String, f32)> {
        let (_output, perf_data) = match self.output.split_once("|") {
            Some((out, perf)) => (out, perf),
            None => (self.output.as_ref(), ""),
        };

        let perf_data_list: Vec<(String, f32)> = perf_data
            .split(",")
            .filter_map(|pair| {
                pair.split_once('=') // Split each entry into key=value
                    .map(|(k, v)| {
                        (
                            k.trim().to_string(),
                            v.trim().to_string().parse::<f32>().unwrap_or_else(|e| {
                                warn!("Unable to parse performance metric as a float, setting it to 0.0 - {e}");
                                0.0
                            }),
                        )
                    })
            })
            .collect();

        perf_data_list
    }

    pub async fn write_to_db(&self, client: Arc<Client>) -> Result<(), tokio_postgres::Error> {
        // Parse the output to remove the performance data, if any
        let output = self.get_output();

        let perf_data_list = self.get_perf_data();

        // If by chance we do not set the timestamp before, it is set to now
        let timestamp = match self.timestamp {
            Some(t) => t,
            None => Utc::now(),
        };

        // Insert the main check result
        client
            .execute(
                "INSERT INTO check_result (timestamp, check_name, status, output) VALUES ($1, $2, $3, $4)",
                &[&timestamp, &self.check_name, &self.status.to_number(), &output],
            )
            .await?;

        // Insert performance data, if needed
        for (perf_key, perf_value) in perf_data_list {
            client
            .execute(
                "INSERT INTO check_result_perf_data (timestamp, check_name, perf_key, perf_value) VALUES ($1, $2, $3, $4)",
                &[&timestamp, &self.check_name, &perf_key, &perf_value],
            )
            .await?;
        }

        Ok(())
    }
}

pub fn map_command_exit_code_to_check_result(exit_code: Option<i32>) -> CheckResultStatus {
    if let Some(exit_code) = exit_code {
        return CheckResultStatus::from(exit_code);
    }
    CheckResultStatus::CheckError
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "pinglow.io",
    version = "v1alpha1",
    kind = "TelegramChannel",
    namespaced
)]
#[allow(non_snake_case)]
pub struct TelegramChannelSpec {
    pub chatId: String,
    pub botTokenRef: String, // The name of the secret
}

#[derive(Debug, Clone)]
pub struct ConcreteTelegramChannel {
    pub chat_id: String,
    pub bot_token: String, // The name of the secret
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(group = "pinglow.io", version = "v1alpha1", kind = "Check", namespaced)]
#[allow(non_snake_case)]
pub struct CheckSpec {
    pub scriptRef: Option<String>,
    pub interval: Option<u64>,
    pub secretRefs: Option<Vec<String>>,
    pub telegramChannelRefs: Option<Vec<String>>,
    pub muteNotifications: Option<bool>,
    pub muteNotificationsUntil: Option<DateTime<Utc>>,
    pub passive: bool,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "pinglow.io",
    version = "v1alpha1",
    kind = "Script",
    namespaced
)]
pub struct ScriptSpec {
    pub language: ScriptLanguage,
    pub content: String,
    pub python_requirements: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct PinglowCheck {
    pub passive: bool,
    pub script: Option<ScriptSpec>,
    pub interval: Option<u64>,
    pub check_name: String,
    pub secrets_refs: Option<Vec<String>>,
    pub telegram_channels: Vec<ConcreteTelegramChannel>,
    pub mute_notifications: Option<bool>,
    pub mute_notifications_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct ScheduledCheck {
    pub check: Arc<PinglowCheck>,
    pub next_run: Instant,
}

// Min-heap (invert the comparison)
impl PartialEq for ScheduledCheck {
    fn eq(&self, other: &Self) -> bool {
        self.next_run == other.next_run
    }
}
impl Eq for ScheduledCheck {}

impl PartialOrd for ScheduledCheck {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(other.next_run.cmp(&self.next_run)) // reverse
    }
}
impl Ord for ScheduledCheck {
    fn cmp(&self, other: &Self) -> Ordering {
        other.next_run.cmp(&self.next_run) // reverse
    }
}
