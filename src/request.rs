//! Parses and validates the public URL format and builds the internal imagor path.
//!
//! ```text
//! /public/{key}                  full size, format picked from `Accept`
//! /public/{key}?w=640&fmt=webp    scaled to width, aspect ratio kept
//! /public/{key}?original          source file, untouched
//! /private/{key}?w=640&exp=1760000000&sig=...
//! ```
//!
//! Nothing from the client is forwarded to imagor verbatim. The key is limited
//! to a strict charset and passed as `b64:` so imagor never interprets it, and
//! every option maps to a fixed imagor fragment. This rules out filters that
//! load other images (`watermark`, `image`), `meta/`, `params/` and any path
//! tricks that could make imagor read from a different bucket.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::StatusCode;

use crate::{config::Config, sign};

const MAX_KEY_LEN: usize = 512;

/// Only keys with an image extension are served. The bucket also holds PDFs,
/// which libvips would happily render to an image.
const IMAGE_EXTENSIONS: [&str; 6] = ["jpg", "jpeg", "png", "webp", "avif", "gif"];

#[derive(Debug, PartialEq, Eq)]
pub enum Access {
    Public,
    Private { exp: u64 },
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Format {
    Avif,
    Webp,
    Jpeg,
    Png,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImageRequest {
    pub access: Access,
    pub key: String,
    pub width: Option<u32>,
    pub format: Option<Format>,
    /// Serve the source file as stored, skipping processing and the result cache.
    pub original: bool,
}

impl ImageRequest {
    /// `raw_path` must be the path exactly as received, not percent-decoded.
    pub fn parse(
        cfg: &Config,
        raw_path: &str,
        raw_query: Option<&str>,
        now: u64,
    ) -> Result<Self, StatusCode> {
        let (private, key) = if let Some(key) = raw_path.strip_prefix("/public/") {
            (false, key)
        } else if let Some(key) = raw_path.strip_prefix("/private/") {
            (true, key)
        } else {
            return Err(StatusCode::NOT_FOUND);
        };
        if !valid_key(key) || !has_image_extension(key) {
            return Err(StatusCode::NOT_FOUND);
        }

        let mut req = ImageRequest {
            access: Access::Public,
            key: key.to_string(),
            width: None,
            format: None,
            original: false,
        };
        let mut exp = None;
        let mut sig = None;

        for pair in raw_query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
            if pair == "original" {
                if std::mem::replace(&mut req.original, true) {
                    return Err(StatusCode::BAD_REQUEST);
                }
                continue;
            }
            let (name, value) = pair.split_once('=').ok_or(StatusCode::BAD_REQUEST)?;
            let duplicate = match name {
                "w" => req.width.replace(size(cfg, value)?).is_some(),
                "fmt" => req
                    .format
                    .replace(match value {
                        "avif" => Format::Avif,
                        "webp" => Format::Webp,
                        "jpeg" | "jpg" => Format::Jpeg,
                        "png" => Format::Png,
                        _ => return Err(StatusCode::BAD_REQUEST),
                    })
                    .is_some(),
                "exp" if private => exp.replace(number::<u64>(value)?).is_some(),
                "sig" if private => sig.replace(value).is_some(),
                _ => return Err(StatusCode::BAD_REQUEST),
            };
            if duplicate {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
        if req.original && (req.width.is_some() || req.format.is_some()) {
            return Err(StatusCode::BAD_REQUEST);
        }

        if private {
            let (Some(exp), Some(sig)) = (exp, sig) else {
                return Err(StatusCode::FORBIDDEN);
            };
            // Verify before checking expiry so an expired-but-forged URL
            // doesn't reveal that the key exists.
            if !sign::verify(&cfg.secret, key, exp, sig) {
                return Err(StatusCode::FORBIDDEN);
            }
            if now >= exp {
                return Err(StatusCode::GONE);
            }
            req.access = Access::Private { exp };
        }
        Ok(req)
    }

    /// Internal imagor path, e.g. `/unsafe/fit-in/640x0/filters:format(webp)/b64:...`.
    pub fn imagor_path(&self, cfg: &Config) -> String {
        let mut path = String::from("/unsafe/");
        if self.original {
            path.push_str("filters:raw()/");
        }
        if let Some(w) = self.width {
            // Height 0 keeps the source aspect ratio; fit-in disables upscaling,
            // so a small source is returned at its own size.
            path.push_str(&format!("fit-in/{w}x0/"));
        }
        if let Some(format) = self.format {
            let name = match format {
                Format::Avif => "avif",
                Format::Webp => "webp",
                Format::Jpeg => "jpeg",
                Format::Png => "png",
            };
            path.push_str(&format!("filters:format({name})/"));
        }
        let bucket = match self.access {
            Access::Public => &cfg.public_bucket,
            Access::Private { .. } => &cfg.private_bucket,
        };
        path.push_str("b64:");
        path.push_str(&URL_SAFE_NO_PAD.encode(format!("{bucket}/{}", self.key)));
        path
    }
}

/// Segments of `[A-Za-z0-9._-]+`, separated by single `/`, no `.`/`..`
/// segments and no leading dot (no hidden files). Since `%` is not allowed,
/// the raw path is also the decoded path.
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && key.split('/').all(|seg| {
            !seg.is_empty()
                && !seg.starts_with('.')
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

fn has_image_extension(key: &str) -> bool {
    key.rsplit_once('.')
        .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Plain decimal only: no sign, no leading zeros, no whitespace.
fn number<T: std::str::FromStr>(value: &str) -> Result<T, StatusCode> {
    let canonical = !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|b| b.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'));
    if !canonical {
        return Err(StatusCode::BAD_REQUEST);
    }
    value.parse().map_err(|_| StatusCode::BAD_REQUEST)
}

fn size(cfg: &Config, value: &str) -> Result<u32, StatusCode> {
    let n = number(value)?;
    if cfg.sizes.contains(&n) {
        Ok(n)
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;
    const EXP: u64 = NOW + 3600;

    fn cfg() -> Config {
        Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            imagor_url: "http://imagor".into(),
            secret: b"0123456789abcdef0123456789abcdef".to_vec(),
            public_bucket: "public".into(),
            private_bucket: "photon-files".into(),
            sizes: vec![320, 640, 1280],
            private_max_age: 3600,
        }
    }

    fn parse(path: &str, query: &str) -> Result<ImageRequest, StatusCode> {
        ImageRequest::parse(&cfg(), path, Some(query), NOW)
    }

    fn private_query(key: &str, exp: u64) -> String {
        format!("exp={exp}&sig={}", sign::sign(&cfg().secret, key, exp))
    }

    fn b64(s: &str) -> String {
        URL_SAFE_NO_PAD.encode(s)
    }

    #[test]
    fn public_minimal() {
        let req = parse("/public/events/a.jpg", "").unwrap();
        assert_eq!(
            req.imagor_path(&cfg()),
            format!("/unsafe/b64:{}", b64("public/events/a.jpg"))
        );
    }

    #[test]
    fn width_and_format() {
        let req = parse("/public/a.jpg", "w=640&fmt=webp").unwrap();
        assert_eq!(
            req.imagor_path(&cfg()),
            format!(
                "/unsafe/fit-in/640x0/filters:format(webp)/b64:{}",
                b64("public/a.jpg")
            )
        );
    }

    #[test]
    fn width_only() {
        let req = parse("/public/a.jpg", "w=320").unwrap();
        assert_eq!(
            req.imagor_path(&cfg()),
            format!("/unsafe/fit-in/320x0/b64:{}", b64("public/a.jpg"))
        );
    }

    #[test]
    fn format_only() {
        let req = parse("/public/a.jpg", "fmt=jpg").unwrap();
        assert_eq!(
            req.imagor_path(&cfg()),
            format!("/unsafe/filters:format(jpeg)/b64:{}", b64("public/a.jpg"))
        );
    }

    #[test]
    fn original() {
        let req = parse("/public/a.jpg", "original").unwrap();
        assert_eq!(
            req.imagor_path(&cfg()),
            format!("/unsafe/filters:raw()/b64:{}", b64("public/a.jpg"))
        );
        let q = format!("original&{}", private_query("a.jpg", EXP));
        let req = parse("/private/a.jpg", &q).unwrap();
        assert_eq!(
            req.imagor_path(&cfg()),
            format!("/unsafe/filters:raw()/b64:{}", b64("photon-files/a.jpg"))
        );
    }

    #[test]
    fn private_valid() {
        let q = format!("w=640&{}", private_query("user/1/a.jpg", EXP));
        let req = parse("/private/user/1/a.jpg", &q).unwrap();
        assert_eq!(req.access, Access::Private { exp: EXP });
        assert_eq!(
            req.imagor_path(&cfg()),
            format!(
                "/unsafe/fit-in/640x0/b64:{}",
                b64("photon-files/user/1/a.jpg")
            )
        );
    }

    #[test]
    fn private_signature_covers_every_size() {
        let sig = private_query("a.jpg", EXP);
        for w in [320, 640, 1280] {
            assert!(parse("/private/a.jpg", &format!("w={w}&{sig}")).is_ok());
        }
    }

    #[test]
    fn private_rejections() {
        // no signature
        assert_eq!(parse("/private/a.jpg", "w=640"), Err(StatusCode::FORBIDDEN));
        assert_eq!(
            parse("/private/a.jpg", &format!("exp={EXP}")),
            Err(StatusCode::FORBIDDEN)
        );
        // signature for another key
        assert_eq!(
            parse("/private/b.jpg", &private_query("a.jpg", EXP)),
            Err(StatusCode::FORBIDDEN)
        );
        // extended expiry
        let sig = sign::sign(&cfg().secret, "a.jpg", EXP);
        assert_eq!(
            parse("/private/a.jpg", &format!("exp={}&sig={sig}", EXP + 1)),
            Err(StatusCode::FORBIDDEN)
        );
        // expired
        assert_eq!(
            parse("/private/a.jpg", &private_query("a.jpg", NOW)),
            Err(StatusCode::GONE)
        );
        // a forged expired URL is forbidden, not expired
        assert_eq!(
            parse("/private/a.jpg", &format!("exp={NOW}&sig=AAAA")),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn public_cannot_use_private_params() {
        assert_eq!(
            parse("/public/a.jpg", &private_query("a.jpg", EXP)),
            Err(StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn rejects_bad_keys() {
        for path in [
            "/public/",
            "/public/contract.pdf",
            "/private/uploads/2026/09/0b6e_contract.pdf",
            "/public/a.svg",
            "/public/a.jpg.pdf",
            "/public/noextension",
            "/public/a.",
            "/public/../photon-files/a.jpg",
            "/public/a/../../photon-files/a.jpg",
            "/public/./a.jpg",
            "/public/a//b.jpg",
            "/public//a.jpg",
            "/public/a.jpg/",
            "/public/.hidden",
            "/public/a%2F..%2Fb.jpg",
            "/public/%2e%2e/photon-files/a.jpg",
            "/public/a b.jpg",
            "/public/a:b.jpg",
            "/public/a(b).jpg",
            "/public/a,b.jpg",
            "/public/a\\b.jpg",
            "/public/æ.jpg",
            "/unsafe/public/a.jpg",
            "/photon-files/a.jpg",
            "/public",
            "/",
        ] {
            assert_eq!(parse(path, ""), Err(StatusCode::NOT_FOUND), "{path}");
        }
        let long = format!("/public/{}", "a".repeat(MAX_KEY_LEN + 1));
        assert_eq!(parse(&long, ""), Err(StatusCode::NOT_FOUND));
    }

    #[test]
    fn rejects_bad_queries() {
        for query in [
            "w=641",
            "w=0640",
            "w=+640",
            "w=-640",
            "w= 640",
            "w=640&w=320",
            "w=99999999999999999999999",
            "w",
            "h=640",
            "fit=contain",
            "q=80",
            "fmt=gif",
            "original=1",
            "original&original",
            "original&w=640",
            "original&fmt=webp",
            "filters=watermark(photon-files/a.jpg)",
            "watermark=photon-files/a.jpg",
            "fmt=webp)&x=",
            "fmt=webp%29",
        ] {
            assert_eq!(
                parse("/public/a.jpg", query),
                Err(StatusCode::BAD_REQUEST),
                "{query}"
            );
        }
    }

    #[test]
    fn accepts_photon_keys_and_extensions() {
        for path in [
            "/public/uploads/2026/09/9f1c2d4e-1b2a-4c3d-8e9f-0a1b2c3d4e5f_Group_photo.JPG",
            "/public/a.jpeg",
            "/public/a.png",
            "/public/a.webp",
            "/public/a.avif",
            "/public/a.gif",
        ] {
            assert!(parse(path, "").is_ok(), "{path}");
        }
    }

    #[test]
    fn allows_empty_query_segments() {
        assert!(parse("/public/a.jpg", "&w=640&&").is_ok());
    }
}
