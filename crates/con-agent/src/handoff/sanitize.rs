use sha2::{Digest, Sha256};

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Conservative filtering before either private persistence or project staging.
/// Preview is still required: no pattern filter can identify every secret.
pub fn sanitize(text: &str) -> String {
    let mut private_key = false;
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if lower.contains("-----begin ") && lower.contains("private key") {
                private_key = true;
            }
            let sensitive = private_key
                || [
                    "api_key",
                    "api-key",
                    "apikey",
                    "access_token",
                    "refresh_token",
                    "secret_key",
                    "private_key",
                    "xoxb-",
                    "xoxp-",
                    "sk_live_",
                    "sk_test_",
                    "password",
                    "authorization:",
                    "bearer ",
                    "sk-",
                    "ghp_",
                    "github_pat_",
                    "akia",
                    "aws_secret_access_key",
                ]
                .iter()
                .any(|pattern| lower.contains(pattern))
                || contains_jwt(line);
            if lower.contains("-----end ") && lower.contains("private key") {
                private_key = false;
            }
            if sensitive {
                "[redacted: possible credential]".to_owned()
            } else {
                line.chars()
                    .filter(|c| !c.is_control() || *c == '\t')
                    .collect()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Match compact JWT shape, not arbitrary high-entropy strings (images, SHAs).
fn contains_jwt(line: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '.'))
        .any(|token| {
            let mut parts = token.split('.');
            let Some(header) = parts.next() else {
                return false;
            };
            let Some(payload) = parts.next() else {
                return false;
            };
            let Some(signature) = parts.next() else {
                return false;
            };
            header.starts_with("eyJ")
                && header.len() > 3
                && !payload.is_empty()
                && !signature.is_empty()
                && parts.next().is_none()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_credentials_key_bodies_and_terminal_controls() {
        let text = "Goal: 修复 tests\nAPI_KEY=private\n-----BEGIN PRIVATE KEY-----\nbody\n-----END PRIVATE KEY-----\nNext\u{1b}[31m";
        let safe = sanitize(text);
        assert!(safe.contains("Goal: 修复 tests"));
        assert!(!safe.contains("body"));
        assert!(!safe.contains("=private"));
        assert!(!safe.contains('\u{1b}'));
    }

    fn assert_redacted(line: &str) {
        assert_eq!(sanitize(line), "[redacted: possible credential]");
    }

    #[test]
    fn redacts_slack_bot_tokens_previously_missed() {
        assert_redacted("service=xoxb-123-456-private trailing context");
    }

    #[test]
    fn redacts_slack_user_tokens() {
        assert_redacted("xoxp-123-456-private");
    }

    #[test]
    fn redacts_stripe_live_keys() {
        assert_redacted("payment=sk_live_private trailing context");
    }

    #[test]
    fn redacts_stripe_test_keys() {
        assert_redacted("payment=sk_test_private");
    }

    #[test]
    fn redacts_private_key_assignments_without_pem() {
        assert_redacted("export PRIVATE_KEY=abcdef # private signing material");
    }

    #[test]
    fn redacts_bare_jwt() {
        assert_redacted("received eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.c2lnbmF0dXJl done");
    }

    #[test]
    fn redacts_api_key_spelling_variants_as_whole_lines() {
        for name in ["APIKEY", "API_KEY", "apiKey", "api_key", "API-KEY"] {
            assert_redacted(&format!("prefix {name}=private suffix"));
        }
    }

    #[test]
    fn preserves_normal_logs_images_and_commit_hashes() {
        let text = "2026-09-25T10:11:12.123Z INFO complete\ncommit 546ef19ccae75e5a84d8ea3124bdeb7666335a91\ndata:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB\neyJhbGciOiJIUzI1NiJ9\nversion 1.2.3";
        assert_eq!(sanitize(text), text);
    }
}
