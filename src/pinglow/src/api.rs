use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use crate::{
    auth::{self, AgentApiKey, Authenticated, Operator, SessionUser},
    check::{Check, SharedPinglowChecks},
    config::PinglowConfig,
    load_single_runnable_check, notification_mute_state,
    scheduler::enqueue_check,
};
use chrono::{DateTime, FixedOffset, Utc};
use kube::{Api, Client as KubeClient};
use log::warn;
use pinglow_common::{
    redis::{check_stream_for_target, redis_client},
    CheckResult, CheckResultStatus, PinglowCheck, ScriptLanguage,
};
use redis::{Client as RedisClient, Value as RedisValue};
use rocket::{
    delete, get, http::Status, post, put, response::status, routes, serde::json::Json, Rocket,
    Shutdown, State,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};
use tokio_postgres::Client;
use utoipa::{
    openapi::security::{ApiKeyValue, SecurityScheme},
    Modify, OpenApi, ToSchema,
};

pub async fn start_rocket(
    pinglow_config: PinglowConfig,
    shared_checks: SharedPinglowChecks,
    client: Arc<tokio_postgres::Client>,
) -> Result<(Rocket<rocket::Ignite>, Shutdown), Box<dyn std::error::Error + Send + Sync>> {
    let redis = redis_client()?;
    let auth_state = auth::create_auth_state(&pinglow_config, redis).await?;
    let gateway_state = AgentGatewayState {
        redis: auth_state.redis.clone(),
        next_target_index: Arc::new(AtomicUsize::new(0)),
    };
    tokio::spawn(auth::watch_bindings(
        pinglow_config.clone(),
        auth_state.clone(),
    ));
    tokio::spawn(auth::reconcile_api_keys(
        pinglow_config.clone(),
        auth_state.clone(),
    ));
    let figment = rocket::Config::figment()
        .merge(("address", "0.0.0.0"))
        .merge(("port", 8000));

    let rocket = rocket::custom(figment)
        .manage(pinglow_config)
        .manage(shared_checks)
        .manage(client)
        .manage(auth_state)
        .manage(gateway_state)
        .mount(
            "/",
            routes![
                auth::login,
                auth::callback,
                auth::me,
                auth::logout,
                get_checks,
                get_check_status,
                get_check_history,
                get_performance_data,
                mute_check,
                unmute_check,
                process_check_result,
                schedule_now,
                agent_next_job,
                agent_submit_result
            ],
        );

    let rocket = rocket.ignite().await?;

    let shutdown = rocket.shutdown();

    Ok((rocket, shutdown))
}

