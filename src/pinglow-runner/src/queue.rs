use anyhow::Error;
use pinglow_common::redis::parse_stream_payload;
use pinglow_common::PinglowCheck;
use redis::aio::MultiplexedConnection;
use redis::Value;

pub async fn fetch_task(
    conn: &mut MultiplexedConnection,
    runner_name: &str,
    task_claim_idle_ms: u64,
) -> Result<Option<(String, PinglowCheck)>, Error> {
    // Recover tasks left pending by a runner that crashed or was scaled down.
    let claimed: Value = redis::cmd("XAUTOCLAIM")
        .arg("pinglow:checks")
        .arg("workers")
        .arg(runner_name)
        .arg(task_claim_idle_ms)
        .arg("0-0")
        .arg("COUNT")
        .arg(1)
        .query_async(conn)
        .await?;

    if let Some(task) = parse_autoclaim_payload(claimed)? {
        return Ok(Some(task));
    }

    let res: Option<Value> = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg("workers")
        .arg(runner_name) // consumer name
        .arg("BLOCK")
        .arg(15000)
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

fn parse_autoclaim_payload(value: Value) -> Result<Option<(String, PinglowCheck)>, Error> {
    let Value::Array(mut response) = value else {
        return Err(anyhow::anyhow!("Invalid XAUTOCLAIM response"));
    };

    // XAUTOCLAIM returns [next_start_id, [[id, fields]]].
    let entries = response
        .pop()
        .ok_or_else(|| anyhow::anyhow!("Missing entries in XAUTOCLAIM response"))?;
    let Value::Array(mut entries) = entries else {
        return Err(anyhow::anyhow!("Invalid entries in XAUTOCLAIM response"));
    };

    let Some(Value::Array(entry)) = entries.pop() else {
        return Ok(None);
    };

    let id = match entry.first() {
        Some(Value::BulkString(id)) => String::from_utf8_lossy(id).into_owned(),
        _ => return Err(anyhow::anyhow!("Invalid entry id in XAUTOCLAIM response")),
    };
    let fields = entry
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("Missing fields in XAUTOCLAIM response"))?;
    let Value::Array(fields) = fields else {
        return Err(anyhow::anyhow!("Invalid fields in XAUTOCLAIM response"));
    };

    let mut payload = None;
    let mut fields = fields.iter();
    while let (Some(Value::BulkString(key)), Some(Value::BulkString(value))) =
        (fields.next(), fields.next())
    {
        if key == b"payload" {
            payload = Some(String::from_utf8_lossy(value).into_owned());
        }
    }

    let payload = payload.ok_or_else(|| anyhow::anyhow!("Missing payload field"))?;
    Ok(Some((id, serde_json::from_str(&payload)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(value: &str) -> Value {
        Value::BulkString(value.as_bytes().to_vec())
    }

    fn check_payload() -> String {
        serde_json::to_string(&PinglowCheck {
            passive: false,
            script: None,
            interval: Some(60),
            check_name: "check-a".to_owned(),
            secrets: None,
            telegram_channels: vec![],
            mute_notifications: None,
            mute_notifications_until: None,
        })
        .unwrap()
    }

    #[test]
    fn parses_autoclaimed_check_payload() {
        let payload = check_payload();
        let value = Value::Array(vec![
            bulk("0-0"),
            Value::Array(vec![Value::Array(vec![
                bulk("1712345678901-0"),
                Value::Array(vec![bulk("payload"), bulk(&payload)]),
            ])]),
        ]);

        let (id, check) = parse_autoclaim_payload(value).unwrap().unwrap();

        assert_eq!(id, "1712345678901-0");
        assert_eq!(check.check_name, "check-a");
        assert_eq!(check.interval, Some(60));
    }

    #[test]
    fn returns_none_when_autoclaim_contains_no_entries() {
        let value = Value::Array(vec![bulk("0-0"), Value::Array(vec![])]);

        assert!(parse_autoclaim_payload(value).unwrap().is_none());
    }

    #[test]
    fn rejects_autoclaim_with_missing_payload() {
        let value = Value::Array(vec![
            bulk("0-0"),
            Value::Array(vec![Value::Array(vec![
                bulk("1712345678901-0"),
                Value::Array(vec![bulk("other-field"), bulk("value")]),
            ])]),
        ]);

        let error = parse_autoclaim_payload(value).unwrap_err();

        assert!(error.to_string().contains("Missing payload field"));
    }

    #[test]
    fn rejects_autoclaim_with_invalid_check_json() {
        let value = Value::Array(vec![
            bulk("0-0"),
            Value::Array(vec![Value::Array(vec![
                bulk("1712345678901-0"),
                Value::Array(vec![bulk("payload"), bulk("{}")]),
            ])]),
        ]);

        assert!(parse_autoclaim_payload(value).is_err());
    }
}
