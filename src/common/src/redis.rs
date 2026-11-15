use std::collections::HashMap;

use redis::{Client as RedisClient, RedisError, Value};

pub fn redis_client() -> Result<RedisClient, RedisError> {
    let host = std::env::var("REDIS_HOST").expect("REDIS_HOST must be set");
    let password = std::env::var("REDIS_PASSWORD").expect("REDIS_PASSWORD must be set");
    RedisClient::open(format!("redis://:{password}@{host}:6379"))
}

pub fn check_stream_for_target(run_on: &str) -> Option<String> {
    if run_on == "cluster" {
        return Some("pinglow:checks".to_owned());
    }

    let target = run_on.strip_prefix("agent/")?;
    if target.is_empty()
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(format!("pinglow:checks:agent:{target}"))
}

pub async fn init_streams(conn: &mut redis::aio::MultiplexedConnection) {
    // Tasks stream
    let _: Result<(), _> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg("pinglow:checks")
        .arg("workers")
        .arg("0")
        .arg("MKSTREAM")
        .query_async(conn)
        .await;

    // Results stream
    let _: Result<(), _> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg("pinglow:results")
        .arg("controller")
        .arg("0")
        .arg("MKSTREAM")
        .query_async(conn)
        .await;
}

pub fn parse_stream_payload(value: Value) -> Option<(String, HashMap<String, String>)> {
    let Value::Array(streams) = value else {
        return None;
    };

    // Only one stream expected
    let Value::Array(stream) = streams.into_iter().next()? else {
        return None;
    };

    // stream = [stream_name, entries]
    let entries = stream.into_iter().nth(1)?;

    let Value::Array(entries) = entries else {
        return None;
    };
    let Value::Array(entry) = entries.into_iter().next()? else {
        return None;
    };

    // entry = [id, fields]

    let Value::BulkString(id_bytes) = entry.first()? else {
        return None;
    };
    let id = String::from_utf8_lossy(id_bytes).into();

    let Value::Array(fields) = entry.into_iter().nth(1)? else {
        return None;
    };

    let mut map = HashMap::new();
    let mut it = fields.into_iter();

    while let (Some(Value::BulkString(k)), Some(Value::BulkString(v))) = (it.next(), it.next()) {
        map.insert(
            String::from_utf8_lossy(&k).into(),
            String::from_utf8_lossy(&v).into(),
        );
    }

    Some((id, map))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(value: &str) -> Value {
        Value::BulkString(value.as_bytes().to_vec())
    }

    #[test]
    fn parses_first_stream_entry_and_fields() {
        let value = Value::Array(vec![Value::Array(vec![
            bulk("pinglow:checks"),
            Value::Array(vec![Value::Array(vec![
                bulk("1712345678901-0"),
                Value::Array(vec![bulk("payload"), bulk("{\"check_name\":\"test\"}")]),
            ])]),
        ])]);

        let (id, fields) = parse_stream_payload(value).unwrap();

        assert_eq!(id, "1712345678901-0");
        assert_eq!(
            fields.get("payload").map(String::as_str),
            Some("{\"check_name\":\"test\"}")
        );
    }

    #[test]
    fn returns_none_for_empty_or_malformed_stream_response() {
        assert_eq!(parse_stream_payload(Value::Array(vec![])), None);
        assert_eq!(
            parse_stream_payload(Value::Array(vec![Value::SimpleString(
                "unexpected".to_owned()
            )])),
            None
        );
    }

    #[test]
    fn ignores_incomplete_trailing_field_pair() {
        let value = Value::Array(vec![Value::Array(vec![
            bulk("pinglow:checks"),
            Value::Array(vec![Value::Array(vec![
                bulk("1-0"),
                Value::Array(vec![bulk("payload"), bulk("serialized"), bulk("orphan")]),
            ])]),
        ])]);

        let (id, fields) = parse_stream_payload(value).unwrap();

        assert_eq!(id, "1-0");
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields.get("payload").map(String::as_str),
            Some("serialized")
        );
    }

    #[test]
    fn routes_only_valid_execution_targets() {
        assert_eq!(
            check_stream_for_target("cluster").as_deref(),
            Some("pinglow:checks")
        );
        assert_eq!(
            check_stream_for_target("agent/nas-1").as_deref(),
            Some("pinglow:checks:agent:nas-1")
        );
        assert_eq!(check_stream_for_target("agent/"), None);
        assert_eq!(check_stream_for_target("agent/nas_1"), None);
        assert_eq!(check_stream_for_target("other"), None);
    }
}
