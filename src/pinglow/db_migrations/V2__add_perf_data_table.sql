CREATE TABLE IF NOT EXISTS "check_result_perf_data" (
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    check_name TEXT NOT NULL, 
    perf_key TEXT NOT NULL,
    perf_value REAL NOT NULL,
    PRIMARY KEY (timestamp, check_name, perf_key)
);
SELECT create_hypertable('check_result_perf_data', 'timestamp', if_not_exists => TRUE);
ALTER TABLE "check_result_perf_data" SET (timescaledb.compress, timescaledb.compress_orderby = 'timestamp DESC');
SELECT add_retention_policy('check_result_perf_data', INTERVAL '7 days');