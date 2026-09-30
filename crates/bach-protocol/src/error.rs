use serde::{Deserialize, Serialize};
use std::fmt;
use ts_rs::TS;

/// What kind of failure an [`ApiError`] is, for code that reacts to it. People read `message`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request was malformed or its arguments were rejected; retrying it unchanged won't help.
    Invalid,
    /// What it refers to doesn't exist (any more): a finished run, an answered approval, a
    /// removed task.
    NotFound,
    /// The feature isn't available on this backend.
    Unavailable,
    /// It was tried and didn't work; `message` says why.
    Failed,
    /// What it would change was changed by someone else since the caller last looked (a file
    /// edited on disk); retrying with the caller's overwrite flag set replaces it anyway.
    Conflict,
}

/// Every command fails with one of these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ApiError {
    pub code: ErrorCode,
    /// Written for the user.
    pub message: String,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, message)
    }
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Failed, message)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// Plain messages from code that doesn't classify its errors yet.
impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self::failed(message)
    }
}

impl From<&str> for ApiError {
    fn from(message: &str) -> Self {
        Self::failed(message)
    }
}
