//! Server settings. Environment variables override the built-in values.

/// Built-in server address (set before release).
pub const DEFAULT_URL: &str = "https://YOUR-PROJECT.supabase.co";
/// Built-in public (publishable) API key (set before release).
pub const DEFAULT_KEY: &str = "YOUR-PUBLISHABLE-KEY";

#[derive(Clone, Debug)]
pub struct Config {
    pub url: String,
    pub key: String,
}

pub fn config() -> Config {
    let pick = |name: &str, default: &str| match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => default.to_string(),
    };
    Config {
        url: pick("KEYMAN_SUPABASE_URL", DEFAULT_URL)
            .trim_end_matches('/')
            .to_string(),
        key: pick("KEYMAN_SUPABASE_KEY", DEFAULT_KEY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_env_overrides() {
        std::env::set_var("KEYMAN_SUPABASE_URL", "http://example.test/");
        std::env::set_var("KEYMAN_SUPABASE_KEY", "key123");
        let c = config();
        assert_eq!(c.url, "http://example.test");
        assert_eq!(c.key, "key123");
        std::env::remove_var("KEYMAN_SUPABASE_URL");
        std::env::remove_var("KEYMAN_SUPABASE_KEY");
        let d = config();
        assert_eq!(d.url, DEFAULT_URL);
        assert_eq!(d.key, DEFAULT_KEY);
    }
}
