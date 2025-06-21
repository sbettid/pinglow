#[derive(thiserror::Error, Debug)]
pub enum ReconcileError {
    #[error("Script '{0}' not found")]
    ScriptNotFound(String),

    #[error("Script fetch error: {0}")]
    ScriptFetchError(#[from] kube::Error),
}
