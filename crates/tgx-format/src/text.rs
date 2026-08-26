//! Entity offsets, which Telegram counts in **UTF-16 code units**.
//!
//! Indexing the text by anything else corrupts formatting in every message
//! containing an emoji. Python had to build an explicit list of code units to
//! get this right; Rust has `str::encode_utf16`, and — the part that matters —
//! `String::from_utf16` *returns an error* where Python silently produced a
//! string it could not later encode, which is what killed one real export
//! thousands of messages after the offending message went past.

use serde_json::{json, Map, Value};

/// One entity as it arrives off the wire, already mapped to Desktop's name for
/// it. `extras` carries the per-type tail (`href`, `user_id`, `language`,
/// `document_id`).
#[derive(Debug, Clone)]
pub struct Entity {
    pub offset: i64,
    pub length: i64,
    pub kind: &'static str,
    pub extras: Vec<(&'static str, Value)>,
}

/// The UTF-16 code units of `text`.
pub fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Decode a slice of code units, replacing any unpaired surrogate with U+FFFD.
///
/// This is Python's `_drop_lone_surrogates` and `_join_units` in one. A lone
/// surrogate cannot be encoded as UTF-8, so one reaching a writer takes down
/// the export of the whole chat; `snap_cuts` stops entity boundaries from
/// creating them and this is the backstop for anything that arrives already
/// broken.
pub fn decode_lossy(slice: &[u16]) -> String {
    String::from_utf16_lossy(slice)
}

/// True when this code unit is a low (trailing) surrogate.
fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

/// Move any cut that falls between the halves of a surrogate pair back to the
/// start of the pair.
///
/// Official clients never split a pair; a hostile one can, and a segment
/// holding half a pair is unwritable. Snapping costs at most one code point of
/// formatting fidelity.
///
/// Clamped at zero. A text starting with an *unpaired* low surrogate puts index
/// 0 in the low set, and the unclamped form turned cut 0 into -1 — which Python
/// read as a slice from the end, so the first segment came out empty and took
/// its characters with it.
pub fn snap_cuts(cuts: impl IntoIterator<Item = usize>, units: &[u16]) -> Vec<usize> {
    let mut out: Vec<usize> = cuts
        .into_iter()
        .map(|c| {
            if units.get(c).copied().is_some_and(is_low_surrogate) {
                c.saturating_sub(1)
            } else {
                c
            }
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One entity's footprint on the string: `(start, end, kind, extras)`.
type Span<'a> = (usize, usize, &'static str, &'a [(&'static str, Value)]);

/// Flat, gap-free segmentation of `text` — Desktop's `text_entities`.
///
/// Telegram allows nested entities but Desktop's JSON is flat, so the string is
/// cut at every boundary and the innermost (shortest) covering entity wins for
/// each resulting segment. Touching plain runs are merged so the output matches
/// Desktop's shape.
pub fn build_text_entities(text: &str, entities: &[Entity]) -> Vec<Value> {
    if text.is_empty() {
        return Vec::new();
    }
    let plain = || vec![json!({ "type": "plain", "text": text })];
    if entities.is_empty() {
        return plain();
    }

    let units = units(text);
    let n = units.len();

    let mut spans: Vec<Span<'_>> = Vec::new();
    for ent in entities {
        let start = ent.offset.max(0) as usize;
        let end = (ent.offset.saturating_add(ent.length)).max(0) as usize;
        let end = end.min(n);
        if start < end {
            spans.push((start, end, ent.kind, ent.extras.as_slice()));
        }
    }
    if spans.is_empty() {
        return plain();
    }

    let mut raw: Vec<usize> = vec![0, n];
    for (s, e, _, _) in &spans {
        raw.push(*s);
        raw.push(*e);
    }
    raw.sort_unstable();
    raw.dedup();
    let cuts = snap_cuts(raw, &units);

    let mut out: Vec<Value> = Vec::new();
    for w in cuts.windows(2) {
        let (lo, hi) = (w[0], w[1]);
        if lo >= hi {
            continue;
        }
        let chunk = decode_lossy(&units[lo..hi]);
        if chunk.is_empty() {
            continue;
        }
        // Innermost covering entity wins.
        let winner = spans
            .iter()
            .filter(|(s, e, _, _)| *s <= lo && *e >= hi)
            .min_by_key(|(s, e, _, _)| e - s);

        let seg = match winner {
            Some((_, _, kind, extras)) => {
                let mut m = Map::new();
                m.insert("type".into(), Value::String((*kind).into()));
                m.insert("text".into(), Value::String(chunk));
                for (k, v) in extras.iter() {
                    m.insert((*k).into(), v.clone());
                }
                Value::Object(m)
            }
            None => json!({ "type": "plain", "text": chunk }),
        };

        // Merge touching plain runs.
        let seg_is_plain = seg["type"] == "plain";
        if seg_is_plain {
            if let Some(last) = out.last_mut() {
                if last["type"] == "plain" {
                    let extra = seg["text"].as_str().unwrap_or("").to_string();
                    if let Some(Value::String(prev)) = last.get_mut("text") {
                        prev.push_str(&extra);
                    }
                    continue;
                }
            }
        }
        out.push(seg);
    }
    out
}

/// Desktop's `text`: a bare string when nothing is formatted, otherwise a list
/// of strings and objects.
pub fn build_text_field(segments: &[Value]) -> Value {
    if segments.is_empty() {
        return Value::String(String::new());
    }
    if segments.iter().all(|s| s["type"] == "plain") {
        let joined: String = segments.iter().filter_map(|s| s["text"].as_str()).collect();
        return Value::String(joined);
    }
    Value::Array(
        segments
            .iter()
            .map(|s| {
                if s["type"] == "plain" {
                    s["text"].clone()
                } else {
                    s.clone()
                }
            })
            .collect(),
    )
}

/// Unwrap a `TextWithEntities` chain until a string appears.
///
/// Telegram is migrating plain `str` fields to `TextWithEntities`, and some of
/// them nest: `MessageActionPollAppendAnswer.answer` is a whole `PollAnswer`
/// whose `.text` is *itself* a `TextWithEntities`, so one hop lands on another
/// object rather than a string. That object reached the JSON encoder and killed
/// the first real export at message 5,609 of 6,600. Every one of these fields
/// goes through here and nowhere else.
pub fn plain_text(value: &Value) -> String {
    let mut cur = value;
    for _ in 0..8 {
        match cur {
            Value::String(s) => return s.clone(),
            Value::Object(map) => match map.get("text") {
                Some(next) => cur = next,
                None => return String::new(),
            },
            Value::Null => return String::new(),
            other => return other.to_string(),
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(offset: i64, length: i64, kind: &'static str) -> Entity {
        Entity {
            offset,
            length,
            kind,
            extras: vec![],
        }
    }

    #[test]
    fn offsets_are_utf16_not_chars() {
        // "👍 bold" — the emoji is ONE char in Rust but TWO UTF-16 units.
        // Telegram's offset for "bold" is 3, counting in UTF-16.
        let text = "👍 bold";
        let segs = build_text_entities(text, &[ent(3, 4, "bold")]);
        let bold: Vec<&Value> = segs.iter().filter(|s| s["type"] == "bold").collect();
        assert_eq!(bold.len(), 1);
        assert_eq!(bold[0]["text"], "bold");

        // And prove the naive reading really would be wrong: byte-slicing at 3
        // lands mid-emoji, char-slicing at 3 yields "old".
        let by_chars: String = text.chars().skip(3).take(4).collect();
        assert_ne!(by_chars, "bold");
    }

    #[test]
    fn a_cut_between_surrogate_halves_snaps_back() {
        let text = "a👍b";
        let units = units(text);
        // 'a'=1 unit, emoji=2 units at index 1..3, 'b' at 3.
        assert_eq!(units.len(), 4);
        // A cut at 2 lands between the halves.
        let snapped = snap_cuts(vec![0, 2, 4], &units);
        assert_eq!(snapped, vec![0, 1, 4]);
        // Every resulting segment must be encodable.
        for w in snapped.windows(2) {
            let s = decode_lossy(&units[w[0]..w[1]]);
            assert!(!s.contains('\u{FFFD}'), "segment {s:?} lost a character");
        }
    }

    #[test]
    fn a_hostile_offset_cannot_produce_an_unwritable_segment() {
        let text = "a👍b";
        // Deliberately split the pair.
        let segs = build_text_entities(text, &[ent(2, 1, "bold")]);
        let joined: String = segs.iter().filter_map(|s| s["text"].as_str()).collect();
        // Nothing is lost and nothing is unencodable.
        assert!(joined.contains('a') && joined.contains('b'));
        for s in &segs {
            assert!(std::str::from_utf8(s["text"].as_str().unwrap().as_bytes()).is_ok());
        }
    }

    #[test]
    fn plain_runs_merge() {
        let segs = build_text_entities("abcdef", &[ent(2, 2, "bold")]);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0]["text"], "ab");
        assert_eq!(segs[1]["type"], "bold");
        assert_eq!(segs[2]["text"], "ef");
    }

    #[test]
    fn innermost_entity_wins() {
        // A bold run with an italic inside it: the overlap is italic, not bold.
        let segs = build_text_entities("abcdef", &[ent(0, 6, "bold"), ent(2, 2, "italic")]);
        let kinds: Vec<&str> = segs.iter().map(|s| s["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["bold", "italic", "bold"]);
    }

    #[test]
    fn text_field_is_a_bare_string_when_unformatted() {
        let segs = build_text_entities("hello", &[]);
        assert_eq!(build_text_field(&segs), Value::String("hello".into()));
    }

    #[test]
    fn text_field_is_a_list_when_formatted() {
        let segs = build_text_entities("abcdef", &[ent(2, 2, "bold")]);
        let field = build_text_field(&segs);
        assert!(field.is_array());
        assert_eq!(field[0], "ab");
        assert_eq!(field[1]["type"], "bold");
    }

    #[test]
    fn plain_text_unwraps_until_a_string_appears() {
        // The shape that killed the first real export: two levels of wrapping.
        let nested = json!({ "text": { "text": "the answer" } });
        assert_eq!(plain_text(&nested), "the answer");
        assert_eq!(plain_text(&json!("flat")), "flat");
        assert_eq!(plain_text(&Value::Null), "");
    }

    #[test]
    fn empty_text_has_no_entities() {
        assert!(build_text_entities("", &[ent(0, 4, "bold")]).is_empty());
    }

    #[test]
    fn out_of_range_entities_are_clamped_away() {
        let segs = build_text_entities("hi", &[ent(50, 4, "bold")]);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0]["type"], "plain");
    }
}
