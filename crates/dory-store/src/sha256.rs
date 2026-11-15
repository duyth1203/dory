use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256, used to key legacy imports by file content.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn known_vectors() {
        let cases = [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(sha256_hex(input.as_bytes()), want, "{input:?}");
        }
    }
}
