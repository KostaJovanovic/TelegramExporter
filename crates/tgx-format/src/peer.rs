//! Typed peer keys, userpic colours and initials.
//!
//! **Name cache keys are typed.** Telegram numbers users, chats and channels in
//! separate id spaces, so the same integer can legitimately be a user *and* a
//! channel. Keying a cache by the bare number conflates them and attributes a
//! message to the wrong entity. Nothing here ever takes a bare `i64` as an
//! identity.

use std::fmt;

/// Which id space a peer number lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PeerKind {
    User,
    Chat,
    Channel,
}

impl PeerKind {
    fn prefix(self) -> &'static str {
        match self {
            PeerKind::User => "user",
            PeerKind::Chat => "chat",
            PeerKind::Channel => "channel",
        }
    }
}

/// A peer, as Desktop writes it: `user123` / `chat123` / `channel123`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerKey {
    pub kind: PeerKind,
    pub id: i64,
}

impl PeerKey {
    pub fn user(id: i64) -> Self {
        Self {
            kind: PeerKind::User,
            id,
        }
    }
    pub fn chat(id: i64) -> Self {
        Self {
            kind: PeerKind::Chat,
            id,
        }
    }
    pub fn channel(id: i64) -> Self {
        Self {
            kind: PeerKind::Channel,
            id,
        }
    }

    /// Parse the `user123` form back. Used by the parity harness, which reads
    /// `from_id` out of a reference `result.json` as a string.
    pub fn parse(s: &str) -> Option<Self> {
        for (prefix, kind) in [
            ("channel", PeerKind::Channel),
            ("chat", PeerKind::Chat),
            ("user", PeerKind::User),
        ] {
            if let Some(rest) = s.strip_prefix(prefix) {
                if let Ok(id) = rest.parse::<i64>() {
                    return Some(Self { kind, id });
                }
            }
        }
        None
    }
}

impl fmt::Display for PeerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.kind.prefix(), self.id)
    }
}

/// Desktop's userpic palette, indexed by `bare_id % 7`.
///
/// Recovered from a reference export and exact on all 180 forwards carrying an
/// id. Every other candidate mapping missed roughly 85%.
const COLOUR_MAP: [u8; 7] = [1, 8, 5, 2, 7, 4, 6];

/// The `userpicN` class Desktop paints for a peer.
///
/// A **hidden** forward has no peer at all, so Desktop colours it from the
/// *message* id instead — exact on all 154 in the reference. Callers pass
/// whichever number applies; this function does not know the difference.
pub fn userpic_colour(bare_id: i64) -> u8 {
    COLOUR_MAP[(bare_id.rem_euclid(7)) as usize]
}

/// The two letters Desktop paints in an empty userpic, from a user's name
/// *fields*.
///
/// Taken from `first_name` and `last_name`, never from splitting the display
/// string. "Nađa Gavrilović arh blokade fotograf" renders as `Nf`, because the
/// surname really is the single word "fotograf"; splitting the joined string on
/// its first space gives `NG`, which appears nowhere in the reference's 281
/// userpics for her. "Relja Jarkovački pravni" renders as `R` alone because all
/// three words are the first name.
///
/// The stylesheet upper-cases them, so case is kept as-is.
pub fn initials_from_fields(first: &str, last: &str) -> String {
    let first = first.trim();
    let last = last.trim();
    let mut out = String::new();
    if let Some(c) = first.chars().next() {
        out.push(c);
    }
    if let Some(c) = last.chars().next() {
        out.push(c);
    }
    out
}

/// Initials for a peer known only by a display string — a hidden forward.
///
/// Split on the **first** space, not the last: "Немања Фарм ГМ" is `НФ`, not
/// `НГ`.
pub fn initials_from_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return String::new();
    }
    match name.split_once(' ') {
        Some((first, rest)) => {
            let mut out = String::new();
            if let Some(c) = first.chars().next() {
                out.push(c);
            }
            if let Some(c) = rest.trim_start().chars().next() {
                out.push(c);
            }
            out
        }
        None => name.chars().next().map(String::from).unwrap_or_default(),
    }
}

/// Initials for a chat or channel, which have a `title` rather than name fields.
///
/// One word gives one letter; more than one gives first-and-last.
pub fn initials_from_title(title: &str) -> Option<String> {
    let words: Vec<&str> = title.split_whitespace().collect();
    match words.len() {
        0 => None,
        1 => words[0].chars().next().map(String::from),
        _ => {
            let mut out = String::new();
            if let Some(c) = words[0].chars().next() {
                out.push(c);
            }
            if let Some(c) = words[words.len() - 1].chars().next() {
                out.push(c);
            }
            Some(out)
        }
    }
}

/// The trimmed display name, which is what `result.json` carries.
pub fn display_name(first: &str, last: &str, username: &str, deleted: bool) -> Option<String> {
    if deleted {
        return Some("Deleted Account".into());
    }
    let parts: Vec<&str> = [first, last]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    if !parts.is_empty() {
        return Some(parts.join(" "));
    }
    if !username.is_empty() {
        return Some(format!("@{username}"));
    }
    None
}

