//! Userpics: the coloured circle with two letters in it.
//!
//! Two measured rules, both of which every other candidate mapping got wrong:
//!
//! * **The colour is the bare id modulo seven, through a permuted palette.**
//!   Exact on all 180 forwards in the reference carrying an id.
//! * **A hidden forward has no peer at all**, so Desktop colours it from the
//!   *message* id instead. Exact on all 154; every other candidate missed ~85%.

use crate::escape::esc;
use crate::tree::{a, Tree};
use serde_json::{Map, Value};
use tgx_format::peer::{initials_from_name, userpic_colour};

/// The presentation-only values Desktop shows in HTML but keeps out of JSON.
///
/// This is the `_p` dict. It travels beside the message rather than inside it
/// so both outputs still come from one object and cannot drift — and it is
/// deliberately **not** a general side-channel: everything in it is something
/// that provably cannot live in the JSON.
#[derive(Debug, Default, Clone)]
pub struct Presentation<'a> {
    map: Option<&'a Map<String, Value>>,
}

impl<'a> Presentation<'a> {
    pub fn of(m: &'a Map<String, Value>) -> Self {
        Self {
            map: m.get("_p").and_then(Value::as_object),
        }
    }

    pub fn str(&self, key: &str) -> Option<&'a str> {
        self.map?.get(key)?.as_str()
    }

    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.map?.get(key)
    }

    /// The letters Desktop painted for this peer, if the harness lifted them.
    pub fn initials(&self, peer: &str) -> Option<&'a str> {
        self.map?.get("initials")?.as_object()?.get(peer)?.as_str()
    }

    /// A self-chosen name colour, which Telegram numbers past the seven-entry
    /// default palette.
    pub fn colour(&self, peer: &str) -> Option<i64> {
        self.map?.get("colours")?.as_object()?.get(peer)?.as_i64()
    }
}

/// CSS index for a peer's userpic colour.
///
/// `override_colour` carries a colour the user chose for themselves, which
/// Telegram numbers beyond this seven-colour palette — Desktop writes those out
/// verbatim even though its own stylesheet defines only eight, so a peer with a
/// custom colour gets an unstyled userpic in Desktop's output too. Reproducing
/// that is the point.
pub fn userpic_class(from_id: &str, override_colour: Option<i64>) -> i64 {
    if let Some(c) = override_colour {
        return c + 1;
    }
    let bytes = from_id.as_bytes();
    let Some(first) = bytes.iter().position(u8::is_ascii_digit) else {
        // No digits at all: fall back to the first palette entry.
        return userpic_colour(0) as i64;
    };
    // Reduced digit by digit rather than parsed. `from_id` is text off the
    // wire, and `parse::<i64>()` fails on anything past nineteen digits — which
    // sent every over-long id to the *no-digits* branch, so a hostile id could
    // pick its own userpic colour by being long. The residue mod 7 is the whole
    // of what `userpic_colour` needs, and it cannot overflow.
    let mut rem: i64 = 0;
    for d in bytes[first..].iter().filter(|b| b.is_ascii_digit()) {
        rem = (rem * 10 + i64::from(d - b'0')) % 7;
    }
    // The sign was dropped by the digit filter, leaving `userpic_colour`'s
    // `rem_euclid` reachable only from its own test.
    if first > 0 && bytes[first - 1] == b'-' {
        rem = -rem;
    }
    userpic_colour(rem) as i64
}

/// The letters for a peer: lifted if the harness had them, derived otherwise.
pub fn letters(p: &Presentation, from_id: &str, name: &str) -> String {
    match p.initials(from_id) {
        Some(found) => found.to_string(),
        None => initials_from_name(name),
    }
}

/// The `<div class="userpic userpicN">` box, with its initials inside.
pub fn userpic_box(
    t: &mut Tree,
    p: &Presentation,
    name: &str,
    from_id: &str,
    size: u32,
    title: Option<&str>,
) {
    let index = userpic_class(from_id, p.colour(from_id));
    t.open(
        "div",
        &[
            a("class", format!("userpic userpic{index}")),
            a("style", format!("width: {size}px; height: {size}px")),
        ],
    );
    let mut attrs = vec![
        a("class", "initials".to_string()),
        a("style", format!("line-height: {size}px")),
    ];
    if let Some(tt) = title {
        attrs.push(a("title", tt.to_string()));
    }
    t.leaf("div", &esc(&letters(p, from_id, name)), &attrs);
    t.close("div");
}

