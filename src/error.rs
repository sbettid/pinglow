#[derive(thiserror::Error, Debug)]
pub enum ReconcileError {
    #[error("Script '{0}' not found")]
    ScriptNotFound(String),

    #[error("TelegramChannel '{0}' not found")]
    TelegramChannelNotFound(String),

    #[error("Secret '{0}' not found")]
    SecretNotFound(String),

    #[error("PropertyExtractionError '{0}' not found")]
    PropertyExtractionError(String),

    #[error("GeneralError fetch error: {0}")]
    GeneralError(#[from] kube::Error),

    #[error("Cannot send message in channel: {0}")]
    SendError(String),
}

#[derive(thiserror::Error, Debug)]
pub enum TimescaleDBConversionError {
    #[error("Cannot deserialize result: {0}")]
    DeserializationError(String),
}

#[derive(thiserror::Error, Debug)]
pub enum ChannelError {}
