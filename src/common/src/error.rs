#[derive(thiserror::Error, Debug)]
pub enum SerializeError {
    #[error("Cannot deserialize object: {0}")]
    DeserializationError(String),
    #[error("Cannot serialize object: {0}")]
    SerializationError(String),
}

#[derive(thiserror::Error, Debug)]
pub enum ExecutionError {
    #[error("Venv error: {0}")]
    VenvError(String),
    #[error("Exit code error: {0}")]
    ExitCodeError(String),
}

#[derive(thiserror::Error, Debug)]
pub enum ScriptError {
    #[error("Error: No script found for check: {0}")]
    NoScriptFound(String),
}
