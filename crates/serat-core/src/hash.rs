//! Content hashes.

use serde::{Deserialize, Serialize};

use crate::error::VaultError;

/// A BLAKE3 hash of a file's bytes.
///
/// Three mechanisms depend on this being the hash of *content*, never of metadata:
/// the hash gate that discards a touched-but-unchanged file before it is parsed
/// (architecture §3.1 step 3), echo suppression of our own writes (§3.2), and the
/// optimistic lock that refuses a stale write-back (§6.1 step 2).
///
/// This type does not compute hashes — computing one means reading a file, and this
/// crate performs no I/O. `serat-vault` hashes; everyone else compares.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContentHash([u8; Self::LEN]);

impl ContentHash {
    /// Length of a BLAKE3 digest in bytes.
    pub const LEN: usize = 32;

    /// Wrap raw digest bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    /// Lowercase hex, which is how the hash is stored in `notes.hash` and how it
    /// crosses the IPC boundary.
    #[must_use]
    pub fn to_hex(&self) -> String {
        use std::fmt::Write as _;
        self.0.iter().fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    /// Parse lowercase or uppercase hex.
    ///
    /// # Errors
    ///
    /// [`VaultError::MalformedHash`] if the input is not 64 hex digits.
    pub fn from_hex(text: &str) -> Result<Self, VaultError> {
        let malformed = || VaultError::MalformedHash {
            value: text.to_owned(),
        };
        if text.len() != Self::LEN * 2 {
            return Err(malformed());
        }
        let mut out = [0u8; Self::LEN];
        for (byte, pair) in out.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
            let hi = (pair[0] as char).to_digit(16).ok_or_else(malformed)?;
            let lo = (pair[1] as char).to_digit(16).ok_or_else(malformed)?;
            *byte = ((hi << 4) | lo) as u8;
        }
        Ok(Self(out))
    }
}

impl std::fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Full hex in Debug: a truncated hash in a log is exactly the thing you
        // cannot compare against the one in the database.
        write!(f, "ContentHash({})", self.to_hex())
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl From<ContentHash> for String {
    fn from(value: ContentHash) -> Self {
        value.to_hex()
    }
}

impl TryFrom<String> for ContentHash {
    type Error = VaultError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_hex(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let hash = ContentHash::from_bytes([0xab; ContentHash::LEN]);
        let hex = hash.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(ContentHash::from_hex(&hex).expect("parse"), hash);
    }

    #[test]
    fn malformed_hex_is_an_error_not_a_panic() {
        assert!(ContentHash::from_hex("").is_err());
        assert!(ContentHash::from_hex("zz").is_err());
        assert!(ContentHash::from_hex(&"g".repeat(64)).is_err());
    }

    #[test]
    fn serialises_as_a_hex_string() {
        let hash = ContentHash::from_bytes([0x01; ContentHash::LEN]);
        let json = serde_json::to_string(&hash).expect("serialise");
        assert!(json.starts_with("\"0101"), "{json}");
        let back: ContentHash = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, hash);
    }
}
