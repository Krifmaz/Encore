//! Identifiers: random tokens, keys and invite codes, the digest tokens are kept as, MusicBrainz ids.
use rand::Rng;

/// `bytes` random bytes as lowercase hex (twice as many characters).
pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::rng().fill(&mut b[..]);
    hex::encode(b)
}

/// `bytes` random bytes as URL-safe base64 without padding (session tokens, share links).
pub fn random_token(bytes: usize) -> String {
    use base64::Engine;
    let mut b = vec![0u8; bytes];
    rand::rng().fill(&mut b[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// `n` random letters and digits (A–Z, a–z, 0–9): passwords and keys written into another program's settings.
pub fn random_alnum(n: usize) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut r = rand::rng();
    (0..n).map(|_| ABC[r.random_range(0..ABC.len())] as char).collect()
}

/// The SHA-256 of `data` as lowercase hex: how tokens are kept (never the token itself) and downloads checked.
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data.as_ref()))
}

/// A MusicBrainz id: 36 characters, hex digits with dashes at 8, 13, 18 and 23.
pub fn is_mbid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_of_the_asked_length() {
        let a = super::random_hex(24);
        assert_eq!(a.len(), 48);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, super::random_hex(24));
    }

    #[test]
    fn sha256_of_abc() {
        assert_eq!(super::sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(super::sha256_hex("abc"), super::sha256_hex(String::from("abc")));
    }

    #[test]
    fn letters_and_digits_of_the_asked_length() {
        let a = super::random_alnum(32);
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_ne!(a, super::random_alnum(32));
    }
}
