use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A field-level validation error returned by the Merchant API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProblemError {
    /// Stable machine-readable error code.
    pub code: String,
    /// Optional wire field associated with the error.
    pub field: Option<String>,
    /// Human-readable diagnostic message.
    pub message: String,
}

/// Stable category derived from an HTTP error status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// Request validation failed.
    Validation,
    /// Credentials are absent, invalid, or insufficient.
    Authentication,
    /// The resource was not found.
    NotFound,
    /// The request conflicts with current resource state.
    Conflict,
    /// The resource is no longer available.
    Gone,
    /// The caller exceeded a rate limit.
    RateLimit,
    /// The server failed while processing the request.
    Server,
    /// The service is temporarily unavailable.
    Unavailable,
    /// A non-standard HTTP status was returned.
    Unknown,
}

impl ApiErrorKind {
    pub(crate) const fn from_status(status: u16) -> Self {
        match status {
            400 => Self::Validation,
            401 | 403 => Self::Authentication,
            404 => Self::NotFound,
            409 => Self::Conflict,
            410 => Self::Gone,
            429 => Self::RateLimit,
            500 => Self::Server,
            503 => Self::Unavailable,
            _ => Self::Unknown,
        }
    }
}

/// Structured RFC 9457 error returned by the Merchant API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiError {
    /// HTTP status code.
    pub status: u16,
    /// Status-derived stable category.
    pub kind: ApiErrorKind,
    /// Problem type URI, when supplied.
    pub problem_type: Option<String>,
    /// Short problem title, when supplied.
    pub title: Option<String>,
    /// Human-readable detail, when supplied.
    pub detail: Option<String>,
    /// Top-level machine-readable code, when supplied.
    pub code: Option<String>,
    /// Top-level field, when supplied.
    pub field: Option<String>,
    /// Validation errors for a multi-error response.
    pub errors: Vec<ProblemError>,
    /// Server-requested retry delay, when supplied.
    pub retry_after: Option<Duration>,
    /// Trace identifier, when supplied.
    pub trace_id: Option<String>,
    /// Raw response body retained for diagnostics.
    pub raw_body: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = self
            .detail
            .as_deref()
            .or(self.title.as_deref())
            .unwrap_or("Paymos API request failed");
        if let Some(code) = self.code.as_deref() {
            write!(
                formatter,
                "Paymos API returned {} ({code}): {message}",
                self.status
            )
        } else {
            write!(formatter, "Paymos API returned {}: {message}", self.status)
        }
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ProblemDetails {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
    pub field: Option<String>,
    #[serde(default)]
    pub errors: Vec<ProblemError>,
    pub trace_id: Option<String>,
}

/// Errors produced by the Paymos client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Client configuration is invalid.
    #[error("invalid Paymos client configuration: {0}")]
    Configuration(String),
    /// A method argument is invalid.
    #[error("invalid Paymos argument: {0}")]
    InvalidArgument(String),
    /// The HTTP request failed before a valid response was received.
    #[error("Paymos transport failed: {0}")]
    Transport(#[source] reqwest::Error),
    /// The API returned an error response.
    #[error(transparent)]
    Api(Box<ApiError>),
    /// A request body could not be serialized.
    #[error("Paymos request could not be serialized: {0}")]
    Serialization(#[source] serde_json::Error),
    /// A successful response did not contain valid JSON for the expected contract.
    #[error("Paymos API returned an invalid response: {0}")]
    InvalidResponse(#[source] serde_json::Error),
    /// Cursor traversal violated its safety bound or repeated a cursor.
    #[error("Paymos pagination failed: {0}")]
    Pagination(String),
    /// The local system clock could not produce Unix time.
    #[error("system clock is before the Unix epoch")]
    InvalidClock,
}

impl From<ApiError> for Error {
    fn from(value: ApiError) -> Self {
        Self::Api(Box::new(value))
    }
}
