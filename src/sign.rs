//! URL signatures for private images.
//!
//! `sig = base64url_nopad(HMAC-SHA256(secret, "private/{key}\n{exp}"))`
//!
//! Only the image key and expiry are signed. Size, fit, format and quality are
//! free for the client to choose (within the allowlist), so one signature
//! covers every srcset entry of the same image.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

fn mac(secret: &[u8], key: &str, exp: u64) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(format!("private/{key}\n{exp}").as_bytes());
    mac
}

/// Produces the `sig` query value. The backend implements the same thing.
pub fn sign(secret: &[u8], key: &str, exp: u64) -> String {
    URL_SAFE_NO_PAD.encode(mac(secret, key, exp).finalize().into_bytes())
}

/// Constant-time signature check.
pub fn verify(secret: &[u8], key: &str, exp: u64, sig: &str) -> bool {
    let Ok(sig) = URL_SAFE_NO_PAD.decode(sig) else {
        return false;
    };
    mac(secret, key, exp).verify_slice(&sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn roundtrip() {
        let sig = sign(SECRET, "user/1/a.jpg", 1_760_000_000);
        assert!(verify(SECRET, "user/1/a.jpg", 1_760_000_000, &sig));
    }

    #[test]
    fn rejects_tampering() {
        let sig = sign(SECRET, "user/1/a.jpg", 1_760_000_000);
        assert!(!verify(SECRET, "user/2/a.jpg", 1_760_000_000, &sig));
        assert!(!verify(SECRET, "user/1/a.jpg", 1_760_000_001, &sig));
        assert!(!verify(
            b"another-secret-another-secret-xx",
            "user/1/a.jpg",
            1_760_000_000,
            &sig
        ));
        assert!(!verify(
            SECRET,
            "user/1/a.jpg",
            1_760_000_000,
            "not base64!"
        ));
        assert!(!verify(SECRET, "user/1/a.jpg", 1_760_000_000, &sig[..10]));
    }

    /// Fixed vector so backend implementations can be checked against it.
    #[test]
    fn known_vector() {
        assert_eq!(
            sign(SECRET, "user/1/a.jpg", 1_760_000_000),
            "JF40nPEwOXXvtzIzv-7qjIENoI0ol9KEpaMyud0nRo8"
        );
    }
}