/// Desktop's *HTML* name: `first + " " + last`, **not** trimmed.
///
/// Measured against a reference export: 94 of 601 `from_name` divs on one page
/// end in a space, and every one of them belongs to a user with no surname.
/// `result.json` carries the trimmed form for the same people, so the two really
/// are different strings and both have to be kept.
pub fn html_name(first: &str, last: &str, username: &str, deleted: bool) -> Option<String> {
    if deleted {
        return Some("Deleted Account".into());
    }
    if !first.is_empty() {
        return Some(format!("{first} {last}")); // the space stays when last is empty
    }
    if !last.is_empty() {
        return Some(last.to_string());
    }
    if !username.is_empty() {
        return Some(format!("@{username}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_id_spaces_do_not_collide() {
        // The bug this prevents: the same integer is a valid user id AND a
        // valid channel id, and a bare-int cache attributed one to the other.
        assert_ne!(
            PeerKey::user(123).to_string(),
            PeerKey::channel(123).to_string()
        );
        assert_eq!(PeerKey::user(123).to_string(), "user123");
        assert_eq!(PeerKey::chat(123).to_string(), "chat123");
        assert_eq!(PeerKey::channel(123).to_string(), "channel123");
    }

    #[test]
    fn peer_keys_round_trip() {
        for k in [
            PeerKey::user(1),
            PeerKey::chat(22),
            PeerKey::channel(3586682625),
        ] {
            assert_eq!(PeerKey::parse(&k.to_string()), Some(k));
        }
        assert_eq!(PeerKey::parse("nonsense"), None);
    }

    #[test]
    fn channel_parses_before_chat() {
        // "channel5" must not be read as chat-with-a-weird-suffix.
        assert_eq!(PeerKey::parse("channel5"), Some(PeerKey::channel(5)));
    }

    #[test]
    fn userpic_colour_is_desktops_permuted_palette() {
        // Not id % 7 directly — the palette is permuted, which is the thing
        // every other candidate mapping got wrong.
        assert_eq!(userpic_colour(0), 1);
        assert_eq!(userpic_colour(1), 8);
        assert_eq!(userpic_colour(2), 5);
        assert_eq!(userpic_colour(3), 2);
        assert_eq!(userpic_colour(4), 7);
        assert_eq!(userpic_colour(5), 4);
        assert_eq!(userpic_colour(6), 6);
        assert_eq!(userpic_colour(7), 1);
    }

    #[test]
    fn userpic_colour_never_panics_on_a_negative_id() {
        // rem_euclid, not %, or a negative id indexes out of bounds.
        for id in [-1i64, -7, -8, i64::MIN + 1] {
            let c = userpic_colour(id);
            assert!(COLOUR_MAP.contains(&c));
        }
    }

    #[test]
    fn initials_come_from_the_fields_not_the_display_string() {
        // The measured case: the surname really is the single word "fotograf",
        // so the fields are "Nađa Gavrilović arh blokade" / "fotograf". This
        // test used to hand the whole tail in as the surname and assert "NG";
        // Desktop paints `Nf`, 281 times in the reference's HTML, and `NG`
        // appears nowhere in it. That the display string is not what is split
        // is settled by the hidden-forward case below, where Desktop takes the
        // *first* two words of a name it has only as one string.
        assert_eq!(
            initials_from_fields("Nađa Gavrilović arh blokade", "fotograf"),
            "Nf"
        );
        // All three words are the first name, so there is only one letter.
        assert_eq!(initials_from_fields("Relja Jarkovački pravni", ""), "R");
    }

    #[test]
    fn a_hidden_forward_splits_on_the_first_space() {
        // "Немања Фарм ГМ" is НФ, not НГ — the last word is not the surname.
        assert_eq!(initials_from_name("Немања Фарм ГМ"), "НФ");
        assert_eq!(initials_from_name("Solo"), "S");
        assert_eq!(initials_from_name(""), "");
    }

    #[test]
    fn a_title_splits_first_and_last() {
        assert_eq!(
            initials_from_title("UA KOLAB TELEGRAM").as_deref(),
            Some("UT")
        );
        assert_eq!(initials_from_title("Redakcija").as_deref(), Some("R"));
        assert_eq!(initials_from_title("   "), None);
    }

    #[test]
    fn html_name_keeps_the_trailing_space_json_trims() {
        // 94 of 601 names on one reference page end in a space. These two
        // functions must disagree, and that disagreement is the point.
        assert_eq!(
            html_name("Ivana", "ETF", "", false).as_deref(),
            Some("Ivana ETF")
        );
        assert_eq!(html_name("Ivana", "", "", false).as_deref(), Some("Ivana "));
        assert_eq!(
            display_name("Ivana", "", "", false).as_deref(),
            Some("Ivana")
        );
    }

    #[test]
    fn a_deleted_account_is_named_not_blank() {
        assert_eq!(
            display_name("", "", "", true).as_deref(),
            Some("Deleted Account")
        );
        assert_eq!(
            html_name("", "", "", true).as_deref(),
            Some("Deleted Account")
        );
    }

    #[test]
    fn a_user_with_only_a_username_is_at_prefixed() {
        assert_eq!(
            display_name("", "", "kosta", false).as_deref(),
            Some("@kosta")
        );
    }
}