#[derive(Clone)]
struct AgentGatewayState {
    redis: RedisClient,
    next_target_index: Arc<AtomicUsize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentJob {
    id: String,
    target: String,
    expires_at: DateTime<Utc>,
    check: PinglowCheck,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentResultPayload {
    #[serde(default)]
    target: Option<String>,
    check_name: String,
    output: String,
    status: CheckResultStatus,
    timestamp: DateTime<Utc>,
}

#[post("/agent/jobs/next")]
async fn agent_next_job(
    agent: AgentApiKey,
    state: &State<AgentGatewayState>,
) -> Result<Json<AgentJob>, Status> {
    let streams: Vec<(String, String)> = agent
        .targets
        .iter()
        .map(|target| {
            let run_on = format!("agent/{target}");
            let stream = check_stream_for_target(&run_on).ok_or(Status::BadRequest)?;
            Ok((target.clone(), stream))
        })
        .collect::<Result<_, Status>>()?;
    if streams.is_empty() {
        return Err(Status::Forbidden);
    }

    let mut connection = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| Status::ServiceUnavailable)?;

    for (target, stream) in &streams {
        match redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(stream)
            .arg("agents")
            .arg("0")
            .arg("MKSTREAM")
            .query_async::<()>(&mut connection)
            .await
        {
            Ok(()) => {}
            Err(error) if error.to_string().contains("BUSYGROUP") => {}
            Err(error) => {
                log::error!("Failed to create stream group for {target}: {error}");
                return Err(Status::ServiceUnavailable);
            }
        }
    }

    let consumer = format!("agent-{}", agent.binding);
    let initial_target = state.next_target_index.fetch_add(1, Ordering::Relaxed) % streams.len();
    for offset in 0..streams.len() {
        let (target, stream) = &streams[(initial_target + offset) % streams.len()];
        if let Some((id, fields)) = claim_agent_job(
            &mut connection,
            stream,
            &consumer,
            &format!("agent/{target}"),
        )
        .await
        .map_err(|error| {
            log::error!("Failed to recover pending job for agent target {target}: {error}");
            Status::ServiceUnavailable
        })? {
            return agent_job_response(id, fields, target);
        }
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let start = state.next_target_index.fetch_add(1, Ordering::Relaxed) % streams.len();
        for offset in 0..streams.len() {
            let (target, stream) = &streams[(start + offset) % streams.len()];
            let response: Option<RedisValue> = redis::cmd("XREADGROUP")
                .arg("GROUP")
                .arg("agents")
                .arg(&consumer)
                .arg("COUNT")
                .arg(1)
                .arg("STREAMS")
                .arg(stream)
                .arg(">")
                .query_async(&mut connection)
                .await
                .map_err(|error| {
                    log::error!("Failed to read agent queue for {target}: {error}");
                    Status::ServiceUnavailable
                })?;
            if let Some(value) = response {
                let (id, fields) = pinglow_common::redis::parse_stream_payload(value)
                    .ok_or(Status::InternalServerError)?;
                return agent_job_response(id, fields, target);
            }
        }

        if Instant::now() >= deadline {
            return Err(Status::NoContent);
        }
        sleep(Duration::from_millis(200)).await;
    }
}

fn agent_job_response(
    id: String,
    fields: HashMap<String, String>,
    target: &str,
) -> Result<Json<AgentJob>, Status> {
    let payload = fields.get("payload").ok_or(Status::InternalServerError)?;
    let mut check: PinglowCheck =
        serde_json::from_str(payload).map_err(|_| Status::InternalServerError)?;
    if check.run_on != format!("agent/{target}") {
        return Err(Status::InternalServerError);
    }
    let expires_at = check.expires_at.ok_or(Status::InternalServerError)?;
    check.telegram_channels.clear();
    Ok(Json(AgentJob {
        id,
        target: target.to_owned(),
        expires_at,
        check,
    }))
}

async fn claim_agent_job(
    connection: &mut redis::aio::MultiplexedConnection,
    stream: &str,
    consumer: &str,
    target: &str,
) -> anyhow::Result<Option<(String, HashMap<String, String>)>> {
    let response: RedisValue = redis::cmd("XAUTOCLAIM")
        .arg(stream)
        .arg("agents")
        .arg(consumer)
        .arg(3660000)
        .arg("0-0")
        .arg("COUNT")
        .arg(1)
        .query_async(connection)
        .await?;
    let RedisValue::Array(parts) = response else {
        anyhow::bail!("Invalid XAUTOCLAIM response");
    };
    let Some(RedisValue::Array(entries)) = parts.get(1) else {
        anyhow::bail!("Invalid XAUTOCLAIM entries");
    };
    let Some(entry) = entries.first() else {
        return Ok(None);
    };
    let (id, fields) = parse_stream_entry(entry.clone())?;
    let payload = fields
        .get("payload")
        .ok_or_else(|| anyhow::anyhow!("Missing task payload"))?;
    let check: PinglowCheck = serde_json::from_str(payload)?;
    if check.run_on != target {
        anyhow::bail!("Recovered task does not belong to this agent");
    }
    Ok(Some((id, fields)))
}

fn parse_stream_entry(value: RedisValue) -> anyhow::Result<(String, HashMap<String, String>)> {
    let RedisValue::Array(entry) = value else {
        anyhow::bail!("Invalid Redis stream entry");
    };
    let Some(RedisValue::BulkString(id)) = entry.first() else {
        anyhow::bail!("Missing Redis stream entry id");
    };
    let id = String::from_utf8_lossy(id).into_owned();
    let Some(RedisValue::Array(fields)) = entry.get(1) else {
        anyhow::bail!("Missing Redis stream entry fields");
    };
    let mut map = HashMap::new();
    let mut iter = fields.iter();
    while let (Some(RedisValue::BulkString(key)), Some(RedisValue::BulkString(value))) =
        (iter.next(), iter.next())
    {
        map.insert(
            String::from_utf8_lossy(key).into_owned(),
            String::from_utf8_lossy(value).into_owned(),
        );
    }
    Ok((id, map))
}

#[post("/agent/jobs/<job_id>/result", data = "<payload>")]
async fn agent_submit_result(
    agent: AgentApiKey,
    state: &State<AgentGatewayState>,
    job_id: &str,
    payload: Json<AgentResultPayload>,
) -> Result<(), status::Custom<String>> {
    let payload = payload.into_inner();
    let target = resolve_agent_result_target(payload.target, &agent.targets).map_err(|status| {
        let message = match status {
            Status::BadRequest => "Result target is required for a multi-target agent",
            _ => "Agent is not authorized for this target",
        };
        status::Custom(status, message.to_owned())
    })?;
    let run_on = format!("agent/{target}");
    let stream = check_stream_for_target(&run_on)
        .ok_or_else(|| status::Custom(Status::BadRequest, "Invalid agent target".to_owned()))?;
    let mut connection = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|error| {
            status::Custom(
                Status::ServiceUnavailable,
                format!("Cannot connect to job queue: {error}"),
            )
        })?;
    let entries: RedisValue = redis::cmd("XRANGE")
        .arg(&stream)
        .arg(job_id)
        .arg(job_id)
        .query_async(&mut connection)
        .await
        .map_err(|error| status::Custom(Status::InternalServerError, error.to_string()))?;
    let RedisValue::Array(entries) = entries else {
        return Err(status::Custom(
            Status::NotFound,
            "Job does not exist".to_owned(),
        ));
    };
    let Some(entry) = entries.first() else {
        return Err(status::Custom(
            Status::NotFound,
            "Job does not exist".to_owned(),
        ));
    };
    let (_, fields) = parse_stream_entry(entry.clone())
        .map_err(|error| status::Custom(Status::InternalServerError, error.to_string()))?;
    let queued_check: PinglowCheck = fields
        .get("payload")
        .ok_or_else(|| status::Custom(Status::InternalServerError, "Job payload is missing".into()))
        .and_then(|value| {
            serde_json::from_str(value)
                .map_err(|error| status::Custom(Status::InternalServerError, error.to_string()))
        })?;
    if queued_check.run_on != run_on || queued_check.check_name != payload.check_name {
        return Err(status::Custom(
            Status::Forbidden,
            "Job does not belong to this agent or check".to_owned(),
        ));
    }

