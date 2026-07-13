//! Low-level helpers for deterministic request signing and RFC 3986 encoding.

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Builds the exact canonical payload signed by the Merchant API.
#[must_use]
pub fn string_to_sign(
    timestamp: impl std::fmt::Display,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
) -> String {
    let body_hash = if body.is_empty() {
        String::new()
    } else {
        hex::encode(Sha256::digest(body))
    };
    format!(
        "{timestamp}\n{}\n{path}\n{query}\n{body_hash}",
        method.to_ascii_uppercase()
    )
}

/// Computes a base64 HMAC-SHA256 API request signature.
///
/// # Panics
///
/// The underlying HMAC constructor accepts keys of every length, so its error
/// branch is unreachable for this algorithm.
#[must_use]
pub fn sign(secret: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(payload);
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Builds the value of the Merchant API `Authorization` header.
#[must_use]
pub fn authorization_header(
    api_key: &str,
    api_secret: &[u8],
    timestamp: impl std::fmt::Display + Copy,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
) -> String {
    let canonical = string_to_sign(timestamp, method, path, query, body);
    format!(
        "HMAC-SHA256 {api_key}:{}",
        sign(api_secret, canonical.as_bytes())
    )
}

/// Percent-encodes a path segment or query component using RFC 3986.
#[must_use]
pub fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            use std::fmt::Write as _;
            write!(encoded, "%{byte:02X}").expect("writing to a String cannot fail");
        }
    }
    encoded
}

/// Produces a stable RFC 3986 query string, including the leading `?`.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidArgument`] when a key or value is empty.
pub fn build_query<I, K, V>(pairs: I) -> Result<String, crate::Error>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let mut values = pairs
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect::<Vec<_>>();
    if values
        .iter()
        .any(|(key, value)| key.is_empty() || value.is_empty())
    {
        return Err(crate::Error::InvalidArgument(
            "query keys and values must be non-empty".to_owned(),
        ));
    }
    values.sort_unstable();
    if values.is_empty() {
        return Ok(String::new());
    }
    let query = values
        .into_iter()
        .map(|(key, value)| format!("{}={}", encode_component(&key), encode_component(&value)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("?{query}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_utf8_and_reserved_characters() {
        assert_eq!(
            encode_component("a b/тест"),
            "a%20b%2F%D1%82%D0%B5%D1%81%D1%82"
        );
    }

    #[test]
    fn sorts_keys_and_repeated_values() {
        let query = build_query([
            ("status", "paid_over"),
            ("project_id", "prj/a"),
            ("status", "paid"),
        ])
        .unwrap();
        assert_eq!(query, "?project_id=prj%2Fa&status=paid&status=paid_over");
    }
}
