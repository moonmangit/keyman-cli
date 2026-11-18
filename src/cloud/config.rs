//! Server settings. Environment variables override the built-in values.

/// Built-in server address (the hosted Keyman project).
pub const DEFAULT_URL: &str = "https://ukcjavxqyfjklagzkuzg.supabase.co";
/// Built-in public (publishable) API key. It is safe to publish: row level security
/// protects the data, and the data is encrypted on the computer before upload.
pub const DEFAULT_KEY: &str = "sb_publishable_Rnllk6qk9z5DR7p4LvihiA_bIkos3o3";

#[derive(Clone, Debug)]
pub struct Config {
    pub url: String,
    pub key: String,
}

pub fn config() -> Config {
    config_from(
        std::env::var("KEYMAN_SUPABASE_URL").ok(),
        std::env::var("KEYMAN_SUPABASE_KEY").ok(),
    )
}

/// Builds the settings from optional overrides. Blank values use the built-in ones.
pub fn config_from(url: Option<String>, key: Option<String>) -> Config {
    let pick = |value: Option<String>, default: &str| match value {
        Some(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => default.to_string(),
    };
    Config {
        url: pick(url, DEFAULT_URL).trim_end_matches('/').to_string(),
        key: pick(key, DEFAULT_KEY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_env_overrides() {
        let c = config_from(Some("http://example.test/".into()), Some("key123".into()));
        assert_eq!(c.url, "http://example.test");
        assert_eq!(c.key, "key123");
        let d = config_from(None, Some("  ".into()));
        assert_eq!(d.url, DEFAULT_URL);
        assert_eq!(d.key, DEFAULT_KEY);
    }
}
