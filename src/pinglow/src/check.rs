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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_process_exit_codes() {
        assert_eq!(
            map_command_exit_code_to_check_result(Some(0)),
            CheckResultStatus::Ok
        );
        assert_eq!(
            map_command_exit_code_to_check_result(Some(1)),
            CheckResultStatus::Warning
        );
        assert_eq!(
            map_command_exit_code_to_check_result(Some(2)),
            CheckResultStatus::Critical
        );
        assert_eq!(
            map_command_exit_code_to_check_result(Some(4)),
            CheckResultStatus::Pending
        );
    }

    #[test]
    fn maps_unknown_or_missing_process_exit_codes_to_check_error() {
        assert_eq!(
            map_command_exit_code_to_check_result(Some(3)),
            CheckResultStatus::CheckError
        );
        assert_eq!(
            map_command_exit_code_to_check_result(Some(127)),
            CheckResultStatus::CheckError
        );
        assert_eq!(
            map_command_exit_code_to_check_result(None),
            CheckResultStatus::CheckError
        );
    }
}
