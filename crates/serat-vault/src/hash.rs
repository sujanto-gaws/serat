//! Content hashing.

use serat_core::ContentHash;

/// Hash a file's bytes.
///
/// The gate at architecture §3.1 step 3: sync tools touch mtime constantly without
/// changing content, and comparing this against `notes.hash` discards those events
/// for the cost of a hash instead of the cost of a parse. The same value is what
/// echo suppression matches on (§3.2) and what the write-back optimistic lock
/// compares (§6.1 step 2), so there is exactly one definition of it.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> ContentHash {
    ContentHash::from_bytes(*blake3::hash(bytes).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bytes_hash_identically_and_different_bytes_do_not() {
        let note = b"---\ntype: project\n---\n\n# Serat\n";
        assert_eq!(hash_bytes(note), hash_bytes(note));
        assert_ne!(hash_bytes(note), hash_bytes(b"---\ntype: system\n---\n"));
        // Empty is a legitimate note, not an error.
        assert_eq!(hash_bytes(b"").to_hex().len(), 64);
    }

    #[test]
    fn the_hash_is_blake3_of_the_content_and_nothing_else() {
        // Pinned against the reference implementation's known vector, so a change of
        // hash function is a test failure rather than a silent full reindex for
        // every user.
        assert_eq!(
            hash_bytes(b"abc").to_hex(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }
}
