use log::info;
use tokio_postgres::Client as PostgresClient;

use crate::config::PinglowConfig;

/// Apply retention policies to TimescaleDB hypertables based on configuration.
/// This function removes any existing retention policies and applies new ones
/// according to the values in PinglowConfig.
pub async fn apply_retention_policies(
    postgres_client: &PostgresClient,
    config: &PinglowConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Applying retention policies to TimescaleDB");

    // Remove existing retention policies (idempotent operation)
    postgres_client
        .execute(
            "SELECT remove_retention_policy('check_result', if_exists => TRUE);",
            &[],
        )
        .await?;
    postgres_client
        .execute(
            "SELECT remove_retention_policy('check_result_perf_data', if_exists => TRUE);",
            &[],
        )
        .await?;

    // Apply retention policy for check_result table
    let check_result_retention = format!(
        "SELECT add_retention_policy('check_result', INTERVAL '{}');",
        config.db_retention_check_results
    );
    postgres_client
        .execute(&check_result_retention, &[])
        .await?;
    info!(
        "Applied retention policy for check_result: {}",
        config.db_retention_check_results
    );

    // Apply retention policy for check_result_perf_data table
    let perf_data_retention = format!(
        "SELECT add_retention_policy('check_result_perf_data', INTERVAL '{}');",
        config.db_retention_perf_data
    );
    postgres_client.execute(&perf_data_retention, &[]).await?;
    info!(
        "Applied retention policy for check_result_perf_data: {}",
        config.db_retention_perf_data
    );

    Ok(())
}
