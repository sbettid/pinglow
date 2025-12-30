use std::collections::HashMap;

use log::info;
use redis::{Client as RedisClient, RedisError, Value};

pub fn redis_client() -> Result<RedisClient, RedisError> {
    let host = std::env::var("REDIS_HOST").expect("REDIS_HOST must be set");
    let password = std::env::var("REDIS_PASSWORD").expect("REDIS_PASSWORD must be set");
    info!("Creating redis client redis://:{password}@{host}:6379");
    RedisClient::open(format!("redis://:{password}@{host}:6379"))
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
