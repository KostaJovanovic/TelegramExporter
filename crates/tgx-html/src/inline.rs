//! Inline text rendering: Desktop's `text_entities` list as HTML.
//!
//! Every branch here interpolates attacker-controlled text into markup, and
//! three of them interpolate it into a **JavaScript expression**. Those three
//! (`hashtag`, `cashtag`, `bot_command`) go through [`js_str`], and the two
//! link forms go through [`safe_href`]. Nothing here is escaped by the caller;
//! the escaping is this module's job and it happens on every path.

use crate::escape::{esc, has_safe_scheme, js_str, safe_href};
use serde_json::Value;

/// Render Desktop's `text_entities` list as inline HTML.
///
/// Accepts either the bare-string form of `text` or the segmented list, which
/// is why the writer can pass `text_entities` with `text` as a fallback.
pub fn render_entities(segments: Option<&Value>) -> String {
    let Some(segments) = segments else {
        return String::new();
    };
    match segments {
        Value::String(s) => text_html(s),
        Value::Array(items) => {
            let mut out = String::new();
            for seg in items {
                out.push_str(&render_segment(seg));
            }
            out
        }
        _ => String::new(),
    }
}

fn text_html(raw: &str) -> String {
    esc(raw).replace('\n', "<br>")
}

fn render_segment(seg: &Value) -> String {
    if let Value::String(s) = seg {
        return text_html(s);
    }
    let Some(map) = seg.as_object() else {
        return String::new();
    };
    let kind = map.get("type").and_then(Value::as_str).unwrap_or("plain");
    let raw = map.get("text").and_then(Value::as_str).unwrap_or("");
    let text = text_html(raw);
    let attr = |k: &str| map.get(k).and_then(Value::as_str).unwrap_or("");

    match kind {
        "plain" => text,
        "bold" => format!("<strong>{text}</strong>"),
        "italic" => format!("<em>{text}</em>"),
        "underline" => format!("<u>{text}</u>"),
        "strikethrough" => format!("<s>{text}</s>"),
        "spoiler" => {
            format!("<span class=\"spoiler hidden\" onclick=\"ShowSpoiler(this)\">{text}</span>")
        }
        "code" => format!("<code>{text}</code>"),
        "pre" => format!("<pre>{text}</pre>"),
        "blockquote" => format!("<blockquote>{text}</blockquote>"),
        "text_link" => match safe_href(attr("href")) {
            Some(href) => format!("<a href=\"{}\">{text}</a>", esc(&href)),
            // Unsafe scheme: show the text plus the raw target, both inert, so
            // the archive stays faithful without becoming clickable. This is
            // the shape Desktop already uses for a file it did not save, so
            // parity is unaffected.
            None => format!(
                "{text} <span class=\"details\">[{}]</span>",
                esc(attr("href"))
            ),
        },
        "link" => match safe_href(raw) {
            Some(href) => {
                let href = if has_safe_scheme(&href) {
                    href
                } else {
                    format!("https://{href}") // bare t.me/foo
                };
                format!("<a href=\"{}\">{text}</a>", esc(&href))
            }
            None => text,
        },
        "email" => format!("<a href=\"mailto:{text}\">{text}</a>"),
        "phone" => format!("<a href=\"tel:{text}\">{text}</a>"),
        "mention" => format!(
            "<a href=\"https://t.me/{}\">{text}</a>",
            esc(raw.trim_start_matches('@'))
        ),
        "mention_name" => format!("<a href=\"\" onclick=\"return ShowMentionName()\">{text}</a>"),
        "hashtag" => format!(
            "<a href=\"\" onclick=\"return ShowHashtag({})\">{text}</a>",
            esc(&js_str(raw.trim_start_matches('#')))
        ),
        "cashtag" => format!(
            "<a href=\"\" onclick=\"return ShowCashtag({})\">{text}</a>",
            esc(&js_str(raw.trim_start_matches('$')))
        ),
        "bot_command" => format!(
            "<a href=\"\" onclick=\"return ShowBotCommand({})\">{text}</a>",
            esc(&js_str(raw.trim_start_matches('/')))
        ),
        "custom_emoji" => {
            let doc = map
                .get("document_id")
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            // Desktop points a custom emoji at the sticker file it downloaded
            // for it, and at a toast when there is none. Its own output writes
            // the spaced `href = "..."` form, kept for an exact diff.
            let href = if !doc.is_empty() && (doc.contains('/') || doc.contains('.')) {
                safe_href(&doc)
            } else {
                None
            };
            match href {
                Some(h) => format!("<a href = \"{}\">{text}</a>", esc(&h)),
                None => {
                    format!("<a href=\"\" onclick=\"return ShowNotLoadedEmoji()\">{text}</a>")
                }
            }
        }
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r(v: Value) -> String {
        render_entities(Some(&v))
    }

    #[test]
    fn newlines_become_breaks() {
        assert_eq!(r(json!("a\nb")), "a<br>b");
    }

    #[test]
    fn a_bare_string_is_escaped() {
        assert_eq!(r(json!("<script>")), "&lt;script&gt;");
    }

    #[test]
    fn every_formatting_type_has_its_tag() {
        let cases = [
            ("bold", "<strong>x</strong>"),
            ("italic", "<em>x</em>"),
            ("underline", "<u>x</u>"),
            ("strikethrough", "<s>x</s>"),
            ("code", "<code>x</code>"),
            ("pre", "<pre>x</pre>"),
            ("blockquote", "<blockquote>x</blockquote>"),
        ];
        for (kind, want) in cases {
            assert_eq!(r(json!([{ "type": kind, "text": "x" }])), want);
        }
    }

    #[test]
    fn a_text_link_with_a_dangerous_scheme_is_inert_but_still_shown() {
        let out =
            r(json!([{ "type": "text_link", "text": "click", "href": "javascript:alert(1)" }]));
        assert!(!out.contains("<a "), "got {out}");
        assert!(out.contains("click"), "got {out}");
        // The archive stays faithful: the target is visible as text.
        assert!(out.contains("javascript:alert(1)"), "got {out}");
    }

    #[test]
    fn a_safe_text_link_is_a_link() {
        let out = r(json!([{ "type": "text_link", "text": "x", "href": "https://a.b" }]));
        assert_eq!(out, "<a href=\"https://a.b\">x</a>");
    }

    #[test]
    fn a_bare_domain_link_gets_https() {
        let out = r(json!([{ "type": "link", "text": "t.me/foo" }]));
        assert_eq!(out, "<a href=\"https://t.me/foo\">t.me/foo</a>");
    }

    #[test]
    fn a_link_that_is_already_absolute_is_left_alone() {
        let out = r(json!([{ "type": "link", "text": "https://t.me/foo" }]));
        assert_eq!(out, "<a href=\"https://t.me/foo\">https://t.me/foo</a>");
    }

    #[test]
    fn the_three_js_handlers_quote_their_argument() {
        // A hashtag is interpolated into an expression, so escaping is not
        // enough — the quote has to survive as an escaped JS quote.
        let out = r(json!([{ "type": "hashtag", "text": "#a'b" }]));
        assert!(out.contains("ShowHashtag("), "got {out}");
        // The apostrophe is JS-escaped and then HTML-escaped.
        assert!(out.contains("\\&apos;"), "got {out}");
        assert!(!out.contains("');alert("), "got {out}");
    }

    #[test]
    fn a_hashtag_carrying_a_break_out_payload_cannot_break_out() {
        let out = r(json!([{ "type": "hashtag", "text": "#x'); alert(1); //" }]));

        // Do NOT substring-match for "alert(1)": it appears twice here and is
        // inert both times — once inside the JS string literal and once as the
        // link's escaped text. Asserting its absence is a false failure, which
        // is the exact trap the Python suite documents.
        //
        // The property that actually matters is that the JS string cannot be
        // terminated early, so decode the attribute and check the quoting.
        let onclick = out
            .split("onclick=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("an onclick attribute");
        let decoded = onclick.replace("&apos;", "'").replace("&quot;", "\"");

        // Shape: return ShowHashtag('...')
        let arg = decoded
            .trim_start_matches("return ShowHashtag(")
            .trim_end_matches(')');
        assert!(arg.starts_with('\'') && arg.ends_with('\''), "got {arg}");

        // Every quote inside the literal is backslash-escaped, so the literal
        // runs to its own closing quote and the payload never becomes code.
        let inner = &arg[1..arg.len() - 1];
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                chars.next(); // skip whatever it escapes
            } else {
                assert_ne!(c, '\'', "an unescaped quote ends the literal: {inner}");
            }
        }
    }

    #[test]
    fn spoilers_carry_desktops_own_handler() {
        let out = r(json!([{ "type": "spoiler", "text": "s" }]));
        assert_eq!(
            out,
            "<span class=\"spoiler hidden\" onclick=\"ShowSpoiler(this)\">s</span>"
        );
    }

    #[test]
    fn a_custom_emoji_with_a_file_uses_desktops_spaced_href() {
        // Desktop really does write `href = "..."` with spaces here.
        let out =
            r(json!([{ "type": "custom_emoji", "text": "e", "document_id": "stickers/a.webp" }]));
        assert_eq!(out, "<a href = \"stickers/a.webp\">e</a>");
    }

    #[test]
    fn a_custom_emoji_without_a_file_falls_back_to_the_toast() {
        let out = r(json!([{ "type": "custom_emoji", "text": "e", "document_id": "12345" }]));
        assert!(out.contains("ShowNotLoadedEmoji"), "got {out}");
    }

    #[test]
    fn a_mention_strips_its_at_sign_from_the_href_only() {
        let out = r(json!([{ "type": "mention", "text": "@kosta" }]));
        assert_eq!(out, "<a href=\"https://t.me/kosta\">@kosta</a>");
    }

    #[test]
    fn an_unknown_entity_type_degrades_to_plain_text() {
        // Telegram adds entity types faster than any exporter follows them.
        let out = r(json!([{ "type": "something_new_in_2027", "text": "<x>" }]));
        assert_eq!(out, "&lt;x&gt;");
    }

    #[test]
    fn segments_concatenate_without_separators() {
        let out = r(json!([
            { "type": "plain", "text": "a" },
            { "type": "bold", "text": "b" },
            { "type": "plain", "text": "c" },
        ]));
        assert_eq!(out, "a<strong>b</strong>c");
    }

    #[test]
    fn nothing_renders_to_nothing() {
        assert_eq!(render_entities(None), "");
        assert_eq!(r(json!([])), "");
        assert_eq!(r(json!(null)), "");
    }
}
