use std::{cmp::Ordering, collections::BTreeMap};

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

pub struct CheckResult {
    pub check_name: String,
    pub output: String,
    pub status: CheckResultStatus,
    pub timestamp: String,
}

pub fn map_command_exit_code_to_check_result(exit_code: Option<i32>) -> CheckResultStatus {
    if let Some(exit_code) = exit_code {
        return CheckResultStatus::from(exit_code);
    }
    CheckResultStatus::CheckError
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(group = "pinglow.io", version = "v1alpha1", kind = "Check", namespaced)]
pub struct CheckSpec {
    #[allow(non_snake_case)]
    pub scriptRef: String,
    pub config: BTreeMap<String, String>,
    pub interval: u64,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "pinglow.io",
    version = "v1alpha1",
    kind = "Script",
    namespaced
)]
pub struct ScriptSpec {
    pub language: String,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct RunnableCheck {
    pub script: String,
    pub interval: u64,
    pub language: String,
    pub check_name: String,
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
