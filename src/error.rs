#[derive(thiserror::Error, Debug)]
pub enum ReconcileError {
    #[error("Script '{0}' not found")]
    ScriptNotFound(String),

    #[error("TelegramChannel '{0}' not found")]
    TelegramChannelNotFound(String),

    #[error("Secret '{0}' not found")]
    SecretNotFound(String),

    #[error("Script fetch error: {0}")]
    ScriptFetchError(#[from] kube::Error),
}
