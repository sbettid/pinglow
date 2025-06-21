use std::env;

#[derive(Debug, Clone)]
pub struct PinglowConfig {
    pub target_namespace: String,
}

/**
 * This function parses the environment variables and returns a configuration
 */
pub fn get_config_from_env() -> PinglowConfig {
    PinglowConfig {
        target_namespace: env::var("namespace").unwrap_or("pinglow".to_string()),
    }
}
