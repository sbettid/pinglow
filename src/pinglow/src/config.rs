use std::env;

#[derive(Debug, Clone)]
pub struct PinglowConfig {
    pub target_namespace: String,
    pub db: String,
    pub db_host: String,
    pub db_user: String,
    pub db_user_password: String,
    pub redis_password: String,
    pub oidc: Option<OidcConfig>,
    pub oidc_cookie_secure: bool,
}

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
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
        redis_password: env::var("REDIS_PASSWORD").expect("Redis password must be set"),
        oidc: match (env::var("OIDC_ISSUER_URL"), env::var("OIDC_CLIENT_ID"), env::var("OIDC_CLIENT_SECRET"), env::var("OIDC_REDIRECT_URL")) {
            (Ok(issuer), Ok(client_id), Ok(client_secret), Ok(redirect_url)) => Some(OidcConfig { issuer, client_id, client_secret, redirect_url }),
            (Err(_), Err(_), Err(_), Err(_)) => None,
            _ => panic!("OIDC configuration requires OIDC_ISSUER_URL, OIDC_CLIENT_ID, OIDC_CLIENT_SECRET, and OIDC_REDIRECT_URL"),
        },
        oidc_cookie_secure: env::var("OIDC_COOKIE_SECURE").map(|value| value != "false").unwrap_or(true),
    }
}
