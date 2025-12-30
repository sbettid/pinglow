use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use crate::{
    check::{Check, SharedPinglowChecks},
    config::PinglowConfig,
};
use chrono::{DateTime, FixedOffset, Utc};
use kube::Api;
use log::warn;
use pinglow_common::{CheckResult, CheckResultStatus, PinglowCheck, ScriptLanguage};
use rocket::{
    delete, get,
    http::Status,
    post, put,
    request::{FromRequest, Outcome},
    response::status,
    routes,
    serde::json::Json,
    Request, Rocket, Shutdown, State,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_postgres::Client;
use utoipa::{
    openapi::security::{ApiKeyValue, SecurityScheme},
    Modify, OpenApi, ToSchema,
};

pub async fn start_rocket(
    pinglow_config: PinglowConfig,
    shared_checks: SharedPinglowChecks,
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
            routes![
                get_checks,
                get_check_status,
                get_performance_data,
                mute_check,
                unmute_check,
                process_check_result
            ],
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

#[derive(Serialize, ToSchema, Debug)]
pub struct SimpleCheckDto {
    pub check_name: String,
    pub passive: bool,
    pub interval: Option<u64>,
    //pub language: Option<ScriptLanguage>,
}

impl From<&Arc<PinglowCheck>> for SimpleCheckDto {
    fn from(value: &Arc<PinglowCheck>) -> Self {
        Self {
            check_name: value.check_name.clone(),
            passive: value.passive,
            interval: value.interval,
            //language: value.as_ref().script.as_ref().map(|c| c.language.clone()),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct SimpleCheckResultDto {
    pub check_name: String,
    pub passive: bool,
    pub output: String,
    pub status: CheckResultStatus,
    pub timestamp: Option<DateTime<Utc>>,
    pub notifications_muted: Option<bool>,
    pub notifications_muted_until: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get,
    path = "/checks",
    responses(
        (status = 200, description = "List of checks", body = [SimpleCheckDto])
    )
)]
#[get("/checks")]
pub async fn get_checks(
    _key: ApiKey,
    checks: &State<SharedPinglowChecks>,
) -> Json<Vec<SimpleCheckDto>> {
    let runnable_checks = checks.read().await;

    let simple_checks_to_return: Vec<SimpleCheckDto> =
        runnable_checks.iter().map(|check| check.1.into()).collect();

    Json(simple_checks_to_return)
}

#[utoipa::path(
    get,
    path = "/check-status/{target_check}",
     params(
        ("target_check" = String, Path, description = "The check for which we would like to know the status")
    ),
    responses(
        (status = 200, description = "The last status of the check", body = [SimpleCheckResultDto])
    )
)]
#[get("/check-status/<target_check>")]
pub async fn get_check_status(
    _key: ApiKey,
    checks: &State<SharedPinglowChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
) -> Option<Json<SimpleCheckResultDto>> {
    let runnable_checks = checks.read().await;

    let (_, check) = runnable_checks
        .iter()
        .find(|&check| check.0 == target_check)?;

    let last_check_result_from_db = client.query_opt("SELECT timestamp,status,output from check_result where check_name = $1 order by timestamp desc limit 1", &[&target_check]).await.ok()?;

    let last_check_result = if let Some(last_check_result) = last_check_result_from_db {
        last_check_result
    } else {
        return Some(Json(SimpleCheckResultDto {
            check_name: target_check.to_string(),
            passive: check.passive,
            output: "Check still needs to be executed".to_owned(),
            status: CheckResultStatus::Pending,
            timestamp: None,
            notifications_muted: check.mute_notifications,
            notifications_muted_until: check.mute_notifications_until,
        }));
    };

    let check_status: i16 = last_check_result.get("status");
    Some(Json(SimpleCheckResultDto {
        check_name: target_check.to_string(),
        passive: check.passive,
        output: last_check_result.get("output"),
        status: CheckResultStatus::from(check_status),
        timestamp: last_check_result.get("timestamp"),
        notifications_muted: check.mute_notifications,
        notifications_muted_until: check.mute_notifications_until,
    }))
}

#[derive(Debug, Deserialize)]
struct GroupedPerfData {
    timestamp: DateTime<Utc>,
    perf_data: HashMap<String, f32>,
}

#[utoipa::path(
    get,
    path = "/performance-data/{target_check}",
     params(
        ("target_check" = String, Path, description = "The check for which we would like to get the performance data")
    ),
    responses(
        (status = 200, description = "The performance data of the check", body = [BTreeMap<DateTime<Utc>, HashMap<String, f32>>])
    )
)]
#[get("/performance-data/<target_check>")]
pub async fn get_performance_data(
    _key: ApiKey,
    checks: &State<SharedPinglowChecks>,
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
                pinglow_common::error::SerializeError::DeserializationError(e.to_string())
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

#[utoipa::path(
    put,
    path = "/check/{target_check}/mute?<until>",
     params(
        ("target_check" = String, Path, description = "The check we would like to mute")
    ),
    responses(
        (status = 200, description = "Whether the mute operation was successful")
    )
)]
#[put("/check/<target_check>/mute?<until>")]
pub async fn mute_check(
    _key: ApiKey,
    checks: &State<SharedPinglowChecks>,
    pinglow_config: &State<PinglowConfig>,
    target_check: &str,
    until: Option<String>,
) -> Result<(), status::Custom<String>> {
    // Read actual shared checks
    let mut runnable_checks = checks.write().await;

    // Ensure we can find the target check
    runnable_checks
        .iter()
        .find(|&check| check.0 == target_check)
        .ok_or(status::Custom(
            Status::NotFound,
            "Invalid target check".into(),
        ))?;

    // Prepare the patch object
    let mut patch = serde_json::json!({
        "spec": {
            "muteNotifications": true
        }
    });

    // If until is specified try to parse it and set it in the patch object
    let mut until_date_time: Option<DateTime<FixedOffset>> = None;
    if let Some(until) = until {
        match chrono::DateTime::parse_from_rfc3339(&until) {
            Ok(until) => {
                until_date_time = Some(until);
                if let Some(spec) = patch.get_mut("spec").and_then(Value::as_object_mut) {
                    spec.insert(
                        "muteNotificationsUntil".to_string(),
                        json!(until.to_rfc3339()),
                    );
                }
            }
            Err(e) => {
                return Err(status::Custom(
                    Status::BadRequest,
                    format!("Invalid datetime format: {e}"),
                ))
            }
        }
    }

    // Get the checks Kube Api
    let client = kube::Client::try_default().await.map_err(|e| {
        status::Custom(
            Status::InternalServerError,
            format!("Error retrieving the Kube client: {e}"),
        )
    })?;
    let checks_api: Api<Check> = Api::namespaced(client.clone(), &pinglow_config.target_namespace);

    checks_api
        .patch(
            target_check,
            &kube::api::PatchParams::apply("pinglow"),
            &kube::api::Patch::Merge(&patch),
        )
        .await
        .map_err(|e| {
            status::Custom(
                Status::InternalServerError,
                format!("Error setting mute status: {e}"),
            )
        })?;

    let check = runnable_checks.get(target_check);

    if let Some(check) = check {
        let mut modified_check = (**check).clone();
        modified_check.mute_notifications = Some(true);

        if let Some(until_date_time) = until_date_time {
            modified_check.mute_notifications_until = Some(until_date_time.into());
        }

        runnable_checks.insert(target_check.to_string(), Arc::new(modified_check));
    }

    Ok(())
}

