use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use log::warn;
use rocket::{
    get,
    http::Status,
    request::{FromRequest, Outcome},
    routes,
    serde::json::Json,
    Request, Rocket, Shutdown, State,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_postgres::Client;

use crate::{
    check::{CheckResultStatus, RunnableCheck, ScriptLanguage, SharedChecks},
    config::PinglowConfig,
    error,
};

pub async fn start_rocket(
    pinglow_config: PinglowConfig,
    shared_checks: SharedChecks,
    client: Arc<tokio_postgres::Client>,
) -> Result<(Rocket<rocket::Ignite>, Shutdown), rocket::Error> {
    let figment = rocket::Config::figment()
        .merge(("address", "0.0.0.0"))
        .merge(("port", 8000));

    let rocket = rocket::custom(figment)
        .manage(pinglow_config)
        .manage(shared_checks)
        .manage(client)
        .mount(
            "/",
            routes![get_checks, get_check_status, get_performance_data],
        );

    let rocket = rocket.ignite().await?;

    let shutdown = rocket.shutdown();

    Ok((rocket, shutdown))
}

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

impl From<&Arc<RunnableCheck>> for SimpleCheckDto {
    fn from(value: &Arc<RunnableCheck>) -> Self {
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

    let simple_checks_to_return: Vec<SimpleCheckDto> =
        runnable_checks.iter().map(|check| check.1.into()).collect();

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
        .find(|&check| check.0 == target_check)?;

    let last_check_result = client.query_one("SELECT timestamp,status,output from check_result where check_name = $1 order by timestamp desc limit 1", &[&target_check]).await.ok()?;
    let check_status: i16 = last_check_result.get("status");
    Some(Json(SimpleCheckResultDto {
        check_name: target_check.to_string(),
        output: last_check_result.get("output"),
        status: crate::check::CheckResultStatus::from(check_status),
        timestamp: last_check_result.get("timestamp"),
    }))
}

#[derive(Debug, Deserialize)]
struct GroupedPerfData {
    timestamp: DateTime<Utc>,
    perf_data: HashMap<String, f32>,
}

#[get("/performance-data/<target_check>")]
pub async fn get_performance_data(
    _key: ApiKey,
    checks: &State<SharedChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
) -> Option<Json<BTreeMap<DateTime<Utc>, HashMap<String, f32>>>> {
    let runnable_checks = checks.read().await;

    runnable_checks
        .iter()
        .find(|&check| check.0 == target_check)?;

    let raw_perf_data_rows = client.query("SELECT timestamp, json_object_agg(perf_key, perf_value ORDER BY perf_key) AS perf_data FROM check_result_perf_data WHERE check_name = $1 GROUP BY timestamp ORDER BY timestamp;", &[&target_check]).await.ok()?;

    let mut perf_data = Vec::new();

    for raw_perf_data in raw_perf_data_rows {
        let timestamp: chrono::DateTime<Utc> = raw_perf_data.get("timestamp");
        let perf_data_json: Value = raw_perf_data.get("perf_data");

        // Convert JSON object to HashMap<String, f32>
        let perf_data_map: HashMap<String, f32> = serde_json::from_value(perf_data_json)
            .map_err(|e| {
                warn!("Failed to parse JSON perf_data: {e}");
                error::TimescaleDBConversionError::DeserializationError(e.to_string())
            })
            .ok()?;

        perf_data.push(GroupedPerfData {
            timestamp,
            perf_data: perf_data_map,
        });
    }

    let map: BTreeMap<_, _> = perf_data
        .into_iter()
        .map(|entry| (entry.timestamp, entry.perf_data))
        .collect();

    Some(Json(map))
}
