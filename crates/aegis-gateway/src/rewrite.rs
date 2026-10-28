//! Header rewriting.
//!
//! Replaces a placeholder credential in a header value with its real secret,
//! including the password field of HTTP basic authentication.

#![allow(dead_code)]

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Replace a placeholder credential in a header value with its real secret.
///
/// Basic authentication values are decoded so that the password field can be
/// replaced before the value is encoded again. Other values have the
/// standalone placeholder token replaced in place.
///
/// Returns `None` when the value is unchanged, including when the value is
/// malformed or does not contain the placeholder.
pub fn rewrite_header(value: &str, placeholder: &str, secret: &str) -> Option<String> {
    if value
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("basic "))
    {
        return rewrite_basic(&value[6..], placeholder, secret);
    }

    replace_standalone(value, placeholder, secret)
}

fn rewrite_basic(encoded: &str, placeholder: &str, secret: &str) -> Option<String> {
    let decoded = STANDARD.decode(encoded).ok()?;
    let credentials = std::str::from_utf8(&decoded).ok()?;
    let (username, password) = credentials.split_once(':')?;

    if password != placeholder {
        return None;
    }

    let rewritten = format!("{username}:{secret}");
    Some(format!("Basic {}", STANDARD.encode(rewritten)))
}

fn replace_standalone(value: &str, placeholder: &str, secret: &str) -> Option<String> {
    if placeholder.is_empty() {
        return None;
    }

    let mut rewritten = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut replaced = false;

    for (index, _) in value.match_indices(placeholder) {
        let end = index + placeholder.len();
        let starts_standalone = value[..index]
            .chars()
            .next_back()
            .is_none_or(|character| !is_token_character(character));
        let ends_standalone = value[end..]
            .chars()
            .next()
            .is_none_or(|character| !is_token_character(character));

        if starts_standalone && ends_standalone {
            rewritten.push_str(&value[cursor..index]);
            rewritten.push_str(secret);
            cursor = end;
            replaced = true;
        }
    }

    if replaced {
        rewritten.push_str(&value[cursor..]);
        Some(rewritten)
    } else {
        None
    }
}

fn is_token_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    use super::rewrite_header;

    fn encode(value: &str) -> String {
        STANDARD.encode(value)
    }

    #[test]
    fn basic_auth_replaces_matching_password() {
        let value = format!("Basic {}", encode("x-access-token:placeholder"));
        let expected = format!("Basic {}", encode("x-access-token:real"));
        assert_eq!(
            rewrite_header(&value, "placeholder", "real"),
            Some(expected)
        );
    }

    #[test]
    fn bearer_replaces_placeholder() {
        assert_eq!(
            rewrite_header("Bearer placeholder", "placeholder", "real"),
            Some("Bearer real".to_string())
        );
    }

    #[test]
    fn token_replaces_placeholder() {
        assert_eq!(
            rewrite_header("token placeholder", "placeholder", "real"),
            Some("token real".to_string())
        );
    }

    #[test]
    fn malformed_basic_auth_is_unchanged() {
        assert_eq!(
            rewrite_header("Basic !!!not-base64!!!", "placeholder", "real"),
            None
        );
    }

    #[test]
    fn basic_auth_with_other_password_is_unchanged() {
        let value = format!("Basic {}", encode("x-access-token:other"));
        assert_eq!(rewrite_header(&value, "placeholder", "real"), None);
    }

    #[test]
    fn basic_auth_is_case_insensitive() {
        let value = format!("basic {}", encode("user:placeholder"));
        let expected = format!("Basic {}", encode("user:real"));
        assert_eq!(
            rewrite_header(&value, "placeholder", "real"),
            Some(expected)
        );
    }

    #[test]
    fn value_without_placeholder_is_unchanged() {
        assert_eq!(rewrite_header("Bearer other", "placeholder", "real"), None);
    }

    #[test]
    fn embedded_placeholder_is_unchanged() {
        assert_eq!(
            rewrite_header("Bearer prefixplaceholder", "placeholder", "real"),
            None
        );
    }
}
