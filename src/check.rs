use std::{cmp::Ordering, fmt::Display, sync::Arc};

use chrono::{DateTime, Utc};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{sync::RwLock, time::Instant};
use tokio_postgres::Client;

pub type SharedChecks = Arc<RwLock<Vec<Arc<RunnableCheck>>>>;

#[derive(Debug, Serialize, PartialEq)]
pub enum CheckResultStatus {
    Ok,
    Warning,
    Critical,
    CheckError,
}

impl From<i32> for CheckResultStatus {
    fn from(value: i32) -> Self {
        match value {
            0 => CheckResultStatus::Ok,
            1 => CheckResultStatus::Warning,
            2 => CheckResultStatus::Critical,
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
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
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
}

impl CheckResult {
    pub fn set_check_result_timestamp(&mut self, timestamp: DateTime<Utc>) {
        self.timestamp = Some(timestamp);
    }

    pub fn map_to_check_error(check_name: &String, error_message: String) -> Self {
        Self {
            check_name: check_name.to_string(),
            output: error_message,
            status: CheckResultStatus::CheckError,
            timestamp: None,
            telegram_channels: Arc::from(&[][..]),
        }
    }

    pub async fn write_to_db(&self, client: Arc<Client>) -> Result<u64, tokio_postgres::Error> {
        if let Some(timestamp) = &self.timestamp {
            client
            .execute(
                "INSERT INTO check_result (timestamp, check_name, status, output) VALUES ($1, $2, $3, $4)",
                &[timestamp, &self.check_name, &self.status.to_number(), &self.output],
            )
            .await
        } else {
            client
                .execute(
                    "INSERT INTO check_result (check_name, status, output) VALUES ($1, $2, $3)",
                    &[&self.check_name, &self.status.to_number(), &self.output],
                )
                .await
        }
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
    group = "myapp.io",
    version = "v1alpha1",
    kind = "TelegramChannel",
    namespaced
)]
pub struct TelegramChannelSpec {
    pub chat_id: String,
    pub bot_token_ref: String, // The name of the secret
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
    pub scriptRef: String,
    pub interval: u64,
    pub secretRefs: Option<Vec<String>>,
    pub telegram_channels: Option<Vec<TelegramChannelSpec>>,
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
pub struct RunnableCheck {
    pub script: String,
    pub interval: u64,
    pub language: ScriptLanguage,
    pub check_name: String,
    pub secrets_refs: Option<Vec<String>>,
    pub python_requirements: Option<Vec<String>>,
    pub telegram_channels: Vec<ConcreteTelegramChannel>,
}

#[derive(Clone, Debug)]
pub struct ScheduledCheck {
    pub check: Arc<RunnableCheck>,
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
