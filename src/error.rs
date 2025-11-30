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

    #[error("GeneralError error: {0}")]
    GeneralError(String),

    #[error("Cannot send message in channel: {0}")]
    SendError(String),
}

impl From<kube::Error> for ReconcileError {
    fn from(value: kube::Error) -> Self {
        ReconcileError::GeneralError(format!("Error: {value}"))
    }
}

#[derive(thiserror::Error, Debug)]
pub enum TimescaleDBConversionError {
    #[error("Cannot deserialize result: {0}")]
    DeserializationError(String),
}

#[derive(thiserror::Error, Debug)]
pub enum ChannelError {}
