use std::sync::Arc;

use rocket::{get, serde::json::Json, State};
use serde::Serialize;
use tokio_postgres::Client;

use crate::check::{CheckResult, RunnableCheck, ScriptLanguage, SharedChecks};

#[derive(Serialize)]
pub struct SimpleCheckDto {
    pub check_name: String,
    pub interval: u64,
    pub language: ScriptLanguage,
}

impl From<&RunnableCheck> for SimpleCheckDto {
    fn from(value: &RunnableCheck) -> Self {
        Self {
            check_name: value.check_name.clone(),
            interval: value.interval,
            language: value.language.clone(),
        }
    }
}

#[get("/checks")]
pub async fn get_checks(checks: &State<SharedChecks>) -> Json<Vec<SimpleCheckDto>> {
    let runnable_checks = checks.read().await;

    let simple_checks_to_return: Vec<SimpleCheckDto> = runnable_checks
        .iter()
        .map(|check| check.as_ref().into())
        .collect();

    Json(simple_checks_to_return)
}

#[get("/check-status/<target_check>")]
pub async fn get_check_status(
    checks: &State<SharedChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
) -> Option<Json<CheckResult>> {
    let runnable_checks = checks.read().await;

    runnable_checks
        .iter()
        .find(|&check| check.check_name == target_check)?;

    let last_check_result = client.query_one("SELECT timestamp,status,output from check_result where check_name = $1 order by timestamp desc limit 1", &[&target_check]).await.ok()?;
    let check_status: i16 = last_check_result.get("status");
    Some(Json(CheckResult {
        check_name: target_check.to_string(),
        output: last_check_result.get("output"),
        status: crate::check::CheckResultStatus::from(check_status),
        timestamp: last_check_result.get("timestamp"),
    }))
}
