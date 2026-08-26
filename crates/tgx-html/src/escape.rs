//! Escaping, and the allowlist that decides what may become a link.
//!
//! **An exported archive opens as a local file**, so anything that survives
//! into it executes with that file's origin. Every string in an export is
//! attacker-controlled — a display name, a sticker emoji, an ID3 tag, a
//! filename, a link target — so there are exactly three rules and no
//! exceptions:
//!
//! 1. Everything interpolated into markup goes through [`esc`].
//! 2. Every href goes through [`safe_href`]. *Every* one — the media rows and
//!    the inline previews included. Five hrefs in the Python original were
//!    escaped but never scheme-checked, and sat green for months because the
//!    test that should have caught them fed markup to a URL field, which
//!    passes the scheme check vacuously.
//! 3. **Escaping is not enough inside a JavaScript expression.** Reproducing
//!    Desktop's markup means emitting `onclick="return GoToMessage(12)"`, and
//!    an id interpolated there is code, not text. See [`message_number`].

/// Escape for HTML the way Desktop does.
///
/// Python's `html.escape` turns an apostrophe into `&#x27;` where Desktop
/// writes `&apos;`. Both are correct HTML and both render identically; the
/// named form is used so output diffs clean against a real export.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Escape an attribute the way Desktop does, control characters included.
///
/// Desktop's poll total really is `class="total details&#x09;"` — a stray tab
/// in the class name, escaped rather than emitted raw. Reproducing that is not
/// politeness; the file is compared line by line.
pub fn attr_value(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in esc(text).chars() {
        match ch {
            '\t' => out.push_str("&#x09;"),
            '\n' => out.push_str("&#x0a;"),
            '\r' => out.push_str("&#x0d;"),
            c => out.push(c),
        }
    }
    out
}

/// Schemes allowed to survive into an href.
const SAFE_SCHEMES: [&str; 6] = [
    "http://", "https://", "mailto:", "tel:", "ftp://", "ftps://",
];

/// Return `url` if it is safe to put in an href, else `None`.
///
/// Browsers ignore control characters *and whitespace* inside a scheme, so both
/// `java\tscript:alert(1)` and `java script:alert(1)` execute. The decision is
/// therefore taken against a fully collapsed copy.
///
/// **What comes back keeps its spaces.** A media path may legitimately contain
/// one — Desktop really does write `stickers/sticker (55).webp` — and
/// collapsing those turned every such link into a dangling reference.
/// Squeezing whitespace out of an accepted URL cannot make it dangerous,
/// because the collapsed form is the one that passed the scheme test.
pub fn safe_href(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    let cleaned: String = url
        .chars()
        .filter(|c| (*c as u32) >= 0x20 && *c != '\u{7f}')
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        return None;
    }
    let collapsed: String = cleaned.split_whitespace().collect();
    let lowered = collapsed.to_lowercase();
    if SAFE_SCHEMES.iter().any(|s| lowered.starts_with(s)) {
        return Some(cleaned);
    }
    // No scheme at all (t.me/foo, ./photos/x.jpg) is a relative URL: safe.
    match collapsed.split_once(':') {
        None => Some(cleaned),
        Some((scheme, _)) => {
            if scheme.contains('/') || scheme.contains('?') || scheme.contains('#') {
                Some(cleaned)
            } else {
                None
            }
        }
    }
}

/// A message id that is safe to interpolate into `GoToMessage(...)`.
///
/// Anything that is not a whole number is rejected and the link is dropped
/// instead. An id reaching an inline handler is **code**, not text, and no
/// amount of HTML escaping makes it safe there.
pub fn message_number(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// A single-quoted JS string literal for an inline handler.
///
/// Used for the hashtag / cashtag / bot-command handlers, whose argument is
/// attacker-controlled text going into an expression.
pub fn js_str(value: &str) -> String {
    let body: String = value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .chars()
        .filter(|c| (*c as u32) >= 0x20)
        .collect();
    format!("'{body}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apostrophe_is_named_not_numeric() {
        // Desktop writes &apos;. Python's html.escape writes &#x27;, which
        // renders the same and diffs on every line carrying one.
        assert_eq!(esc("it's"), "it&apos;s");
    }

    #[test]
    fn ampersand_is_escaped_first() {
        // Escaping < before & would double-escape into &amp;lt;.
        assert_eq!(esc("&<>\""), "&amp;&lt;&gt;&quot;");
    }

    #[test]
    fn a_script_payload_is_inert() {
        let out = esc("<img src=x onerror=alert(1)>");
        assert!(!out.contains('<'));
        assert!(out.contains("&lt;img"));
    }

    #[test]
    fn attribute_control_characters_become_references() {
        // Desktop's own poll total carries a tab in its class name.
        assert_eq!(attr_value("total details\t"), "total details&#x09;");
        assert_eq!(attr_value("a\nb\rc"), "a&#x0a;b&#x0d;c");
    }

    // ---- safe_href --------------------------------------------------------

    #[test]
    fn dangerous_schemes_are_refused() {
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html;base64,PHNjcmlwdD4=",
            "vbscript:msgbox(1)",
        ] {
            assert_eq!(safe_href(bad), None, "{bad} was accepted");
        }
    }

    #[test]
    fn whitespace_and_control_chars_cannot_smuggle_a_scheme() {
        // Browsers execute both of these; the decision is taken against a
        // fully collapsed copy for exactly that reason.
        assert_eq!(safe_href("java\tscript:alert(1)"), None);
        assert_eq!(safe_href("java script:alert(1)"), None);
        assert_eq!(safe_href("java\nscript:alert(1)"), None);
        assert_eq!(safe_href("\u{1}javascript:alert(1)"), None);
    }

    #[test]
    fn an_accepted_url_keeps_its_spaces() {
        // This is the one that broke every real media path when it was
        // collapsed: Desktop writes "stickers/sticker (55).webp".
        assert_eq!(
            safe_href("stickers/sticker (55).webp").as_deref(),
            Some("stickers/sticker (55).webp")
        );
    }

    #[test]
    fn ordinary_links_survive() {
        for good in [
            "https://example.com/a?b=1#c",
            "http://t.me/foo",
            "mailto:a@b.c",
            "tel:+3811234567",
            "ftp://files.example.com/x",
            "t.me/foo",
            "./photos/photo_1.jpg",
            "messages2.html#go_to_message12",
        ] {
            assert!(safe_href(good).is_some(), "{good} was refused");
        }
    }

    #[test]
    fn an_empty_or_blank_target_is_no_link() {
        assert_eq!(safe_href(""), None);
        assert_eq!(safe_href("   "), None);
        assert_eq!(safe_href("\u{1}\u{2}"), None);
    }

    // ---- the JS boundary --------------------------------------------------

    #[test]
    fn only_a_whole_number_reaches_go_to_message() {
        assert_eq!(message_number(&json!(12)), Some(12));
        assert_eq!(message_number(&json!("12")), Some(12));
        // Everything else drops the link rather than interpolating code.
        assert_eq!(message_number(&json!("12); alert(1); //")), None);
        assert_eq!(message_number(&json!("1.5")), None);
        assert_eq!(message_number(&json!(true)), None);
        assert_eq!(message_number(&json!(null)), None);
        assert_eq!(message_number(&json!({})), None);
    }

    #[test]
    fn js_strings_are_quoted_and_stripped() {
        assert_eq!(js_str("plain"), "'plain'");
        assert_eq!(js_str("it's"), r"'it\'s'");
        assert_eq!(js_str(r"back\slash"), r"'back\\slash'");
        // A newline would end the statement.
        assert_eq!(js_str("a\nb"), "'ab'");
    }
}
