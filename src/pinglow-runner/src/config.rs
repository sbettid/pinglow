use std::env;

#[derive(Debug, Clone)]
pub struct PinglowRunnerConfig {
    #[allow(dead_code)]
    pub redis_password: String,
    pub runner_name: String,
    pub checks_base_path: String,
}

/**
 * This function parses the environment variables and returns a configuration
 */
pub fn get_config_from_env() -> PinglowRunnerConfig {
    PinglowRunnerConfig {
        redis_password: env::var("REDIS_PASSWORD").expect("Redis password must be set"),
        runner_name: env::var("RUNNER_NAME").unwrap_or_else(|_| "runner-unknown".into()),
        checks_base_path: env::var("CHECKS_BASE_PATH")
            .unwrap_or_else(|_| "/home/pinglow-runner/".into()),
    }
}