    let result = CheckResult {
        check_name: queued_check.check_name,
        output: payload.output.clone(),
        status: payload.status.clone(),
        timestamp: Some(payload.timestamp),
        telegram_channels: queued_check.telegram_channels.into(),
        mute_notifications: queued_check.mute_notifications,
        mute_notifications_until: queued_check.mute_notifications_until,
    };
    let serialized = serde_json::to_string(&result)
        .map_err(|error| status::Custom(Status::InternalServerError, error.to_string()))?;
    let script = r#"
        if redis.call('GET', KEYS[3]) then return 0 end
        local pending = redis.call('XPENDING', KEYS[1], ARGV[1], ARGV[2], ARGV[2], 1)
        if #pending == 0 then return -1 end
        local result_id = redis.call('XADD', KEYS[2], '*', 'payload', ARGV[3])
        redis.call('XACK', KEYS[1], ARGV[1], ARGV[2])
        redis.call('SET', KEYS[3], result_id, 'EX', 86400)
        return 1
    "#;
    let result_key = format!("pinglow:agent-result:{stream}:{job_id}");
    let outcome: i32 = redis::cmd("EVAL")
        .arg(script)
        .arg(3)
        .arg(&stream)
        .arg("pinglow:results")
        .arg(&result_key)
        .arg("agents")
        .arg(job_id)
        .arg(serialized)
        .query_async(&mut connection)
        .await
        .map_err(|error| status::Custom(Status::InternalServerError, error.to_string()))?;
    if outcome < 0 {
        return Err(status::Custom(
            Status::Conflict,
            "Job is not pending for this agent".to_owned(),
        ));
    }
    Ok(())
}

