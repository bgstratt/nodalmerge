use thiserror::Error;

pub type GcResult<T> = Result<T, GcError>;

#[derive(Debug, Error)]
pub enum GcError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("invalid state transition: {0}")]
    InvalidState(String),
    #[error("invariant violation: {0}")]
    Invariant(String),
}
