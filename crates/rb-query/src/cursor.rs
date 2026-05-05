//! Opaque cursor encoding for pagination.
//!
//! A cursor is `base64url(json({"offset": N}))`. Callers receive a `next_cursor`
//! and pass it back as `?cursor=...` to advance through a result set.

use anyhow::{bail, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct CursorPayload {
    offset: usize,
}

/// Encode an offset into an opaque cursor string.
pub fn encode(offset: usize) -> String {
    let payload = serde_json::to_string(&CursorPayload { offset }).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(payload.as_bytes())
}

/// Decode an opaque cursor string into an offset.
///
/// Returns `Ok(0)` when `cursor` is `None`.
pub fn decode(cursor: Option<&str>) -> Result<usize> {
    let Some(s) = cursor else {
        return Ok(0);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| anyhow::anyhow!("invalid cursor: base64 decode failed"))?;
    let payload: CursorPayload = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid cursor: JSON decode failed"))?;
    if payload.offset > 1_000_000 {
        bail!("invalid cursor: offset out of range");
    }
    Ok(payload.offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cursor = encode(42);
        let offset = decode(Some(&cursor)).unwrap();
        assert_eq!(offset, 42);
    }

    #[test]
    fn none_cursor_is_zero() {
        assert_eq!(decode(None).unwrap(), 0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(Some("not-base64!!!")).is_err());
    }

    #[test]
    fn rejects_invalid_json_payload() {
        let garbage = URL_SAFE_NO_PAD.encode(b"not json");
        assert!(decode(Some(&garbage)).is_err());
    }

    #[test]
    fn zero_offset_cursor() {
        let cursor = encode(0);
        assert_eq!(decode(Some(&cursor)).unwrap(), 0);
    }
}