fn resolve_agent_result_target(
    target: Option<String>,
    allowed_targets: &[String],
) -> Result<String, Status> {
    let target = match target {
        Some(target) => target,
        None if allowed_targets.len() == 1 => allowed_targets[0].clone(),
        None => return Err(Status::BadRequest),
    };
    if allowed_targets.iter().any(|allowed| allowed == &target) {
        Ok(target)
    } else {
        Err(Status::Forbidden)
    }
}

#[derive(Serialize, ToSchema, Debug)]
pub struct SimpleCheckDto {
    pub check_name: String,
    pub passive: bool,
    pub interval: Option<u64>,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    //pub language: Option<ScriptLanguage>,
}

impl From<&Arc<PinglowCheck>> for SimpleCheckDto {
    fn from(value: &Arc<PinglowCheck>) -> Self {
        Self {
            check_name: value.check_name.clone(),
            passive: value.passive,
            interval: value.interval,
            group_id: value.group_id.clone(),
            group_name: value.group_name.clone(),
            //language: value.as_ref().script.as_ref().map(|c| c.language.clone()),
        }
    }
}

fn check_group_is_accessible(user: &SessionUser, group_id: Option<&str>) -> bool {
    user.can_access_check_group(group_id)
}

fn find_accessible_check<'a>(
    user: &SessionUser,
    checks: &'a HashMap<String, Arc<PinglowCheck>>,
    target_check: &str,
) -> Option<&'a Arc<PinglowCheck>> {
    checks
        .get(target_check)
        .filter(|check| check_group_is_accessible(user, check.group_id.as_deref()))
}

fn invalid_target_check() -> status::Custom<String> {
    status::Custom(Status::NotFound, "Invalid target check".to_owned())
}

#[derive(Serialize, ToSchema)]
pub struct SimpleCheckResultDto {
    pub check_name: String,
    pub passive: bool,
    pub run_on: String,
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
    user: Authenticated,
    checks: &State<SharedPinglowChecks>,
) -> Json<Vec<SimpleCheckDto>> {
    let runnable_checks = checks.read().await;

    let simple_checks_to_return: Vec<SimpleCheckDto> = runnable_checks
        .values()
        .filter(|check| check_group_is_accessible(&user.0, check.group_id.as_deref()))
        .map(|check| check.into())
        .collect();

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
    user: Authenticated,
    checks: &State<SharedPinglowChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
) -> Option<Json<SimpleCheckResultDto>> {
    let runnable_checks = checks.read().await;

    let check = find_accessible_check(&user.0, &runnable_checks, target_check)?;

    let notifications_muted = notification_mute_state(
        check.mute_notifications,
        check.mute_notifications_until,
        Utc::now(),
    );

    let last_check_result_from_db = client.query_opt("SELECT timestamp,status,output from check_result where check_name = $1 order by timestamp desc limit 1", &[&target_check]).await.ok()?;

    let last_check_result = if let Some(last_check_result) = last_check_result_from_db {
        last_check_result
    } else {
        return Some(Json(SimpleCheckResultDto {
            check_name: target_check.to_string(),
            passive: check.passive,
            run_on: check.run_on.clone(),
            output: "Check still needs to be executed".to_owned(),
            status: CheckResultStatus::Pending,
            timestamp: None,
            notifications_muted,
            notifications_muted_until: check.mute_notifications_until,
        }));
    };

    let check_status: i16 = last_check_result.get("status");
    Some(Json(SimpleCheckResultDto {
        check_name: target_check.to_string(),
        passive: check.passive,
        run_on: check.run_on.clone(),
        output: last_check_result.get("output"),
        status: CheckResultStatus::from(check_status),
        timestamp: last_check_result.get("timestamp"),
        notifications_muted,
        notifications_muted_until: check.mute_notifications_until,
    }))
}

