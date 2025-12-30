CREATE TABLE IF NOT EXISTS "check_result" (
    id SERIAL,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    check_name TEXT NOT NULL,
    status SMALLINT NOT NULL, 
    output TEXT NOT NULL,
    PRIMARY KEY (check_name, timestamp)
);
SELECT create_hypertable('check_result', 'timestamp', if_not_exists => TRUE);
ALTER TABLE "check_result" SET (timescaledb.compress, timescaledb.compress_orderby = 'timestamp DESC');
SELECT add_retention_policy('check_result', INTERVAL '7 days');