/// The wrapped form used beside a message body.
pub fn userpic(
    t: &mut Tree,
    p: &Presentation,
    name: &str,
    from_id: &str,
    size: u32,
    extra_class: &str,
) {
    let wrap = if extra_class.is_empty() {
        "userpic_wrap".to_string()
    } else {
        format!("{extra_class} userpic_wrap")
    };
    t.open("div", &[a("class", format!("pull_left {wrap}"))]);
    userpic_box(t, p, name, from_id, size, None);
    t.close("div");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn the_colour_comes_from_the_digits_of_a_typed_key() {
        // from_id arrives as "user123", not as a bare integer.
        assert_eq!(userpic_class("user7", None), userpic_colour(7) as i64);
        assert_eq!(userpic_class("channel3", None), userpic_colour(3) as i64);
    }

    #[test]
    fn a_self_chosen_colour_overrides_the_palette() {
        // Telegram numbers these past the seven-entry default, and Desktop
        // writes them out verbatim — so a custom colour is unstyled there too.
        assert_eq!(userpic_class("user1", Some(11)), 12);
    }

    #[test]
    fn a_peer_with_no_digits_falls_back_rather_than_panicking() {
        assert_eq!(userpic_class("", None), userpic_colour(0) as i64);
        assert_eq!(userpic_class("anonymous", None), userpic_colour(0) as i64);
    }

    #[test]
    fn a_gigantic_id_gets_its_own_colour_not_the_fallback() {
        // `parse::<i64>()` returns Err past nineteen digits, and Err was the
        // no-digits branch — so every over-long id silently landed on palette
        // entry 0 rather than on its own colour. The old test asserted only
        // `(1..=8).contains(c)`, which the fallback satisfies, so it could not
        // see this. 10^30 ≡ 1 (mod 7), because 10^6 ≡ 1 and 6 divides 30.
        let huge = "user1".to_string() + &"0".repeat(30);
        assert_eq!(userpic_class(&huge, None), userpic_colour(1) as i64);
        assert_ne!(userpic_class(&huge, None), userpic_colour(0) as i64);
    }

    #[test]
    fn a_negative_id_keeps_its_sign() {
        // The digit filter dropped the minus, so `userpic_colour`'s deliberate
        // `rem_euclid` — and the test that pins it — was unreachable from the
        // only caller it has. No negative `from_id` appears in the reference,
        // so this is consistency with `userpic_colour`, not a measured case.
        assert_eq!(userpic_class("user-1", None), userpic_colour(-1) as i64);
        assert_eq!(userpic_class("user-8", None), userpic_colour(-8) as i64);
        assert_ne!(userpic_class("user-1", None), userpic_class("user1", None));
    }

    #[test]
    fn lifted_initials_beat_derived_ones() {
        let m = obj(json!({ "_p": { "initials": { "user1": "XY" } } }));
        let p = Presentation::of(&m);
        assert_eq!(letters(&p, "user1", "Some Name"), "XY");
        // A peer the harness did not see still derives from the name.
        assert_eq!(letters(&p, "user2", "Немања Фарм ГМ"), "НФ");
    }

    #[test]
    fn a_message_with_no_presentation_dict_is_harmless() {
        let m = obj(json!({ "id": 1 }));
        let p = Presentation::of(&m);
        assert_eq!(p.colour("user1"), None);
        assert_eq!(p.initials("user1"), None);
        assert_eq!(letters(&p, "user1", "A B"), "AB");
    }

    #[test]
    fn the_userpic_box_has_desktops_attribute_shape() {
        let m = obj(json!({}));
        let p = Presentation::of(&m);
        let mut t = Tree::new();
        userpic_box(&mut t, &p, "Ivana ETF", "user7", 42, None);
        let out = t.as_str();
        assert!(out.contains("style=\"width: 42px; height: 42px\""), "{out}");
        assert!(out.contains("style=\"line-height: 42px\""), "{out}");
        assert!(out.contains("class=\"initials\""), "{out}");
    }

    #[test]
    fn a_reaction_userpic_carries_a_title() {
        let m = obj(json!({}));
        let p = Presentation::of(&m);
        let mut t = Tree::new();
        userpic_box(&mut t, &p, "mila mfub", "user1", 20, Some("mila mfub"));
        assert!(t.as_str().contains("title=\"mila mfub\""));
    }

    #[test]
    fn a_forwarded_userpic_gets_its_extra_class() {
        let m = obj(json!({}));
        let p = Presentation::of(&m);
        let mut t = Tree::new();
        userpic(&mut t, &p, "X", "user1", 42, "forwarded");
        assert!(t
            .as_str()
            .contains("class=\"pull_left forwarded userpic_wrap\""));
    }

    #[test]
    fn initials_are_escaped() {
        let m = obj(json!({ "_p": { "initials": { "user1": "<>" } } }));
        let p = Presentation::of(&m);
        let mut t = Tree::new();
        userpic_box(&mut t, &p, "x", "user1", 42, None);
        assert!(t.as_str().contains("&lt;&gt;"));
        assert!(!t.as_str().contains("<>"));
    }
}