#[derive(Serialize, ToSchema)]
pub struct CheckHistoryEntryDto {
    pub timestamp: DateTime<Utc>,
    pub status: CheckResultStatus,
    pub output: String,
}

#[derive(Serialize, ToSchema)]
pub struct CheckHistoryPageDto {
    #[schema(minimum = 1)]
    pub page: u32,
    #[schema(minimum = 1)]
    pub size: u32,
    pub total: i64,
    pub history: Vec<CheckHistoryEntryDto>,
}

fn validate_history_pagination(
    page: Option<u32>,
    size: Option<u32>,
) -> Result<(u32, u32, i64), status::Custom<String>> {
    let page = page.unwrap_or(1);
    let size = size.unwrap_or(10);

    if page == 0 || size == 0 {
        return Err(status::Custom(
            Status::BadRequest,
            "page and size must be greater than zero".to_owned(),
        ));
    }

    let offset = (i64::from(page) - 1)
        .checked_mul(i64::from(size))
        .ok_or(status::Custom(
            Status::BadRequest,
            "Requested page is too large".to_owned(),
        ))?;

    Ok((page, size, offset))
}

const STATE_CHANGE_HISTORY_CTE: &str = "WITH ordered AS (
    SELECT timestamp, status, output,
           LEAD(status) OVER (ORDER BY timestamp DESC) AS older_status
    FROM check_result
    WHERE check_name = $1
)";

const STATE_CHANGE_FILTER: &str = "older_status IS NULL OR status <> older_status";

