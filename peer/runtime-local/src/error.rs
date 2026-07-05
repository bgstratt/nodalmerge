//! Bounded `reason_class` values for peer-local persistence failures (Wave 2 Phase A).

/// Stable rejection taxonomy for local persistence adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPersistReason {
    Unavailable,
    VersionSkew,
    Corruption,
    TailConflict,
    Quota,
    ReadOnly,
}

impl LocalPersistReason {
    pub fn reason_class(self) -> &'static str {
        match self {
            Self::Unavailable => "reject.local_persist_unavailable",
            Self::VersionSkew => "reject.local_persist_version_skew",
            Self::Corruption => "reject.local_persist_corruption",
            Self::TailConflict => "reject.local_persist_tail_conflict",
            Self::Quota => "reject.local_persist_quota",
            Self::ReadOnly => "reject.local_persist_readonly",
        }
    }

    /// Operator-facing recovery posture (retryable / config / data-loss-risk).
    pub fn recovery_posture(self) -> &'static str {
        match self {
            Self::Unavailable | Self::TailConflict => "retryable",
            Self::VersionSkew | Self::Quota | Self::ReadOnly => "operator_fix",
            Self::Corruption => "data_loss_risk",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalPersistError {
    pub reason: LocalPersistReason,
    pub message: String,
}

impl LocalPersistError {
    pub fn new(reason: LocalPersistReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }

    pub fn reason_class(&self) -> &'static str {
        self.reason.reason_class()
    }
}

impl std::fmt::Display for LocalPersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason.reason_class(), self.message)
    }
}

impl std::error::Error for LocalPersistError {}

pub type LocalPersistResult<T> = Result<T, LocalPersistError>;
