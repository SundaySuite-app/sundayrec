//! Centralised error type for the SundayRec backend.
//!
//! Tauri commands return `Result<T, AppError>` — `AppError` implements
//! `serde::Serialize` so it crosses the IPC boundary as a stable JSON shape
//! (`{ code, message }`) the renderer can pattern-match on.
//!
//! Keep `AppError::code()` (here) and the TS `AppError` union in
//! `src/lib/bindings/` in sync when you add a variant. Domain variants
//! (`Recording`, `Database`, `Export`, …) get added as their phases land.

use serde::{Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    /// Entity not found by ID — distinct so the renderer can render a "404" UI.
    #[error("not found: {entity} id={id}")]
    NotFound { entity: &'static str, id: String },

    /// Caller passed input that fails our domain rules.
    #[error("validation: {0}")]
    Validation(String),

    /// Recording subsystem failure (device, ffmpeg process, capture).
    #[error("recording error: {0}")]
    Recording(String),

    /// File-system / IO failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialisation/deserialisation issue.
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),

    /// Anything else we couldn't classify.
    #[error("internal: {0}")]
    Internal(String),
}

impl AppError {
    /// Short, machine-readable category for the renderer to switch on.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotFound { .. } => "not_found",
            AppError::Validation(_) => "validation",
            AppError::Recording(_) => "recording",
            AppError::Io(_) => "io",
            AppError::Json(_) => "json",
            AppError::Internal(_) => "internal",
        }
    }
}

/// Custom serializer so the JSON sent to the renderer carries both a stable
/// `code` (for switch statements) and a human-readable `message`.
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("code", self.code())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

/// Convenience alias for the project.
pub type AppResult<T> = Result<T, AppError>;