#[utoipa::path(
    delete,
    path = "/check/{target_check}/mute",
     params(
        ("target_check" = String, Path, description = "The check we would like to unmute")
    ),
    responses(
        (status = 200, description = "Whether the unmute operation was successful")
    )
)]
#[delete("/check/<target_check>/mute")]
pub async fn unmute_check(
    _key: ApiKey,
    checks: &State<SharedPinglowChecks>,
    pinglow_config: &State<PinglowConfig>,
    target_check: &str,
) -> Result<(), status::Custom<String>> {
    // Read actual shared checks
    let mut runnable_checks = checks.write().await;

    // Ensure we can find the target check
    runnable_checks
        .iter()
        .find(|&check| check.0 == target_check)
        .ok_or(status::Custom(
            Status::NotFound,
            "Invalid target check".into(),
        ))?;

    // Prepare the patch object
    let patch = serde_json::json!({
        "spec": {
            "muteNotifications": false,
            "muteNotificationsUntil": null
        }
    });

    // Get the checks Kube Api
    let client = kube::Client::try_default().await.map_err(|e| {
        status::Custom(
            Status::InternalServerError,
            format!("Error retrieving the Kube client: {e}"),
        )
    })?;
    let checks_api: Api<Check> = Api::namespaced(client.clone(), &pinglow_config.target_namespace);

    checks_api
        .patch(
            target_check,
            &kube::api::PatchParams::apply("pinglow"),
            &kube::api::Patch::Merge(&patch),
        )
        .await
        .map_err(|e| {
            status::Custom(
                Status::InternalServerError,
                format!("Error setting unmute status: {e}"),
            )
        })?;

    let check = runnable_checks.get(target_check);

    if let Some(check) = check {
        let mut modified_check = (**check).clone();
        modified_check.mute_notifications = Some(false);
        modified_check.mute_notifications_until = None;

        runnable_checks.insert(target_check.to_string(), Arc::new(modified_check));
    }

    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProcessCheckResultPayload {
    output: String,
    status: i32,
}

#[utoipa::path(
    post,
    path = "/check/{target_check}/result",
     params(
        ("target_check" = String, Path, description = "The check for which we would like to send a result")
    ),
    responses(
        (status = 200, description = "Whether the processing of the check result was successful")
    )
)]
#[post("/check/<target_check>/result", data = "<check_result_payload>")]
pub async fn process_check_result(
    _key: ApiKey,
    checks: &State<SharedPinglowChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
    check_result_payload: Json<ProcessCheckResultPayload>,
) -> Result<(), status::Custom<String>> {
    // Read actual shared checks
    let runnable_checks = checks.read().await;

    // Ensure we can find the target check
    let (_check_name, check) = runnable_checks
        .iter()
        .find(|&check| check.0 == target_check)
        .ok_or(status::Custom(
            Status::NotFound,
            "Invalid target check".into(),
        ))?;

    // Extract the inner result object
    let check_result_payload = check_result_payload.into_inner();

    // Create the actual full check result
    let check_result: CheckResult = CheckResult {
        check_name: target_check.to_owned(),
        output: check_result_payload.output,
        status: check_result_payload.status.into(),
        timestamp: Some(Utc::now()),
        telegram_channels: check.telegram_channels.clone().into(),
        mute_notifications: check.mute_notifications,
        mute_notifications_until: check.mute_notifications_until,
    };

    let http_client = reqwest::Client::new();

    crate::process_check_result(check_result, client, &http_client)
        .await
        .map_err(|err| {
            status::Custom(
                Status::InternalServerError,
                format!("Error processing check result: {err}"),
            )
        })?;

    Ok(())
}

#[derive(OpenApi)]
#[openapi(
    paths(get_checks, get_check_status, get_performance_data, mute_check, unmute_check, process_check_result),
    components(schemas(
        SimpleCheckDto,
        SimpleCheckResultDto,
        CheckResultStatus,
        ScriptLanguage
    )),
    info(
        title = "Pinglow RestAPI",
        version = "1.0.0",
        license(name = "MIT", url = "https://opensource.org/licenses/MIT"),
        description = "The RestAPI to interact with Pinglow"
    ),
    modifiers(&SecurityAddon),
    security(
        ("api_key" = [])
    )
)]
pub struct ApiDoc;

// Add bearer auth security scheme
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.security_schemes.insert(
                "api_key".to_string(),
                SecurityScheme::ApiKey(utoipa::openapi::security::ApiKey::Header(
                    ApiKeyValue::new("x-api-key"),
                )),
            );
        }
    }
}
