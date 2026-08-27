//! The emitter, pinned byte for byte against a real `result.json`.
//!
//! Four things define it, all recovered from a reference export:
//!
//! * **One-space indent.** `JSON_INDENT = 1`.
//! * **Raw UTF-8**, not `\uXXXX` escapes. Python's `ensure_ascii=False`;
//!   serde_json's default.
//! * **Desktop's key order** — see [`crate::order`].
//! * **`reactions` is indented one level too deep**, a quirk of Desktop's own
//!   writer that is part of the format because the file is compared line by
//!   line.
//!
//! The escaping question is the one that can hide a silent byte difference, so
//! it is tested directly rather than reasoned about: see the tests at the foot
//! of this file, which pin every case where `json.dumps(ensure_ascii=False)`
//! and `serde_json` could conceivably disagree.

use serde::Serialize;
use serde_json::{Map, Value};

/// Desktop indents with a single space.
pub const INDENT: usize = 1;

/// Serialise with Desktop's formatting: one-space indent, raw UTF-8.
pub fn to_string(value: &Value) -> String {
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    value
        .serialize(&mut ser)
        .expect("serialising a Value cannot fail");
    // Serialising a Value only ever produces UTF-8.
    String::from_utf8(buf).expect("serde_json emits UTF-8")
}

/// Reproduce the one place Desktop indents an array a level too deep.
///
/// In a real `result.json` every array sits at its key's depth except
/// `reactions`, whose elements are pushed one space further and whose closing
/// bracket moves with them.
///
/// The block that moves is found by matching the array's own brackets, not by
/// running to the object's final brace. `reactions` is the last *ranked* key
/// ([`crate::order::TAIL_ORDER`]), but an unranked one — a key Telegram added
/// that nobody here has classified — sorts after it
/// ([`crate::order::ordered`]), and the run-to-the-brace form pushed those
/// keys a space right as well, silently corrupting the indentation of the one
/// kind of key a reader is most likely to be looking at in a diff.
pub fn desktop_reaction_indent(lines: Vec<String>) -> Vec<String> {
    if lines.len() < 2 {
        return lines;
    }
    let Some(at) = lines
        .iter()
        .position(|l| l.trim_start().starts_with("\"reactions\": ["))
    else {
        return lines;
    };
    // The pretty-printer closes an array at its opening line's indent, and no
    // value line can begin with `]` (a JSON string never spans lines), so the
    // first such line is the array's own bracket. An empty `"reactions": []`
    // has no closing line and nothing to move.
    let depth = indent_of(&lines[at]);
    let Some(close) = lines
        .iter()
        .enumerate()
        .skip(at + 1)
        .find(|(_, l)| l.trim_start().starts_with(']') && indent_of(l) == depth)
        .map(|(i, _)| i)
    else {
        return lines;
    };
    lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            if i > at && i <= close {
                format!(" {l}")
            } else {
                l
            }
        })
        .collect()
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// One message, rendered as it appears inside the streamed `messages` array.
///
/// The message is serialised standalone, every line is then prefixed by
/// `INDENT * 2` spaces, and the reaction quirk is applied last — which is the
/// order Desktop's own writer produces and the only order that reproduces it.
pub fn message_block(body: &Map<String, Value>) -> String {
    let text = to_string(&Value::Object(body.clone()));
    let pad = " ".repeat(INDENT * 2);
    let lines: Vec<String> = text.lines().map(|l| format!("{pad}{l}")).collect();
    desktop_reaction_indent(lines).join("\n")
}

/// The header object, spliced so the `messages` array can be streamed into it.
///
/// Returns everything up to and including `"messages": [` and a newline.
/// Desktop's three header keys stay first and keep their order; a topic's own
/// metadata follows them.
///
/// The empty map is handled separately rather than left to the arithmetic. An
/// empty object serialises as `{}` — two bytes, no newline — so cutting a fixed
/// two off it leaves nothing at all, and the file opened on `,\n "messages": [`
/// with no `{` and a leading comma: not JSON, and only found by reading the
/// code, since every caller today passes at least `name`/`type`/`id`.
pub fn header_prelude(header: &Map<String, Value>) -> String {
    let inner = " ".repeat(INDENT);
    if header.is_empty() {
        return format!("{{\n{inner}\"messages\": [\n");
    }
    let head = to_string(&Value::Object(header.clone()));
    // Drop the trailing "\n}" and splice the streamed array on.
    let trimmed = head.strip_suffix("\n}").unwrap_or(&head);
    format!("{trimmed},\n{inner}\"messages\": [\n")
}

