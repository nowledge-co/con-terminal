//! Strict projection of models_cache.json; never open auth or configuration.
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

pub(super) fn path() -> Option<PathBuf> {
    cache_path(std::env::var_os("CODEX_HOME"), std::env::var_os("HOME"))
}

fn cache_path(
    codex_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    codex_home
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|v| !v.is_empty())
                .map(|v| PathBuf::from(v).join(".codex"))
        })
        .map(|dir| dir.join("models_cache.json"))
}

#[derive(Deserialize)]
struct Cache {
    fetched_at: chrono::DateTime<chrono::Utc>,
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    slug: String,
    visibility: Option<String>,
}

fn parse(text: &[u8]) -> Option<(Vec<String>, bool)> {
    let cache: Cache = serde_json::from_slice(text).ok()?;
    let stale = chrono::Utc::now()
        .signed_duration_since(cache.fetched_at)
        .to_std()
        .is_ok_and(|age| age >= super::CACHE_TTL);
    let mut models = Vec::new();
    for model in cache.models {
        if model.visibility.as_deref().is_none_or(|v| v == "list") {
            super::push_candidate(&mut models, &model.slug);
        }
    }
    Some((models, stale))
}

pub(super) async fn read(path: &Path) -> Vec<String> {
    // Native model metadata can be larger than CLI listings. Bound disk reads.
    const LIMIT: u64 = 4 * 1024 * 1024;
    let Ok(file) = tokio::fs::File::open(path).await else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    if file.take(LIMIT + 1).read_to_end(&mut bytes).await.is_err() || bytes.len() as u64 > LIMIT {
        return Vec::new();
    }
    let Some((models, stale)) = parse(&bytes) else {
        return Vec::new();
    };
    if stale {
        // Best-effort candidates, never an assertion that the service accepts them.
        log::debug!("handoff Codex model candidates degraded: local cache older than 600s");
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_projection_excludes_credentials_and_hidden_or_invalid_slugs() {
        let fixture = include_bytes!("codex_cache.fixture.json");
        let (models, stale) = parse(fixture).unwrap();
        assert_eq!(models, ["gpt-example", "gpt-optional"]);
        assert!(stale);
        assert!(!format!("{models:?}").contains("SECRET"));
        assert!(parse(b"bad JSON").is_none());
        assert!(parse(br#"{"fetched_at":"invalid","models":[]}"#).is_none());
    }

    #[test]
    fn cache_path_honors_nonempty_codex_home() {
        assert_eq!(
            cache_path(Some("/custom".into()), Some("/home".into())).unwrap(),
            PathBuf::from("/custom/models_cache.json")
        );
        assert_eq!(
            cache_path(Some("".into()), Some("/home".into())).unwrap(),
            PathBuf::from("/home/.codex/models_cache.json")
        );
        assert!(cache_path(None, None).is_none());
    }

    #[tokio::test]
    async fn missing_file_and_bad_json_degrade_without_a_subprocess() {
        let dir = std::env::temp_dir().join(format!("con-codex-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("models_cache.json");
        assert!(read(&path).await.is_empty());
        std::fs::write(&path, b"invalid").unwrap();
        assert!(read(&path).await.is_empty());
        std::fs::write(&path, include_bytes!("codex_cache.fixture.json")).unwrap();
        assert_eq!(read(&path).await, ["gpt-example", "gpt-optional"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
