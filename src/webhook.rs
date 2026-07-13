use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, KeyInit, Mac};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha2::Sha256;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;

/// A typed webhook event after signature and timestamp verification.
#[derive(Clone, Debug, Deserialize)]
pub struct WebhookEvent<T> {
    /// Stable event identifier used for deduplication.
    pub event_id: String,
    /// Event type, such as `invoice.paid`.
    pub event_type: String,
    /// Event schema version.
    pub version: u32,
    /// Event creation timestamp in Unix seconds.
    pub occurred_at: i64,
    /// Event-specific data.
    pub data: T,
}

/// Webhook verification or decoding error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WebhookError {
    /// Verifier configuration is invalid.
    #[error("invalid webhook verifier configuration: {0}")]
    Configuration(&'static str),
    /// The signature header is missing required fields or contains duplicates.
    #[error("webhook signature header is missing or malformed")]
    MalformedHeader,
    /// The event timestamp lies outside the configured replay window.
    #[error("webhook timestamp is outside the allowed tolerance")]
    TimestampOutsideTolerance,
    /// None of the rotated `v1` signatures matches the exact raw body.
    #[error("webhook signature does not match the payload")]
    SignatureMismatch,
    /// The verified payload is not a valid event envelope.
    #[error("webhook payload is invalid: {0}")]
    InvalidPayload(#[source] serde_json::Error),
    /// The local system clock cannot be represented as Unix seconds.
    #[error("system clock is before the Unix epoch")]
    InvalidClock,
}

/// Verifies Paymos webhook signatures over the exact raw request bytes.
pub struct WebhookVerifier {
    secret: Zeroizing<String>,
    tolerance_seconds: u64,
}

impl WebhookVerifier {
    /// Creates a verifier with the default five-minute replay window.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::Configuration`] when the secret is empty.
    pub fn new(secret: impl Into<String>) -> Result<Self, WebhookError> {
        Self::with_tolerance(secret, 300)
    }

    /// Creates a verifier with an explicit replay window in seconds.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::Configuration`] when the secret is empty.
    pub fn with_tolerance(
        secret: impl Into<String>,
        tolerance_seconds: u64,
    ) -> Result<Self, WebhookError> {
        let secret = secret.into();
        if secret.trim().is_empty() {
            return Err(WebhookError::Configuration(
                "webhook secret must be non-empty",
            ));
        }
        Ok(Self {
            secret: Zeroizing::new(secret),
            tolerance_seconds,
        })
    }

    /// Verifies a signature using the current system clock.
    ///
    /// # Errors
    ///
    /// Returns a [`WebhookError`] for a malformed header, stale timestamp,
    /// signature mismatch, or invalid local clock.
    pub fn verify(&self, signature_header: &str, raw_body: &[u8]) -> Result<(), WebhookError> {
        self.verify_at(signature_header, raw_body, unix_now()?)
    }

    /// Verifies a signature against an explicit Unix timestamp.
    ///
    /// # Errors
    ///
    /// Returns a [`WebhookError`] for a malformed header, stale timestamp, or
    /// signature mismatch.
    ///
    /// # Panics
    ///
    /// The underlying HMAC constructor accepts keys of every length, so its
    /// error branch is unreachable for this algorithm.
    pub fn verify_at(
        &self,
        signature_header: &str,
        raw_body: &[u8],
        now: i64,
    ) -> Result<(), WebhookError> {
        let parsed = ParsedHeader::parse(signature_header)?;
        if now.abs_diff(parsed.timestamp) > self.tolerance_seconds {
            return Err(WebhookError::TimestampOutsideTolerance);
        }

        let mut payload = parsed.timestamp.to_string().into_bytes();
        payload.push(b'.');
        payload.extend_from_slice(raw_body);

        let matched = parsed.signatures.iter().any(|signature| {
            let Ok(candidate) = hex::decode(signature) else {
                return false;
            };
            let mut mac = HmacSha256::new_from_slice(self.secret.as_bytes())
                .expect("HMAC accepts keys of any length");
            mac.update(&payload);
            mac.verify_slice(&candidate).is_ok()
        });
        if matched {
            Ok(())
        } else {
            Err(WebhookError::SignatureMismatch)
        }
    }

    /// Verifies and deserializes a typed event using the current system clock.
    ///
    /// # Errors
    ///
    /// Returns a [`WebhookError`] when verification or event decoding fails.
    pub fn construct_event<T: DeserializeOwned>(
        &self,
        signature_header: &str,
        raw_body: &[u8],
    ) -> Result<WebhookEvent<T>, WebhookError> {
        self.construct_event_at(signature_header, raw_body, unix_now()?)
    }

    /// Verifies and deserializes a typed event against an explicit Unix timestamp.
    ///
    /// # Errors
    ///
    /// Returns a [`WebhookError`] when verification or event decoding fails.
    pub fn construct_event_at<T: DeserializeOwned>(
        &self,
        signature_header: &str,
        raw_body: &[u8],
        now: i64,
    ) -> Result<WebhookEvent<T>, WebhookError> {
        self.verify_at(signature_header, raw_body, now)?;
        let event: WebhookEvent<T> =
            serde_json::from_slice(raw_body).map_err(WebhookError::InvalidPayload)?;
        if event.event_id.trim().is_empty()
            || event.event_type.trim().is_empty()
            || event.version == 0
        {
            return Err(WebhookError::InvalidPayload(serde_json::Error::io(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "webhook event envelope contains an empty required field",
                ),
            )));
        }
        Ok(event)
    }
}

struct ParsedHeader<'a> {
    timestamp: i64,
    signatures: Vec<&'a str>,
}

impl<'a> ParsedHeader<'a> {
    fn parse(value: &'a str) -> Result<Self, WebhookError> {
        let mut timestamp = None;
        let mut signatures = Vec::new();
        for part in value.split(',') {
            let Some((key, value)) = part.trim().split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "t" => {
                    if timestamp.is_some()
                        || value.is_empty()
                        || !value.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        return Err(WebhookError::MalformedHeader);
                    }
                    timestamp = Some(value.parse().map_err(|_| WebhookError::MalformedHeader)?);
                }
                "v1" if !value.is_empty() => signatures.push(value),
                _ => {}
            }
        }
        let timestamp = timestamp.ok_or(WebhookError::MalformedHeader)?;
        if signatures.is_empty() {
            return Err(WebhookError::MalformedHeader);
        }
        Ok(Self {
            timestamp,
            signatures,
        })
    }
}

fn unix_now() -> Result<i64, WebhookError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| WebhookError::InvalidClock)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| WebhookError::InvalidClock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_timestamps() {
        let verifier = WebhookVerifier::new("secret").unwrap();
        let error = verifier.verify_at("t=1,t=1,v1=00", b"{}", 1).unwrap_err();
        assert!(matches!(error, WebhookError::MalformedHeader));
    }

    #[test]
    fn rejects_non_decimal_timestamps() {
        let verifier = WebhookVerifier::new("secret").unwrap();
        assert!(matches!(
            verifier.verify_at("t=-1,v1=00", b"{}", 1),
            Err(WebhookError::MalformedHeader)
        ));
    }

    #[test]
    fn rejects_stale_events_before_signature_work() {
        let verifier = WebhookVerifier::with_tolerance("secret", 10).unwrap();
        let error = verifier.verify_at("t=1,v1=00", b"{}", 12).unwrap_err();
        assert!(matches!(error, WebhookError::TimestampOutsideTolerance));
    }
}
