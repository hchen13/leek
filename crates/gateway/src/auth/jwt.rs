//! Minimal JWT helpers — decode `exp` only, no signature verification.
//!
//! Codex access tokens are JWTs whose `exp` field tells us when to refresh.
//! Verifying the signature would require OpenAI's JWKS, which we don't need
//! for refresh decisions (the LLM API itself rejects invalid tokens).

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
struct ExpClaim {
    exp: i64,
}

pub fn decode_exp(token: &str) -> Result<DateTime<Utc>> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(anyhow!("not a JWT (expected 3 segments)"));
    }
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .context("JWT payload base64 decode failed")?;
    let claims: ExpClaim =
        serde_json::from_slice(&payload).context("JWT payload not valid JSON")?;
    Utc.timestamp_opt(claims.exp, 0)
        .single()
        .ok_or_else(|| anyhow!("JWT exp out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_known_exp() {
        // Header: {"alg":"none"} — Payload: {"exp":1893456000} — Signature: empty
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"exp":1893456000}"#);
        let token = format!("{header}.{payload}.");
        let exp = decode_exp(&token).unwrap();
        assert_eq!(exp.timestamp(), 1893456000);
    }

    #[test]
    fn rejects_non_jwt() {
        assert!(decode_exp("not-a-jwt").is_err());
    }
}
