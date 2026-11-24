//! Secret hygiene: masking for UI status, redaction for logs and error text.

use std::fmt;

/// A credential value. `Debug`/`Display` never print the contents.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    /// The only way to read the value; call sites are the authenticated request builders.
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// "r8_…a1F4" style mask that is safe to show in the UI.
pub fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 12 {
        return "••••".into();
    }
    let prefix: String = if key.starts_with("r8_") {
        "r8_".into()
    } else {
        String::new()
    };
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}…{tail}")
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '+' | '/' | '=')
}

/// Removes anything that looks like a credential or a signed URL query from
/// free text (provider error bodies, log lines, panic messages).
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        // URLs: keep scheme://host/path, drop query and fragment (signatures live there).
        if let Some(stripped) = strip_url(rest) {
            out.push_str(&stripped.0);
            rest = &rest[stripped.1..];
            continue;
        }
        // Auth scheme words followed by a token.
        let mut matched = false;
        for word in ["Bearer ", "Token ", "Key ", "bearer ", "token ", "key="] {
            if rest.starts_with(word) {
                let after = &rest[word.len()..];
                let len = after
                    .chars()
                    .take_while(|c| is_token_char(*c))
                    .map(char::len_utf8)
                    .sum::<usize>();
                if len >= 8 {
                    out.push_str(word);
                    out.push_str("***");
                    rest = &after[len..];
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        // Bare tokens: Replicate (r8_...) and fal (uuid:hex) keys, or any long opaque run.
        let len = rest
            .chars()
            .take_while(|c| is_token_char(*c))
            .map(char::len_utf8)
            .sum::<usize>();
        if len > 0 {
            let tok = &rest[..len];
            let looks_secret = tok.starts_with("r8_") && len >= 12
                || (tok.contains(':')
                    && len >= 40
                    && tok.chars().filter(|c| c.is_ascii_hexdigit()).count() >= 32)
                || (len >= 40
                    && tok.chars().any(|c| c.is_ascii_digit())
                    && tok.chars().any(|c| c.is_ascii_uppercase())
                    && tok.chars().any(|c| c.is_ascii_lowercase()));
            if looks_secret {
                out.push_str("***");
            } else {
                out.push_str(tok);
            }
            rest = &rest[len..];
            continue;
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn strip_url(s: &str) -> Option<(String, usize)> {
    let scheme_len = if s.starts_with("https://") {
        8
    } else if s.starts_with("http://") {
        7
    } else {
        return None;
    };
    let end = s
        .char_indices()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ']'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let url = &s[..end];
    let cut = url[scheme_len..]
        .find(['?', '#'])
        .map(|i| i + scheme_len)
        .unwrap_or(url.len());
    let mut kept = url[..cut].to_string();
    if cut < url.len() {
        kept.push_str("?<redacted>");
    }
    Some((kept, end))
}

/// URL form for logs: origin + path only.
pub fn redact_url(u: &url::Url) -> String {
    let mut c = u.clone();
    c.set_query(None);
    c.set_fragment(None);
    let _ = c.set_password(None);
    let _ = c.set_username("");
    c.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_keys() {
        assert_eq!(
            mask_key("r8_abcdefghijklmnopqrstuvwxyz0123456789AB"),
            "r8_…89AB"
        );
        assert_eq!(mask_key("short"), "••••");
        assert_eq!(
            mask_key("0b1c2d3e-aaaa-bbbb-cccc-1234567890ab:0123456789abcdef"),
            "…cdef"
        );
    }

    #[test]
    fn redacts_secrets_and_signed_urls() {
        let fal = "0b1c2d3e-aaaa-bbbb-cccc-1234567890ab:0123456789abcdef0123456789abcdef";
        let text = format!(
            "auth Bearer r8_SECRETsecret123456 and Key {fal} url https://replicate.delivery/x/out.ply?X-Amz-Signature=abc&token=zzz done; bare r8_ABCdef1234567890"
        );
        let r = redact(&text);
        assert!(!r.contains("SECRET"), "{r}");
        assert!(!r.contains("0123456789abcdef0123"), "{r}");
        assert!(!r.contains("X-Amz-Signature"), "{r}");
        assert!(!r.contains("r8_ABC"), "{r}");
        assert!(
            r.contains("https://replicate.delivery/x/out.ply?<redacted>"),
            "{r}"
        );
        assert!(r.contains("done"));
    }

    #[test]
    fn secret_debug_is_masked() {
        let s = Secret::new("r8_topsecretvalue123");
        assert_eq!(format!("{s:?} {s}"), "Secret(***) ***");
    }

    #[test]
    fn keeps_ordinary_text() {
        let t = "Prediction failed: CUDA out of memory (sha 5e3ccfea)";
        assert_eq!(redact(t), t);
    }
}