#[utoipa::path(
    get,
    path = "/check-history/{target_check}",
    params(
        ("target_check" = String, Path, description = "The check whose status history is requested"),
        ("page" = Option<u32>, Query, description = "1-based page number", minimum = 1, example = 1),
        ("size" = Option<u32>, Query, description = "Number of history entries per page (defaults to 10)", minimum = 1, example = 10),
        ("state_changes" = Option<bool>, Query, description = "When true, return only the newest result in each consecutive run of the same status", example = true)
    ),
    responses(
        (status = 200, description = "Paginated status history for the check", body = CheckHistoryPageDto),
        (status = 400, description = "Invalid pagination parameters"),
        (status = 404, description = "The check does not exist"),
        (status = 500, description = "Failed to retrieve check history")
    )
)]
#[get("/check-history/<target_check>?<page>&<size>&<state_changes>")]
pub async fn get_check_history(
    user: Authenticated,
    checks: &State<SharedPinglowChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
    page: Option<u32>,
    size: Option<u32>,
    state_changes: Option<bool>,
) -> Result<Json<CheckHistoryPageDto>, status::Custom<String>> {
    let (page, size, offset) = validate_history_pagination(page, size)?;
    let state_changes = state_changes.unwrap_or(false);

    let runnable_checks = checks.read().await;
    let _check = find_accessible_check(&user.0, &runnable_checks, target_check)
        .cloned()
        .ok_or_else(invalid_target_check)?;
    drop(runnable_checks);

    let total_query = if state_changes {
        format!(
            "{STATE_CHANGE_HISTORY_CTE}
            SELECT COUNT(*) FROM ordered WHERE {STATE_CHANGE_FILTER}"
        )
    } else {
        "SELECT COUNT(*) FROM check_result WHERE check_name = $1".to_owned()
    };
    let total: i64 = client
        .query_one(&total_query, &[&target_check])
        .await
        .map_err(|e| {
            status::Custom(
                Status::InternalServerError,
                format!("Error retrieving check history count: {e}"),
            )
        })?
        .get(0);

    let history_query = if state_changes {
        format!(
            "{STATE_CHANGE_HISTORY_CTE}
            SELECT timestamp, status, output FROM ordered
            WHERE {STATE_CHANGE_FILTER}
            ORDER BY timestamp DESC LIMIT $2 OFFSET $3"
        )
    } else {
        "SELECT timestamp, status, output FROM check_result
        WHERE check_name = $1
        ORDER BY timestamp DESC LIMIT $2 OFFSET $3"
            .to_owned()
    };
    let rows = client
        .query(&history_query, &[&target_check, &i64::from(size), &offset])
        .await
        .map_err(|e| {
            status::Custom(
                Status::InternalServerError,
                format!("Error retrieving check history: {e}"),
            )
        })?;

    let history = rows
        .into_iter()
        .map(|row| {
            let status: i16 = row.get("status");
            CheckHistoryEntryDto {
                timestamp: row.get("timestamp"),
                status: CheckResultStatus::from(status),
                output: row.get("output"),
            }
        })
        .collect();

    Ok(Json(CheckHistoryPageDto {
        page,
        size,
        total,
        history,
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
    user: Authenticated,
    checks: &State<SharedPinglowChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
) -> Option<Json<BTreeMap<DateTime<Utc>, HashMap<String, f32>>>> {
    let runnable_checks = checks.read().await;

    let _check = find_accessible_check(&user.0, &runnable_checks, target_check)?;

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
    user: Operator,
    checks: &State<SharedPinglowChecks>,
    pinglow_config: &State<PinglowConfig>,
    target_check: &str,
    until: Option<String>,
) -> Result<(), status::Custom<String>> {
    // Read actual shared checks
    let mut runnable_checks = checks.write().await;

    // Ensure we can find the target check
    let _check = find_accessible_check(&user.0, &runnable_checks, target_check)
        .ok_or_else(invalid_target_check)?;

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
    user: Operator,
    checks: &State<SharedPinglowChecks>,
    pinglow_config: &State<PinglowConfig>,
    target_check: &str,
) -> Result<(), status::Custom<String>> {
    // Read actual shared checks
    let mut runnable_checks = checks.write().await;

    // Ensure we can find the target check
    let _check = find_accessible_check(&user.0, &runnable_checks, target_check)
        .ok_or_else(invalid_target_check)?;

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
    #[serde(default)]
    image_jpg_base64: Option<String>, // Base64 image
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
    user: Operator,
    checks: &State<SharedPinglowChecks>,
    client: &State<Arc<Client>>,
    target_check: &str,
    check_result_payload: Json<ProcessCheckResultPayload>,
) -> Result<(), status::Custom<String>> {
    // Read actual shared checks
    let runnable_checks = checks.read().await;

    // Ensure we can find the target check
    let check = find_accessible_check(&user.0, &runnable_checks, target_check)
        .cloned()
        .ok_or_else(invalid_target_check)?;

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

    crate::process_check_result(
        check_result,
        check_result_payload.image_jpg_base64,
        client,
        &http_client,
    )
    .await
    .map_err(|err| {
        status::Custom(
            Status::InternalServerError,
            format!("Error processing check result: {err}"),
        )
    })?;

    Ok(())
}

#[utoipa::path(
    post,
    path = "/check/{target_check}/schedule-now",
     params(
        ("target_check" = String, Path, description = "The check we would like to schedule now")
    ),
    responses(
        (status = 200, description = "Whether the immediate scheduling operation was successful")
    )
)]
#[post("/check/<target_check>/schedule-now")]
pub async fn schedule_now(
    user: Operator,
    pinglow_config: &State<PinglowConfig>,
    target_check: &str,
) -> Result<(), status::Custom<String>> {
    let client = KubeClient::try_default().await.map_err(|e| {
        status::Custom(
            Status::InternalServerError,
            format!("Error retrieving the Kube client: {e}"),
        )
    })?;
    let checks_api: Api<Check> = Api::namespaced(client.clone(), &pinglow_config.target_namespace);
    let definition = checks_api.get(target_check).await.map_err(|e| {
        status::Custom(
            if matches!(e, kube::Error::Api(ref response) if response.code == 404) {
                Status::NotFound
            } else {
                Status::InternalServerError
            },
            format!("Error retrieving check {target_check}: {e}"),
        )
    })?;
    if !check_group_is_accessible(&user.0, definition.spec.groupRef.as_deref()) {
        return Err(invalid_target_check());
    }
    let check = load_single_runnable_check(&definition, &client, pinglow_config)
        .await
        .map(Arc::new)
        .map_err(|e| {
            status::Custom(
                Status::InternalServerError,
                format!("Error resolving check {target_check}: {e}"),
            )
        })?;

    // Get redis client to enqueue the check
    let redis_client = redis_client().map_err(|e| {
        status::Custom(
            Status::InternalServerError,
            format!("Error retrieving the redis client: {e}"),
        )
    })?;

    let mut redis_conn = redis_client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| {
            status::Custom(
                Status::InternalServerError,
                format!("Error retrieving the connection to redis: {e}"),
            )
        })?;

    redis_conn.set_response_timeout(Duration::from_secs(30));

    enqueue_check(&mut redis_conn, &check, pinglow_config.redis_stream_max_len)
        .await
        .map_err(|e| {
            status::Custom(
                Status::InternalServerError,
                format!("Error adding check to run queue: {e}"),
            )
        })?;

    Ok(())
}

