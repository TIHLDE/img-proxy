use std::{env, net::SocketAddr};

#[derive(Clone, Debug)]
pub struct Config {
    /// Address the proxy listens on, e.g. `0.0.0.0:8080`.
    pub listen: SocketAddr,

    /// Base URL of the internal imagor instance, e.g. `http://127.0.0.1:8000`.
    pub imagor_url: String,

    /// Shared HMAC secret, also known by the backend that issues private URLs.
    pub secret: Vec<u8>,

    /// Bucket served under `/public/...`.
    pub public_bucket: String,

    /// Bucket served under `/private/...`.
    pub private_bucket: String,

    /// Allowed values for `w` and `h`. Anything else is rejected.
    pub sizes: Vec<u32>,

    /// Upper bound in seconds for `Cache-Control: max-age` on private responses.
    pub private_max_age: u64,
}

impl Config {
    /// Parses the environment variables into the config values
    ///
    /// # Errors
    ///
    /// Returns a description of the problem if a required variable is missing
    /// or a variable has an invalid value.
    pub fn from_env() -> Result<Self, String> {
        let secret = required("SIGNING_SECRET")?;
        if secret.len() < 32 {
            return Err("SIGNING_SECRET must be at least 32 bytes".into());
        }

        let sizes = optional("ALLOWED_SIZES", "480,640,1280,1920,2560")
            .split(',')
            .map(|s| {
                s.trim()
                    .parse::<u32>()
                    .map_err(|e| format!("ALLOWED_SIZES: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        if sizes.iter().any(|&s| s == 0 || s > 8192) {
            return Err("ALLOWED_SIZES values must be between 1 and 8192".into());
        }

        let listen = optional("LISTEN_ADDR", "0.0.0.0:8080")
            .parse::<SocketAddr>()
            .map_err(|e| format!("LISTEN_ADDR: {e}"))?;

        let imagor_url = optional("IMAGOR_URL", "http://127.0.0.1:8000")
            .trim_end_matches('/')
            .to_string();

        let public_bucket = optional("PUBLIC_BUCKET", "public");

        let private_bucket = optional("PRIVATE_BUCKET", "private");

        // this defaults to one day in the browser cache
        let private_max_age = optional("PRIVATE_MAX_AGE", "86400")
            .parse::<u64>()
            .map_err(|e| format!("PRIVATE_MAX_AGE: {e}"))?;

        Ok(Self {
            secret: secret.into_bytes(),
            listen,
            imagor_url,
            public_bucket,
            private_bucket,
            sizes,
            private_max_age,
        })
    }
}

/// Gets a env variable and fails if not present
fn required(name: &str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} is required"))
}

/// Gets a env variable and returns it's default value if not found
fn optional(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}
