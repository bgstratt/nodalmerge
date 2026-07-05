use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HostCoreError {
    #[error("invalid command")]
    InvalidCommand,

    #[error("room not found")]
    RoomNotFound,

    #[error("session not found")]
    SessionNotFound,

    #[error("session already open")]
    SessionAlreadyOpen,

    #[error("protocol violation")]
    ProtocolViolation,

    #[error("auth violation")]
    AuthViolation,

    #[error("policy violation")]
    PolicyViolation,

    #[error("internal invariant")]
    InternalInvariant,
}

pub type HostCoreResult<T> = Result<T, HostCoreError>;
