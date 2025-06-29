use std::{cmp::Ordering, fmt::Display};

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

#[derive(Debug)]
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
    pub timestamp: Option<String>,
}

impl CheckResult {
    pub fn set_check_result_timestamp(&mut self, timestamp: String) {
        self.timestamp = Some(timestamp);
    }

    pub fn map_to_check_error(check_name: &String, error_message: String) -> Self {
        Self {
            check_name: check_name.to_string(),
            output: error_message,
            status: CheckResultStatus::CheckError,
            timestamp: None,
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
#[kube(group = "pinglow.io", version = "v1alpha1", kind = "Check", namespaced)]
#[allow(non_snake_case)]
pub struct CheckSpec {
    pub scriptRef: String,
    pub interval: u64,
    pub secretRefs: Option<Vec<String>>,
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
}

#[derive(Clone, Debug)]
pub struct ScheduledCheck {
    pub check: RunnableCheck,
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
