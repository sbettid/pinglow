use std::sync::Arc;

use chrono::{DateTime, Utc};
use rocket::{
    get,
    http::Status,
    request::{FromRequest, Outcome},
    serde::json::Json,
    Request, State,
};
use serde::Serialize;
use tokio_postgres::Client;

use crate::{
    check::{CheckResultStatus, RunnableCheck, ScriptLanguage, SharedChecks},
    config::PinglowConfig,
};

pub struct ApiKey;

// FromRequest trait to validate the provided ApiKey
#[rocket::async_trait]
impl<'r> FromRequest<'r> for ApiKey {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let config = request.rocket().state::<PinglowConfig>();
        let config = match config {
            Some(c) => c,
            None => return Outcome::Error((Status::InternalServerError, ())),
        };

        let keys: Vec<_> = request.headers().get("x-api-key").collect();

        if keys.len() != 1 {
            return Outcome::Error((Status::Unauthorized, ()));
        }

        let client_key = keys[0];
        if config.api_key == client_key {
            Outcome::Success(ApiKey)
        } else {
            Outcome::Error((Status::Unauthorized, ()))
        }
    }
}

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

#[derive(Serialize)]
pub struct SimpleCheckResultDto {
    pub check_name: String,
    pub output: String,
    pub status: CheckResultStatus,
    pub timestamp: Option<DateTime<Utc>>,
}

#[get("/checks")]
pub async fn get_checks(_key: ApiKey, checks: &State<SharedChecks>) -> Json<Vec<SimpleCheckDto>> {
    let runnable_checks = checks.read().await;

    let simple_checks_to_return: Vec<SimpleCheckDto> = runnable_checks
        .iter()
        .map(|check| check.as_ref().into())
        .collect();

    Json(simple_checks_to_return)
}

#[get("/check-status/<target_check>")]
pub async fn get_check_status(
    _key: ApiKey,
    checks: &State<SharedChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
) -> Option<Json<SimpleCheckResultDto>> {
    let runnable_checks = checks.read().await;

    runnable_checks
        .iter()
        .find(|&check| check.check_name == target_check)?;

    let last_check_result = client.query_one("SELECT timestamp,status,output from check_result where check_name = $1 order by timestamp desc limit 1", &[&target_check]).await.ok()?;
    let check_status: i16 = last_check_result.get("status");
    Some(Json(SimpleCheckResultDto {
        check_name: target_check.to_string(),
        output: last_check_result.get("output"),
        status: crate::check::CheckResultStatus::from(check_status),
        timestamp: last_check_result.get("timestamp"),
    }))
}
