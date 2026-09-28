//! Centralized application error type.
//!
//! Every fallible operation in NotroDNS funnels through [`AppError`] so the
//! GUI layer can display a single, consistent error string regardless of
//! which subsystem (network, filesystem, Windows API, input validation)
//! produced it.

use std::fmt;

#[derive(Debug, Clone)]
pub enum AppError {
    Network(String),
    Io(String),
    Serde(String),
    Windows(String),
    Validation(String),
    NotElevated,
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Network(s) => write!(f, "Network error: {s}"),
            AppError::Io(s) => write!(f, "I/O error: {s}"),
            AppError::Serde(s) => write!(f, "Data error: {s}"),
            AppError::Windows(s) => write!(f, "Windows error: {s}"),
            AppError::Validation(s) => write!(f, "Validation error: {s}"),
            AppError::NotElevated => write!(f, "Administrator privileges are required for this action"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Network(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Serde(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
