//! Magpie gateway defaults shared by inference and model discovery.
use crate::ProviderConfig;
use anyhow::{Result, bail};

pub const BASE_URL: &str = "http://127.0.0.1:3425/v1";
pub const USER_AGENT: &str = concat!("Con/", env!("CARGO_PKG_VERSION"));
// This is an attribution token, not a vendor credential. Remote gateways need
// a user-issued gateway key instead; they reject this local marker.
const LOCAL_API_KEY: &str = "magpie-con";

pub fn api_key(config: &ProviderConfig) -> Result<String> {
    resolve_api_key(config, std::env::var("MAGPIE_API_KEY").ok().as_deref())
}

fn resolve_api_key(config: &ProviderConfig, default_env: Option<&str>) -> Result<String> {
    if let Some(key) = crate::provider::configured_provider_api_key_value(Some(config)) {
        return Ok(key);
    }
    if let Some(name) = config
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        bail!("Environment variable {name} is not set or is empty.");
    }
    Ok(default_env
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(LOCAL_API_KEY)
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_key_precedence_and_local_attribution() {
        let mut config = ProviderConfig::default();
        assert_eq!(resolve_api_key(&config, None).unwrap(), "magpie-con");
        assert_eq!(resolve_api_key(&config, Some("  ")).unwrap(), "magpie-con");
        assert_eq!(
            resolve_api_key(&config, Some(" gateway-key ")).unwrap(),
            "gateway-key"
        );
        config.api_key = Some(" explicit-key ".into());
        assert_eq!(
            resolve_api_key(&config, Some("env-key")).unwrap(),
            "explicit-key"
        );
        config.api_key = None;
        config.api_key_env = Some("CON_TEST_MAGPIE_MISSING_KEY_7CA02740".into());
        assert!(resolve_api_key(&config, Some("fallback")).is_err());
    }
}
