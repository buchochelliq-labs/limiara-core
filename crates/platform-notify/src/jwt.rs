//! Compact JWS for the providers' tokens: ES256 for Apple, RS256 for
//! Google. Only signing is needed; the providers verify.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::Value;

fn segment(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string().as_bytes())
}

/// The signing input, `header.claims`.
pub(crate) fn signing_input(header: &Value, claims: &Value) -> String {
    format!("{}.{}", segment(header), segment(claims))
}

/// Appends a raw signature to its signing input.
pub(crate) fn compact(input: &str, signature: &[u8]) -> String {
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
}
