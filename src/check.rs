use std::fmt::Display;

#[derive(Debug)]
pub enum CheckResultStatus {
    SUCCESS,
    FAILURE
}

pub struct CheckResult {
    pub check_name: String,
    pub output: String,
    pub status: CheckResultStatus,
    pub timestamp: String
}

pub struct Check {
    pub check_name: String,
    pub command: String,
    pub check_interval: u64
}