//! Entity offsets, which Telegram counts in **UTF-16 code units**.
//!
//! Indexing the text by anything else corrupts formatting in every message
//! containing an emoji. `str::encode_utf16` gives the units directly, and —
//! the part that matters — `String::from_utf16` *returns an error* rather than
//! handing back a string that cannot later be encoded. Producing one silently
//! is what kills a real export thousands of messages after the offending
//! message went past.

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
/// Dropping lone surrogates and joining the units, in one pass. A lone
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
/// 0 in the low set, and without the clamp cut 0 would go below zero — an
/// underflow here, and in any language that reads a negative index as a slice
/// from the end, a first segment that comes out empty having taken its
/// characters with it.
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

    // Desktop's empty tail segment.
    //
    // When the last entity runs to the end of the message Desktop appends one
    // more part — the empty string — but *only* when the text is not pure
    // ASCII. Measured over the whole reference export: 98 messages end on an
    // entity, and the 11 that carry the empty tail are exactly the 11 whose
    // text contains a non-ASCII character. 98 of 98, no exceptions. The entity
    // type plays no part: `mention` and `link` occur on both sides of the
    // split, so "always append when the text ends on an entity" would have been
    // right 11 times and wrong 87.
    //
    // That split is the signature of comparing a UTF-16 offset against a UTF-8
    // byte count. Desktop asks "is there anything after the last entity?" with
    // the entity's end *offset* (code units) and the text's *byte* length, and
    // above U+007F the byte count is the larger of the two — so it slices a
    // tail that is already empty and writes it out. Reference message 2084,
    // `"Treba mi nešto malo starije fazon " + phone`, ends at unit 43 of a
    // 44-byte string and gets the tail; message 4111, `".lens\n" + mention`,
    // ends at unit 20 of a 20-byte string and does not.
    //
    // Neither replay leg can catch this. Both start from Desktop's own
    // `result.json`, so the segment is already in their input and comes back
    // out untouched — which is exactly how it survived until a live export was
    // held up against the reference. The tests below are the only guard.
    if out.last().is_some_and(|last| last["type"] != "plain") && text.len() > n {
        out.push(json!({ "type": "plain", "text": "" }));
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
    fn a_trailing_entity_gets_desktops_empty_tail_when_the_text_is_not_ascii() {
        // Reference message 2084 in `ćaskanje`. Desktop wrote
        //   ["Treba mi nešto malo starije fazon ", {phone "2021-2023"}, ""]
        // and we used to stop at the phone. 43 UTF-16 units, 44 UTF-8 bytes.
        let text = "Treba mi nešto malo starije fazon 2021-2023";
        let segs = build_text_entities(text, &[ent(34, 9, "phone")]);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[1]["type"], "phone");
        assert_eq!(segs[1]["text"], "2021-2023");
        // `text_entities` carries it as a plain segment...
        assert_eq!(segs[2], json!({ "type": "plain", "text": "" }));
        // ...and `text` as a bare empty string.
        let field = build_text_field(&segs);
        assert_eq!(
            field,
            json!(["Treba mi nešto malo starije fazon ", { "type": "phone", "text": "2021-2023" }, ""])
        );
    }

    #[test]
    fn a_trailing_entity_in_pure_ascii_text_gets_no_tail() {
        // The other 87 of the 98. Reference message 4111: 20 units, 20 bytes,
        // so Desktop's "is anything left?" test comes out false.
        let text = ".lens\n@flicsbysoleee";
        let segs = build_text_entities(text, &[ent(6, 14, "mention")]);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1]["type"], "mention");
        assert_eq!(
            build_text_field(&segs),
            json!([".lens\n", { "type": "mention", "text": "@flicsbysoleee" }])
        );
    }

    #[test]
    fn a_non_ascii_message_ending_in_plain_text_gets_no_tail() {
        // The tail is about the *last entity* reaching the end, not about the
        // text being non-ASCII: here the entity is in the middle and Desktop's
        // ordinary tail part is non-empty already.
        let segs = build_text_entities("šta @matfic hm", &[ent(4, 7, "mention")]);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[2], json!({ "type": "plain", "text": " hm" }));
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
