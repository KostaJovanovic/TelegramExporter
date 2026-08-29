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
//!    the inline previews included. An href that is escaped but never
//!    scheme-checked sits green indefinitely, because the test that should
//!    catch it feeds markup to a URL field — which passes the scheme check
//!    vacuously.
//! 3. **Escaping is not enough inside a JavaScript expression.** Reproducing
//!    Desktop's markup means emitting `onclick="return GoToMessage(12)"`, and
//!    an id interpolated there is code, not text. See [`message_number`].

/// Escape for HTML the way Desktop does.
///
/// Desktop writes `&apos;` for an apostrophe where most escapers emit `&#x27;`.
/// Both are correct HTML and both render identically; the named form is used so
/// output diffs clean against a real export.
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
///
/// **Declared once.** `inline.rs` kept a byte-identical copy of this list for
/// its "is this link already absolute?" test, so adding a scheme in one place
/// and not the other would have left a URL that [`safe_href`] accepts getting
/// `https://` glued to its front.
const SAFE_SCHEMES: [&str; 6] = [
    "http://", "https://", "mailto:", "tel:", "ftp://", "ftps://",
];

/// Does this URL already carry a scheme from [`SAFE_SCHEMES`]?
///
/// For callers deciding whether a URL is absolute. It is **not** a safety test:
/// only [`safe_href`] is, because a scheme can be smuggled past a naive prefix
/// check with whitespace or control characters.
pub fn has_safe_scheme(url: &str) -> bool {
    let lowered = url.to_lowercase();
    SAFE_SCHEMES.iter().any(|s| lowered.starts_with(s))
}

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
    // **Before the scheme test, because these have no scheme to test.** An
    // archive is opened as `file://`, where `//host/x` and `\\host\share` both
    // resolve against that scheme to `file://host/…` — on Windows a UNC path,
    // so following one opens an SMB connection to a host the message chose and
    // leaks an NTLM handshake to it. A `text_link` entity carries an arbitrary
    // URL, so this is reachable from any message. Neither form is a relative
    // URL despite having no `scheme:` prefix, which is exactly how both slipped
    // through the branch below.
    //
    // A backslash is not a path separator in a URL, but browsers normalise it
    // to one before resolving, so `/\host` reaches the same place as `//host`.
    // All four mixtures are one pattern; the explicit `//` and `\\` prefix
    // tests that used to sit here were a subset of it and never fired.
    if matches!(collapsed.as_bytes(), [b'/' | b'\\', b'/' | b'\\', ..]) {
        return None;
    }
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
        // Desktop writes &apos;. The numeric &#x27; renders the same and diffs
        // on every line carrying one.
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

    #[test]
    fn a_host_relative_target_cannot_reach_a_remote_host() {
        // The archive is opened as `file://`, so every one of these resolves to
        // `file://evil.example/…` — a UNC path on Windows. Following one opens
        // an SMB connection to a host the *message* chose and hands it an NTLM
        // handshake. They have no `scheme:`, so the relative-URL branch waved
        // them through.
        for bad in [
            "//evil.example/x",
            r"\\evil.example\share",
            r"/\evil.example/x",
            r"\/evil.example/x",
            "//evil.example",
            "  //evil.example/x  ",
            "/\t/evil.example/x",
        ] {
            assert_eq!(safe_href(bad), None, "{bad} was accepted");
        }
    }

    #[test]
    fn the_absolute_test_and_the_safety_test_read_one_list() {
        // `inline.rs` used to keep its own copy of SAFE_SCHEMES to decide
        // whether a link already had a scheme. A scheme added to one list and
        // not the other means a URL safe_href accepts gets "https://" glued to
        // its front, so the two views have to be the same list.
        for good in ["ftps://a.b/c", "FTPS://a.b/c", "mailto:a@b.c", "tel:+381"] {
            assert!(has_safe_scheme(good), "{good}");
            assert!(safe_href(good).is_some(), "{good}");
        }
        // Relative targets are safe but not absolute — that is the whole
        // distinction inline.rs needs.
        for relative in ["t.me/foo", "./photos/x.jpg", "messages2.html#go1"] {
            assert!(!has_safe_scheme(relative), "{relative}");
            assert!(safe_href(relative).is_some(), "{relative}");
        }
        // And it is not a safety test on its own.
        assert!(!has_safe_scheme("javascript:alert(1)"));
    }

    #[test]
    fn a_single_leading_slash_is_still_an_ordinary_relative_path() {
        // Only the doubled form names a host. One slash is a path.
        assert!(safe_href("/photos/photo_1.jpg").is_some());
        assert!(safe_href("/a//b").is_some());
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