/// The closing bracket and brace for a streamed file.
///
/// **There is no trailing newline.** A real Desktop `result.json` ends on the
/// final `}` and nothing else — verified by `xxd` on the reference, which tails
/// `...\n }\n}` with no `0a` after it.
///
/// The Python exporter emitted `}\n` here and had done so since the first
/// commit. Its parity harness replayed `result.json` through the *HTML* writer
/// only, so the JSON emitter was covered by tests encoding what we believed,
/// and one byte per file went unnoticed. This is the first thing the JSON leg
/// caught, on its first run.
pub fn footer() -> String {
    format!("\n{}]\n}}", " ".repeat(INDENT))
}

/// Record of values the encoder could not map.
///
/// **An unmapped value must not be able to end an export.** Telegram adds
/// constructors faster than any exporter follows them, so a value nothing knows
/// how to serialise writes its text (or `(TypeName)`) and records the type name
/// here, to be reported at the end of the chat.
#[derive(Debug, Default, Clone)]
pub struct Degraded(pub std::collections::BTreeSet<String>);

impl Degraded {
    pub fn note(&mut self, type_name: &str) {
        self.0.insert(type_name.to_string());
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn names(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn indent_is_one_space() {
        let v = json!({ "a": 1, "b": { "c": 2 } });
        let s = to_string(&v);
        assert_eq!(s, "{\n \"a\": 1,\n \"b\": {\n  \"c\": 2\n }\n}");
    }

    #[test]
    fn non_ascii_is_raw_never_escaped() {
        // Desktop writes raw UTF-8. This is `ensure_ascii=False`.
        let v = json!({ "name": "bitno pročitaj ❤ 👍" });
        let s = to_string(&v);
        assert!(s.contains("bitno pročitaj ❤ 👍"), "got {s}");
        assert!(!s.contains("\\u"), "something got escaped: {s}");
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        // Python's json.dumps(indent=1) writes [] and {}, not [\n] — and the
        // reference is full of `"text_entities": []`.
        let s = to_string(&json!({ "text_entities": [], "o": {} }));
        assert!(s.contains("\"text_entities\": []"), "got {s}");
        assert!(s.contains("\"o\": {}"), "got {s}");
    }

    // ---- escaping parity with Python's json.dumps(ensure_ascii=False) ------
    // Each of these is a case where the two encoders could differ. Verified
    // against CPython 3.14: the expected strings below are what json.dumps
    // actually produces.

    #[test]
    fn quote_and_backslash_escape() {
        assert_eq!(to_string(&json!("a\"b\\c")), r#""a\"b\\c""#);
    }

    #[test]
    fn control_characters_use_the_short_forms_then_lowercase_u() {
        // Verified against CPython 3.14, json.dumps(ensure_ascii=False).
        // The five short forms, then lowercase-u escapes for the rest.
        // Hex digits are lowercase in both encoders; an uppercase escape
        // would diff on every message carrying a control character.
        assert_eq!(to_string(&json!("\u{8}\t\n\u{c}\r")), r#""\b\t\n\f\r""#);
        assert_eq!(to_string(&json!("\u{1}")), r#""\u0001""#);
        assert_eq!(to_string(&json!("\u{1f}")), r#""\u001f""#);
    }

    #[test]
    fn del_and_line_separators_are_not_escaped() {
        // Python leaves 0x7F, U+2028 and U+2029 raw under ensure_ascii=False.
        // serde_json must agree or the two files differ on any message
        // containing one.
        assert_eq!(to_string(&json!("\u{7f}")), "\"\u{7f}\"");
        assert_eq!(to_string(&json!("\u{2028}")), "\"\u{2028}\"");
        assert_eq!(to_string(&json!("\u{2029}")), "\"\u{2029}\"");
    }

    #[test]
    fn forward_slash_is_not_escaped() {
        // Media paths are full of these: "photos/photo_1@01-01-2025.jpg".
        assert_eq!(
            to_string(&json!("photos/photo_1.jpg")),
            "\"photos/photo_1.jpg\""
        );
    }

    #[test]
    fn reaction_block_is_indented_one_space_too_deep() {
        let lines: Vec<String> = vec![
            "  {".into(),
            "   \"id\": 1,".into(),
            "   \"reactions\": [".into(),
            "    {".into(),
            "     \"type\": \"emoji\"".into(),
            "    }".into(),
            "   ]".into(),
            "  }".into(),
        ];
        let out = desktop_reaction_indent(lines);
        // Everything between the opening bracket and the final brace moves.
        assert_eq!(out[2], "   \"reactions\": [");
        assert_eq!(out[3], "     {");
        assert_eq!(out[6], "    ]");
        assert_eq!(out[7], "  }"); // the object's own brace does not move
    }

    #[test]
    fn a_message_without_reactions_is_untouched() {
        let lines: Vec<String> = vec!["  {".into(), "   \"id\": 1".into(), "  }".into()];
        assert_eq!(desktop_reaction_indent(lines.clone()), lines);
    }

    #[test]
    fn a_key_sorting_after_reactions_keeps_its_own_indent() {
        // `ordered` puts an unranked key after `reactions`, so this shape is
        // what any key Telegram adds and nobody here has classified produces.
        // Running the over-indent to the object's closing brace instead of to
        // the array's own bracket moved those keys too — corrupting the
        // indentation of precisely the line a reader opens the diff for.
        let lines: Vec<String> = vec![
            "  {".into(),
            "   \"id\": 1,".into(),
            "   \"reactions\": [".into(),
            "    {".into(),
            "     \"type\": \"emoji\"".into(),
            "    }".into(),
            "   ],".into(),
            "   \"something_new_in_2027\": 1".into(),
            "  }".into(),
        ];
        let out = desktop_reaction_indent(lines);
        assert_eq!(out[3], "     {"); // the array still moves
        assert_eq!(out[6], "    ],"); // and so does its closing bracket
        assert_eq!(out[7], "   \"something_new_in_2027\": 1"); // this does not
        assert_eq!(out[8], "  }");
    }

    #[test]
    fn an_empty_reactions_array_moves_nothing() {
        // `[]` has no closing line of its own; there is nothing to shift and
        // nothing after it may be shifted either.
        let lines: Vec<String> = vec![
            "  {".into(),
            "   \"reactions\": [],".into(),
            "   \"something_new_in_2027\": 1".into(),
            "  }".into(),
        ];
        assert_eq!(desktop_reaction_indent(lines.clone()), lines);
    }

    #[test]
    fn message_block_matches_the_reference_shape() {
        let mut m = Map::new();
        m.insert("id".into(), json!(66));
        m.insert("type".into(), json!("service"));
        m.insert("text".into(), json!(""));
        m.insert("text_entities".into(), json!([]));
        let block = message_block(&m);
        // Two spaces of lead for the object, three for its keys — exactly what
        // `head -c 1200 result.json` shows in the reference.
        assert!(block.starts_with("  {\n   \"id\": 66,\n"), "got:\n{block}");
        assert!(block.ends_with("\n  }"), "got:\n{block}");
    }

    #[test]
    fn header_prelude_splices_the_array_on() {
        let mut h = Map::new();
        h.insert("name".into(), json!("bitno pročitaj"));
        h.insert("type".into(), json!("public_supergroup"));
        h.insert("id".into(), json!(3586682625i64));
        let out = header_prelude(&h);
        assert_eq!(
            out,
            "{\n \"name\": \"bitno pročitaj\",\n \"type\": \"public_supergroup\",\n \"id\": 3586682625,\n \"messages\": [\n"
        );
    }

    #[test]
    fn the_file_ends_on_the_brace_with_no_trailing_newline() {
        // Measured on the reference with xxd: the last bytes are
        //   20 5d 0a 7d      i.e. " ]\n}"  — and then EOF.
        // The Python exporter appended a newline here; Desktop does not.
        assert_eq!(footer(), "\n ]\n}");
        assert!(!footer().ends_with('\n'));
    }

    #[test]
    fn an_empty_header_still_opens_a_valid_object() {
        // Unreachable from the exporter, but the byte-slicing form emitted
        // `,\n "messages": [` — a file with no opening brace.
        let out = header_prelude(&Map::new());
        assert_eq!(out, "{\n \"messages\": [\n");
        let whole = format!("{out}{}", footer());
        let parsed: Value = serde_json::from_str(&whole).expect("valid JSON");
        assert_eq!(parsed["messages"], json!([]));
    }

    #[test]
    fn chat_id_is_the_bare_peer_number() {
        // Not the bot-API -100… form. The reference reads 3586682625.
        let mut h = Map::new();
        h.insert("id".into(), json!(3586682625i64));
        assert!(header_prelude(&h).contains("3586682625"));
        assert!(!header_prelude(&h).contains("-100"));
    }
}
