use std::fmt;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub path: String,
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("usage error: {0}")]
    Usage(String),

    #[error("configuration error: {0:?}")]
    Config(Vec<Violation>),

    #[error("name resolution error: {0}")]
    NotFound(String),

    #[error("credential error: {0}")]
    Credential(String),

    #[error("target is not executable: {0}")]
    TargetNotExecutable(String),
}

impl AppError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 1,
            Self::Config(_) => 2,
            Self::NotFound(_) => 3,
            Self::Credential(_) => 4,
            Self::TargetNotExecutable(_) => 127,
        }
    }
}
