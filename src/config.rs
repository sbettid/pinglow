use std::env;

#[derive(Debug, Clone)]
pub struct PinglowConfig {
    pub target_namespace: String,
    pub db: String,
    pub db_host: String,
    pub db_user: String,
    pub db_user_password: String,
}

/**
 * This function parses the environment variables and returns a configuration
 */
pub fn get_config_from_env() -> PinglowConfig {
    PinglowConfig {
        target_namespace: env::var("NAMESPACE").unwrap_or("pinglow".to_string()),
        db: env::var("DB").unwrap_or("pinglow".to_string()),
        db_host: env::var("DB_HOST").unwrap_or("localhost".to_string()),
        db_user: env::var("DB_USER").expect("The variable DB_USER must be set"),
        db_user_password: env::var("DB_USER_PASSWORD")
            .expect("The variable DB_USER_PASSWORD must be set"),
    }
}
