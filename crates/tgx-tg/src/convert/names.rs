//! Who a peer is: the name book, the variants only the HTML needs, and the
//! typed key everything else is filed under.
//!
//! A sender is a typed peer key, never a display name -- two people can share
//! a name and one person can change theirs.

use super::*;

/// Display names by typed peer key, plus the variants only the HTML needs.
#[derive(Debug, Default, Clone)]
pub struct NameBook {
    /// The trimmed display name, which is what `result.json` carries.
    pub names: HashMap<String, String>,
    /// Desktop's HTML name: `first + " " + last`, **untrimmed**.
    pub html: HashMap<String, String>,
    /// The userpic letters, from the name *fields*.
    pub initials: HashMap<String, String>,
    /// A self-chosen name colour, numbered past the seven-entry palette.
    pub colour: HashMap<String, i64>,
    /// Write a contact as `@handle` rather than under the name you saved.
    ///
    /// **Set here rather than checked at each call site**, because there are
    /// four of them — `result.json`'s name, the HTML's untrimmed one, the
    /// userpic letters and the roster — and a switch honoured by three of four
    /// is worse than one honoured by none: the export would then disagree with
    /// itself about who somebody is.
    pub own_names: bool,
    /// Contacts written as their own handle, and contacts that had no handle
    /// to be written as.
    ///
    /// Counted because this option is otherwise invisible: on an account with
    /// no contacts in the chat it legitimately changes nothing, and that looks
    /// exactly like a switch that does nothing.
    pub aliased: (usize, usize),
}

impl NameBook {
    /// Learn a user from [`UserFacts`].
    ///
    /// grammers regenerates `tl::types::User` from Telegram's schema and it has
    /// gained fields three times in recent releases; taking the parts keeps
    /// this testable without a forty-field fixture that rots on every bump.
    ///
    /// **The `own_names` substitution lives here**, not in [`Self::learn_user`].
    /// This is the entry point every test uses, so a switch honoured only on
    /// the TL path would be a switch no test could reach.
    pub fn learn(&mut self, f: UserFacts<'_>) {
        let (first, last) = own_name_parts(
            self.own_names,
            f.contact,
            f.username,
            f.first,
            f.last,
            &mut self.aliased,
        );
        let key = PeerKey::user(f.id).to_string();
        if let Some(n) = tgx_format::peer::display_name(first, last, f.username, f.deleted) {
            self.names.insert(key.clone(), n);
        }
        if let Some(n) = tgx_format::peer::html_name(first, last, f.username, f.deleted) {
            self.html.insert(key.clone(), n);
        }
        let letters = if f.deleted {
            "D".to_string()
        } else if first.is_empty() && last.is_empty() {
            f.username
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default()
        } else {
            tgx_format::peer::initials_from_fields(first, last)
        };
        self.initials.insert(key.clone(), letters);
        if let Some(v) = f.colour {
            self.colour.insert(key, v);
        }
    }

    pub fn learn_user(&mut self, u: &tl::types::User) {
        // `color` is an enum: a plain palette index, or one of the collectible
        // forms which carry no index at all. Only the plain one is a colour
        // Desktop would write.
        let colour = match &u.color {
            Some(tl::enums::PeerColor::Color(c)) => c.color.map(|v| v as i64),
            _ => None,
        };
        self.learn(UserFacts {
            id: u.id,
            first: u.first_name.as_deref().unwrap_or(""),
            last: u.last_name.as_deref().unwrap_or(""),
            username: u.username.as_deref().unwrap_or(""),
            deleted: u.deleted,
            contact: u.contact,
            colour,
        });
    }

    pub fn learn_chat_title(&mut self, key: PeerKey, title: &str) {
        let k = key.to_string();
        self.names.insert(k.clone(), title.to_string());
        self.html.insert(k.clone(), title.to_string());
        if let Some(i) = tgx_format::peer::initials_from_title(title) {
            self.initials.insert(k, i);
        }
    }

    pub fn get(&self, key: &str) -> &str {
        self.names.get(key).map(String::as_str).unwrap_or("")
    }

    pub fn html_name(&self, key: &str) -> &str {
        self.html
            .get(key)
            .or_else(|| self.names.get(key))
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// The handful of `tl::types::User` fields an export actually needs.
///
/// Named rather than positional because the flags are the point: a call site
/// reading `false, false, None` says nothing about which of *deleted*,
/// *contact* and *colour* is which, and the two were one argument away from
/// silently swapping.
#[derive(Debug, Default, Clone, Copy)]
pub struct UserFacts<'a> {
    pub id: i64,
    pub first: &'a str,
    pub last: &'a str,
    pub username: &'a str,
    pub deleted: bool,
    /// In *your* address book — which is why the name above may be the one you
    /// gave them rather than the one they chose. See `Settings::own_names`.
    pub contact: bool,
    pub colour: Option<i64>,
}

/// The name parts to build a user's display name from, honouring `own_names`.
///
/// Returns the pair to feed to `display_name`, which already falls back to
/// `@username` when both are empty — so blanking them here *is* the
/// substitution, and it goes through the same tested path as a user who
/// genuinely set no name.
///
/// **Only a contact is touched.** For everyone else Telegram already sends the
/// name they chose; rewriting those to handles would lose real names to
/// implement an option about false ones.
///
/// `tally` counts `(replaced, kept)` — the second being a contact with no
/// username, where their own name is unobtainable and yours is better than
/// nothing.
pub(crate) fn own_name_parts<'a>(
    own_names: bool,
    contact: bool,
    username: &str,
    first: &'a str,
    last: &'a str,
    tally: &mut (usize, usize),
) -> (&'a str, &'a str) {
    if !own_names || !contact {
        return (first, last);
    }
    if username.is_empty() {
        tally.1 += 1;
        return (first, last);
    }
    tally.0 += 1;
    ("", "")
}

/// Desktop writes peers as `user123` / `chat123` / `channel123`.
pub fn peer_key(peer: &tl::enums::Peer) -> PeerKey {
    match peer {
        tl::enums::Peer::User(p) => PeerKey::user(p.user_id),
        tl::enums::Peer::Chat(p) => PeerKey::chat(p.chat_id),
        tl::enums::Peer::Channel(p) => PeerKey::channel(p.channel_id),
    }
}
