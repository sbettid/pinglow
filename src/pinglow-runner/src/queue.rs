use anyhow::Error;
use pinglow_common::redis::parse_stream_payload;
use pinglow_common::PinglowCheck;
use redis::aio::MultiplexedConnection;
use redis::Value;

pub async fn fetch_task(
    conn: &mut MultiplexedConnection,
    runner_name: &str,
) -> Result<Option<(String, PinglowCheck)>, Error> {
    let res: Option<Value> = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg("workers")
        .arg(runner_name) // consumer name
        .arg("BLOCK")
        .arg(0) // block indefinitely
        .arg("COUNT")
        .arg(1) // fetch one message at a time
        .arg("STREAMS")
        .arg("pinglow:checks")
        .arg(">") // fetch only new messages
        .query_async(conn)
        .await?;

    let Some(value) = res else {
        return Ok(None);
    };

    let (id, fields) = parse_stream_payload(value).ok_or(
        pinglow_common::error::SerializeError::DeserializationError(
            "Cannot extract id and fields from redis message".into(),
        ),
    )?;

    let payload = fields.get("payload").ok_or(
        pinglow_common::error::SerializeError::DeserializationError(
            "The expected payload field was not found".into(),
        ),
    )?;

    let check: PinglowCheck = serde_json::from_str(payload)?;

    Ok(Some((id, check)))
}