#[derive(OpenApi)]
#[openapi(
    paths(get_checks, get_check_status, get_check_history, get_performance_data, mute_check, unmute_check, process_check_result, schedule_now),
    components(schemas(
        SimpleCheckDto,
        SimpleCheckResultDto,
        CheckHistoryEntryDto,
        CheckHistoryPageDto,
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
                ))
                .into(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::CheckGroupAccess;
    use pinglow_common::CheckResultStatus;

    #[test]
    fn agent_results_must_name_an_authorized_target() {
        let allowed = vec!["nas".to_owned(), "linux-hosts".to_owned()];
        assert_eq!(
            resolve_agent_result_target(Some("linux-hosts".to_owned()), &allowed),
            Ok("linux-hosts".to_owned())
        );
        assert_eq!(
            resolve_agent_result_target(Some("other".to_owned()), &allowed),
            Err(Status::Forbidden)
        );
        assert_eq!(
            resolve_agent_result_target(None, &allowed),
            Err(Status::BadRequest)
        );
        assert_eq!(
            resolve_agent_result_target(None, &["nas".to_owned()]),
            Ok("nas".to_owned())
        );
    }

    fn test_check(check_name: &str, group_id: Option<&str>) -> Arc<PinglowCheck> {
        Arc::new(PinglowCheck {
            passive: true,
            script: None,
            collector: None,
            interval: None,
            check_name: check_name.to_owned(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: group_id.map(str::to_owned),
            group_name: group_id.map(str::to_owned),
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        })
    }

    #[test]
    fn test_find_accessible_check_applies_group_scope() {
        let user = SessionUser {
            user: "viewer@example.com".to_owned(),
            role: pinglow_common::UserRole::Viewer,
            check_groups: CheckGroupAccess::Included(vec!["home".to_owned()]),
        };
        let checks = HashMap::from([
            (
                "home-check".to_owned(),
                test_check("home-check", Some("home")),
            ),
            (
                "service-check".to_owned(),
                test_check("service-check", Some("services")),
            ),
            (
                "ungrouped-check".to_owned(),
                test_check("ungrouped-check", None),
            ),
        ]);

        assert!(find_accessible_check(&user, &checks, "home-check").is_some());
        assert!(find_accessible_check(&user, &checks, "service-check").is_none());
        assert!(find_accessible_check(&user, &checks, "ungrouped-check").is_none());
        assert!(find_accessible_check(&user, &checks, "missing-check").is_none());
    }

    #[test]
    fn test_simple_check_dto_creation() {
        let check = PinglowCheck {
            passive: true,
            script: None,
            collector: None,
            interval: Some(300),
            check_name: "test-check".to_string(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: Some("home".to_string()),
            group_name: Some("Home".to_string()),
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        };

        let dto = SimpleCheckDto::from(&Arc::new(check));
        assert_eq!(dto.check_name, "test-check");
        assert!(dto.passive);
        assert_eq!(dto.interval, Some(300));
        assert_eq!(dto.group_id.as_deref(), Some("home"));
        assert_eq!(dto.group_name.as_deref(), Some("Home"));
    }

    #[test]
    fn test_simple_check_dto_non_passive() {
        let check = PinglowCheck {
            passive: false,
            script: None,
            collector: None,
            interval: Some(60),
            check_name: "active-check".to_string(),
            run_on: "cluster".to_owned(),
            expires_at: None,
            group_id: None,
            group_name: None,
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        };

        let dto = SimpleCheckDto::from(&Arc::new(check));
        assert!(!dto.passive);
        assert_eq!(dto.interval, Some(60));
    }

    #[test]
    fn check_status_response_includes_execution_target() {
        let response = SimpleCheckResultDto {
            check_name: "group-check".to_owned(),
            passive: false,
            run_on: "agent/linux-hosts".to_owned(),
            output: "OK".to_owned(),
            status: CheckResultStatus::Ok,
            timestamp: None,
            notifications_muted: None,
            notifications_muted_until: None,
        };

        assert_eq!(
            serde_json::to_value(response).unwrap()["run_on"],
            "agent/linux-hosts"
        );
    }

    #[test]
    fn test_history_pagination_defaults_and_offset() {
        assert_eq!(validate_history_pagination(None, None).unwrap(), (1, 10, 0));
        assert_eq!(
            validate_history_pagination(Some(3), Some(25)).unwrap(),
            (3, 25, 50)
        );
    }

    #[test]
    fn test_history_pagination_rejects_zero_values_and_overflow() {
        assert_eq!(
            validate_history_pagination(Some(0), None).unwrap_err().0,
            Status::BadRequest
        );
        assert_eq!(
            validate_history_pagination(None, Some(0)).unwrap_err().0,
            Status::BadRequest
        );
        assert_eq!(
            validate_history_pagination(Some(u32::MAX), Some(u32::MAX))
                .unwrap_err()
                .0,
            Status::BadRequest
        );
        assert_eq!(
            validate_history_pagination(Some(50_000), Some(100_000))
                .unwrap()
                .2,
            4_999_900_000
        );
    }

    #[test]
    fn test_state_change_history_query_uses_status_transition_window() {
        assert!(STATE_CHANGE_HISTORY_CTE
            .contains("LEAD(status) OVER (ORDER BY timestamp DESC) AS older_status"));
        assert_eq!(
            STATE_CHANGE_FILTER,
            "older_status IS NULL OR status <> older_status"
        );
    }

    #[test]
    fn test_history_endpoint_openapi_documents_state_changes_filter() {
        let openapi = serde_json::to_value(ApiDoc::openapi()).unwrap();
        let parameters = openapi["paths"]["/check-history/{target_check}"]["get"]["parameters"]
            .as_array()
            .unwrap();
        let state_changes = parameters
            .iter()
            .find(|parameter| parameter["name"] == "state_changes")
            .expect("state_changes query parameter should be documented");

        assert_eq!(state_changes["in"], "query");
        assert_eq!(state_changes["schema"]["type"], "boolean");
        assert!(state_changes["description"]
            .as_str()
            .unwrap()
            .contains("newest result in each consecutive run"));
    }

    #[test]
    fn test_check_result_status_ok() {
        assert_eq!(CheckResultStatus::Ok, CheckResultStatus::Ok);
        assert_ne!(CheckResultStatus::Ok, CheckResultStatus::Warning);
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
        assert_eq!(
            CheckResultStatus::from(99i32),
            CheckResultStatus::CheckError
        );
    }

    #[test]
    fn test_user_role_permissions() {
        use pinglow_common::UserRole;

        let viewer = UserRole::Viewer;
        let operator = UserRole::Operator;
        let admin = UserRole::Admin;

        assert_ne!(viewer, operator);
        assert_ne!(operator, admin);
        assert_ne!(viewer, admin);
    }
}
