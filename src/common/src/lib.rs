use chrono::{DateTime, Utc};
use kube::CustomResource;
use log::warn;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::HashMap, fmt::Display, sync::Arc};
use tokio::time::Instant;
use tokio_postgres::Client;
use utoipa::ToSchema;

pub mod error;
pub mod redis;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub enum CheckResultStatus {
    Ok,
    Warning,
    Critical,
    CheckError,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum BuiltinCollector {
    Disk {
        paths: Vec<String>,
        #[serde(default)]
        exclude_mount_points: Vec<String>,
        #[serde(default)]
        exclude_filesystem_types: Vec<String>,
        warning_percent: f64,
        critical_percent: f64,
    },
    Memory {
        warning_percent: f64,
        critical_percent: f64,
    },
    Load {
        #[serde(default = "default_load_period_minutes")]
        period_minutes: u8,
        warning_load: f64,
        critical_load: f64,
    },
}

fn default_load_period_minutes() -> u8 {
    1
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

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, ToSchema, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcreteTelegramChannel {
    pub chat_id: String,
    pub bot_token: String, // The name of the secret
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckResult {
    pub check_name: String,
    pub output: String,
    pub status: CheckResultStatus,
    pub timestamp: Option<DateTime<Utc>>,
    pub telegram_channels: Arc<Vec<ConcreteTelegramChannel>>,
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
            telegram_channels: Arc::from(vec![]),
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

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "pinglow.io",
    version = "v1alpha1",
    kind = "Script",
    namespaced
)]
pub struct ScriptSpec {
    //pub language: ScriptLanguage,
    pub content: String,
    pub python_requirements: Option<Vec<String>>,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "pinglow.io",
    version = "v1alpha1",
    kind = "PinglowUserBinding",
    namespaced
)]
pub struct PinglowUserBindingSpec {
    pub role: UserRole,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub groups: Option<Vec<String>>,
    pub check_groups: Option<Vec<String>>,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "pinglow.io",
    version = "v1alpha1",
    kind = "ApiKeyBinding",
    namespaced
)]
pub struct ApiKeyBindingSpec {
    pub role: UserRole,
    pub secret_name: Option<String>,
    pub check_groups: Option<Vec<String>>,
    pub agent_target: Option<String>,
    pub agent_targets: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Viewer,
    Operator,
    Admin,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Secret {
    pub name: String,
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinglowCheck {
    pub passive: bool,
    pub script: Option<ScriptSpec>,
    #[serde(default)]
    pub collector: Option<BuiltinCollector>,
    pub interval: Option<u64>,
    pub check_name: String,
    #[serde(default = "default_run_on")]
    pub run_on: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub group_name: Option<String>,
    pub secrets: Option<HashMap<String, String>>,
    pub telegram_channels: Vec<ConcreteTelegramChannel>,
    pub mute_notifications: Option<bool>,
    pub mute_notifications_until: Option<DateTime<Utc>>,
}

fn default_run_on() -> String {
    "cluster".to_owned()
}

#[derive(Clone, Debug)]
pub struct ScheduledCheck {
    pub check: Arc<PinglowCheck>,
    pub next_run: Instant,
}

impl PartialEq for ScheduledCheck {
    fn eq(&self, other: &Self) -> bool {
        self.next_run == other.next_run
    }
}
impl Eq for ScheduledCheck {}

impl PartialOrd for ScheduledCheck {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScheduledCheck {
    fn cmp(&self, other: &Self) -> Ordering {
        other.next_run.cmp(&self.next_run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_check_payloads_without_group_metadata() {
        let payload = r#"{
            "passive": true,
            "script": null,
            "interval": null,
            "check_name": "legacy-check",
            "secrets": null,
            "telegram_channels": [],
            "mute_notifications": null,
            "mute_notifications_until": null
        }"#;

        let check: PinglowCheck = serde_json::from_str(payload).unwrap();

        assert_eq!(check.group_id, None);
        assert_eq!(check.group_name, None);
    }

    #[test]
    fn builtin_collector_uses_camel_case_configuration_fields() {
        let collector = BuiltinCollector::Disk {
            paths: vec!["/".to_owned()],
            exclude_mount_points: vec!["/backup".to_owned()],
            exclude_filesystem_types: vec!["tmpfs".to_owned()],
            warning_percent: 80.0,
            critical_percent: 90.0,
        };
        let json = serde_json::to_string(&collector).unwrap();
        assert!(json.contains("\"excludeMountPoints\""));
        assert!(json.contains("\"warningPercent\""));
        assert_eq!(
            serde_json::from_str::<BuiltinCollector>(&json).unwrap(),
            collector
        );
    }

    #[test]
    fn load_collector_uses_camel_case_fields_and_defaults_to_one_minute() {
        let json = r#"{"type":"load","periodMinutes":5,"warningLoad":2,"criticalLoad":4}"#;
        let collector: BuiltinCollector = serde_json::from_str(json).unwrap();
        assert_eq!(
            collector,
            BuiltinCollector::Load {
                period_minutes: 5,
                warning_load: 2.0,
                critical_load: 4.0,
            }
        );

        let json = r#"{"type":"load","warningLoad":2,"criticalLoad":4}"#;
        assert_eq!(
            serde_json::from_str::<BuiltinCollector>(json).unwrap(),
            BuiltinCollector::Load {
                period_minutes: 1,
                warning_load: 2.0,
                critical_load: 4.0,
            }
        );
    }

    #[test]
    fn builtin_collector_rejects_unknown_configuration_fields() {
        let json =
            r#"{"type":"memory","warningPercent":80,"criticalPercent":90,"warnngPercent":85}"#;
        assert!(serde_json::from_str::<BuiltinCollector>(json).is_err());
    }

    #[test]
    fn test_user_role_equality() {
        assert_eq!(UserRole::Admin, UserRole::Admin);
        assert_eq!(UserRole::Operator, UserRole::Operator);
        assert_eq!(UserRole::Viewer, UserRole::Viewer);
    }

    #[test]
    fn test_user_role_inequality() {
        assert_ne!(UserRole::Admin, UserRole::Operator);
        assert_ne!(UserRole::Operator, UserRole::Viewer);
        assert_ne!(UserRole::Admin, UserRole::Viewer);
    }

    #[test]
    fn test_check_result_status_equality() {
        assert_eq!(CheckResultStatus::Ok, CheckResultStatus::Ok);
        assert_eq!(CheckResultStatus::Warning, CheckResultStatus::Warning);
        assert_eq!(CheckResultStatus::Critical, CheckResultStatus::Critical);
        assert_eq!(CheckResultStatus::CheckError, CheckResultStatus::CheckError);
        assert_eq!(CheckResultStatus::Pending, CheckResultStatus::Pending);
    }

    #[test]
    fn test_check_result_status_to_number() {
        assert_eq!(CheckResultStatus::Ok.to_number(), 0);
        assert_eq!(CheckResultStatus::Warning.to_number(), 1);
        assert_eq!(CheckResultStatus::Critical.to_number(), 2);
        assert_eq!(CheckResultStatus::CheckError.to_number(), 3);
        assert_eq!(CheckResultStatus::Pending.to_number(), 4);
    }

    #[test]
    fn test_check_result_status_from_i32() {
        assert_eq!(CheckResultStatus::from(0i32), CheckResultStatus::Ok);
        assert_eq!(CheckResultStatus::from(1i32), CheckResultStatus::Warning);
        assert_eq!(CheckResultStatus::from(2i32), CheckResultStatus::Critical);
        assert_eq!(CheckResultStatus::from(3i32), CheckResultStatus::CheckError);
        assert_eq!(CheckResultStatus::from(4i32), CheckResultStatus::Pending);
    }

    #[test]
    fn test_scheduled_check_ordering() {
        let now = Instant::now();
        let future = now + std::time::Duration::from_secs(60);

        let check1 = PinglowCheck {
            passive: true,
            script: None,
            collector: None,
            interval: Some(300),
            check_name: "check1".to_string(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: None,
            group_name: None,
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        };

        let check2 = PinglowCheck {
            passive: true,
            script: None,
            collector: None,
            interval: Some(300),
            check_name: "check2".to_string(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: None,
            group_name: None,
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        };

        let scheduled_now = ScheduledCheck {
            check: Arc::new(check1),
            next_run: now,
        };
        let scheduled_future = ScheduledCheck {
            check: Arc::new(check2),
            next_run: future,
        };

        assert!(
            scheduled_future < scheduled_now,
            "later next_run should be sorted before earlier"
        );
    }

    #[test]
    fn test_scheduled_check_partial_ord() {
        let now = Instant::now();

        let check1 = PinglowCheck {
            passive: true,
            script: None,
            collector: None,
            interval: Some(300),
            check_name: "check1".to_string(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: None,
            group_name: None,
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        };

        let check2 = PinglowCheck {
            passive: true,
            script: None,
            collector: None,
            interval: Some(300),
            check_name: "check2".to_string(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: None,
            group_name: None,
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        };

        let scheduled1 = ScheduledCheck {
            check: Arc::new(check1),
            next_run: now,
        };
        let scheduled2 = ScheduledCheck {
            check: Arc::new(check2),
            next_run: now,
        };

        assert_eq!(scheduled1.partial_cmp(&scheduled2), Some(Ordering::Equal));
    }

    #[test]
    fn test_api_key_binding_spec_role() {
        let binding = ApiKeyBindingSpec {
            role: UserRole::Operator,
            secret_name: Some("my-secret".to_string()),
            check_groups: Some(vec!["home".to_string()]),
            agent_target: None,
            agent_targets: None,
        };
        assert_eq!(binding.role, UserRole::Operator);
        assert_eq!(binding.secret_name, Some("my-secret".to_string()));
        assert_eq!(binding.check_groups, Some(vec!["home".to_string()]));
    }

    #[test]
    fn test_api_key_binding_spec_default_secret_name() {
        let binding = ApiKeyBindingSpec {
            role: UserRole::Admin,
            secret_name: None,
            check_groups: None,
            agent_target: None,
            agent_targets: None,
        };
        assert_eq!(binding.role, UserRole::Admin);
        assert_eq!(binding.secret_name, None);
    }

    #[test]
    fn api_key_binding_deserializes_multiple_agent_targets() {
        let binding: ApiKeyBindingSpec = serde_json::from_value(serde_json::json!({
            "role": "operator",
            "agent_targets": ["nas", "linux-hosts"]
        }))
        .unwrap();

        assert_eq!(
            binding.agent_targets,
            Some(vec!["nas".to_owned(), "linux-hosts".to_owned()])
        );
        assert_eq!(binding.agent_target, None);
    }
}
