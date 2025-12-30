use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use dashmap::DashMap;

use pinglow_common::{CheckResultStatus, PinglowCheck};

pub type SharedPinglowChecks = Arc<RwLock<HashMap<String, Arc<PinglowCheck>>>>;
pub type SharedChecks = Arc<DashMap<String, Arc<Check>>>;

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
