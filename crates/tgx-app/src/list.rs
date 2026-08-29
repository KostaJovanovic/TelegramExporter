//! What the chat list paints: the sort, the five buckets, folding, the filter.
//!
//! Pure logic, deliberately. Nothing here touches the UI framework, so the
//! rules below are testable without opening a window — and every one of them
//! is a rule that would otherwise only be checkable by looking at the screen.
//!
//! The window calls [`rows`] once per frame to get what to paint, and
//! [`visible`] to find what All / None / Invert / Only forums act on.

use chrono::{DateTime, Datelike, Local, TimeZone};
use std::cmp::Ordering;
use std::collections::HashSet;
use tgx_tg::client::{ChatInfo, ChatKind};

/// A chat's last activity, as Telegram itself words it.
///
/// **A relative timestamp is what makes the default sort legible.** "Recent
/// activity" orders on a unix second nobody can see; without a date beside the
/// title the list is in an order the user has to take on trust. This is the
/// original's `human_when`, ported with its thresholds intact — today is a
/// clock time, yesterday is a word, this week is a weekday, this year drops the
/// year.
///
/// `now` is a parameter rather than read inside, because a function that reads
/// the clock can only be tested by waiting.
pub fn human_when(timestamp: i64, now: DateTime<Local>) -> String {
    if timestamp <= 0 {
        return String::new();
    }
    let Some(when) = Local.timestamp_opt(timestamp, 0).single() else {
        return String::new();
    };
    // Whole days between calendar dates, not between instants: a message from
    // 23:50 last night is "Yesterday" at 00:10, not "today, 20 minutes ago".
    let days = (now.date_naive() - when.date_naive()).num_days();
    match days {
        0 => when.format("%H:%M").to_string(),
        1 => "Yesterday".into(),
        2..=6 => when.format("%A").to_string(),
        _ if when.year() == now.year() => when.format("%d %b").to_string(),
        _ => when.format("%d %b %Y").to_string(),
    }
}

/// The caption under a chat's title: what it is, and when it last moved.
///
/// Joined here rather than in the painting so the separator and the empty-date
/// case have one home — a chat Telegram gave no last message reads as its kind
/// alone, not as its kind followed by a dangling separator.
pub fn caption(chat: &ChatInfo, now: DateTime<Local>) -> String {
    match human_when(chat.last_activity, now) {
        when if when.is_empty() => chat.kind.label().to_string(),
        when => format!("{}  {when}", chat.kind.label()),
    }
}

/// The seven sort modes, in the order the menu lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortMode {
    #[default]
    Recent,
    Oldest,
    Name,
    NameDesc,
    Largest,
    Smallest,
    Kind,
}

impl SortMode {
    /// The name this mode is stored under in `settings.json`.
    ///
    /// These strings are the on-disk settings format. Renaming one does not
    /// fail loudly: it silently resets every existing user to the default
    /// sort, because the key their file holds no longer matches anything.
    pub fn key(self) -> &'static str {
        match self {
            SortMode::Recent => "recent",
            SortMode::Oldest => "oldest",
            SortMode::Name => "name",
            SortMode::NameDesc => "name_desc",
            SortMode::Largest => "largest",
            SortMode::Smallest => "smallest",
            SortMode::Kind => "type",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Recent => "Recent activity",
            SortMode::Oldest => "Least recent",
            SortMode::Name => "Name (A-Z)",
            SortMode::NameDesc => "Name (Z-A)",
            SortMode::Largest => "Most messages",
            SortMode::Smallest => "Fewest messages",
            SortMode::Kind => "Type",
        }
    }

    /// An unknown key falls back rather than failing.
    ///
    /// The sort arrives from a hand-editable file. A typo in it must cost the
    /// user their sort preference and nothing else — never the chat list.
    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|m| m.key() == key)
            .unwrap_or(SortMode::Recent)
    }

    /// Does this mode run largest-first?
    ///
    /// Only the primary key is reversed by this; see [`directed`].
    fn descending(self) -> bool {
        matches!(
            self,
            SortMode::Recent | SortMode::NameDesc | SortMode::Largest
        )
    }

    pub const ALL: [SortMode; 7] = [
        SortMode::Recent,
        SortMode::Oldest,
        SortMode::Name,
        SortMode::NameDesc,
        SortMode::Largest,
        SortMode::Smallest,
        SortMode::Kind,
    ];
}

/// The five buckets, in the fixed order they are painted.
///
/// Groups and supergroups are separate: a basic group and a supergroup differ
/// in what an export of them costs and contains — only supergroups can be
/// forums, and only they carry the message history a new member can read — so
/// they are worth choosing between rather than ticking as one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Channels,
    Groups,
    Supergroups,
    Private,
    Bots,
}

impl Category {
    pub fn of(kind: ChatKind) -> Self {
        match kind {
            ChatKind::Channel => Category::Channels,
            ChatKind::Group => Category::Groups,
            ChatKind::Supergroup => Category::Supergroups,
            ChatKind::Private => Category::Private,
            ChatKind::Bot => Category::Bots,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Channels => "Channels",
            Category::Groups => "Groups",
            Category::Supergroups => "Supergroups",
            Category::Private => "Private chats",
            Category::Bots => "Bots",
        }
    }

    /// The name a fold is remembered under. On-disk format, like
    /// [`SortMode::key`] — renaming one silently forgets a user's folds.
    pub fn key(self) -> &'static str {
        match self {
            Category::Channels => "channel",
            Category::Groups => "group",
            Category::Supergroups => "supergroup",
            Category::Private => "personal",
            Category::Bots => "bot",
        }
    }

    pub const ALL: [Category; 5] = [
        Category::Channels,
        Category::Groups,
        Category::Supergroups,
        Category::Private,
        Category::Bots,
    ];
}

/// One painted row: either a category heading or a chat.
#[derive(Debug, Clone, PartialEq)]
pub enum Row<'a> {
    Heading {
        category: Category,
        /// How many chats this heading owns *right now*, after the filter.
        ///
        /// Carried as a number rather than glued onto the label, because a
        /// count inside the string is something the filter would then search.
        /// It counts what is painted rather than the bucket's full size, so a
        /// heading can never contradict the rows under it: a pre-filter count
        /// reads "Channels 40" above three matches.
        total: usize,
        /// Whether this category is folded shut.
        ///
        /// The same answer the caller's own fold set gives — see
        /// [`reopen_matched`], which is where a search changes it, and why
        /// that is not done here.
        folded: bool,
    },
    Chat(&'a ChatInfo),
}

/// Everything the list needs to decide what to paint.
#[derive(Debug, Clone)]
pub struct View {
    pub sort: SortMode,
    /// Off means one flat list with the sort applied across every chat at once.
    pub grouped: bool,
    pub filter: String,
    pub folded: HashSet<Category>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            sort: SortMode::Recent,
            // Matches `Settings::default().group_by_type`, so a first run and a
            // saved run open on the same shape.
            grouped: true,
            filter: String::new(),
            folded: HashSet::new(),
        }
    }
}

/// Apply a descending mode's direction to a *primary* key, and to nothing else.
///
/// A framework that inverts the whole comparison for a descending sort forces
/// every tie-break to be un-inverted by hand. Writing the comparator directly
/// reverses that burden: nothing is inverted unless it is passed through here,
/// so this is called on the primary key alone.
///
/// The alphabetical tie-break, the fixed category order and the position of an
/// uncounted chat are all deliberately outside it. Reversing those is what
/// makes a Z-A sort reorder equal rows arbitrarily, turn the category list
/// upside down, and float the blank counts to the top.
fn directed(ordering: Ordering, descending: bool) -> Ordering {
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}

/// The form of a title that sorting and filtering compare.
///
/// Exists so that a title's capitalisation never decides its place in the list.
fn folded_title(title: &str) -> String {
    title.to_lowercase()
}

/// Order two chats under one sort mode, on real values rather than on the text
/// that happens to be painted.
///
/// Without this the count column orders `100` before `99` and the date column
/// sorts alphabetically, putting "Yesterday" after "Wednesday".
fn compare(a: &ChatInfo, b: &ChatInfo, sort: SortMode) -> Ordering {
    let descending = sort.descending();

    let primary = match sort {
        SortMode::Recent | SortMode::Oldest => {
            directed(a.last_activity.cmp(&b.last_activity), descending)
        }
        SortMode::Name | SortMode::NameDesc => directed(
            folded_title(&a.title).cmp(&folded_title(&b.title)),
            descending,
        ),
        SortMode::Largest | SortMode::Smallest => match (a.message_count, b.message_count) {
            (Some(x), Some(y)) => directed(x.cmp(&y), descending),
            // **A missing count is not a count of zero**, and an uncounted chat
            // sorts below every counted one *in both directions* — the row is
            // blank, and a column of blanks at the top reads as an empty list.
            // Kept out of `directed` on purpose. A sentinel value — -1 for "no
            // count" — gets this only half right, because the direction flip
            // applies to the sentinel too and floats the blanks to the top of
            // the ascending sort.
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (None, None) => Ordering::Equal,
        },
        // The kind column sorts on its label, which is the value: there is
        // nothing behind "Supergroup" but the word.
        SortMode::Kind => directed(
            folded_title(a.kind.label()).cmp(&folded_title(b.kind.label())),
            descending,
        ),
    };
    if primary != Ordering::Equal {
        return primary;
    }

    // Ties read alphabetically in both directions, never arbitrarily. Not
    // directed: a Z-A sort reverses the counts, not the meaning of "next".
    folded_title(&a.title).cmp(&folded_title(&b.title))
}

/// Does this chat survive the filter?
///
/// Case-insensitive, on the **stored** title. Nothing in this module appends to
/// a title — a forum is marked by a painted dot, never by a suffix — because
/// presentation in the string is what the filter then searches.
fn matches(chat: &ChatInfo, needle: &str) -> bool {
    needle.is_empty() || folded_title(&chat.title).contains(needle)
}

/// The chats the filter leaves, bucketed and sorted the way they will paint.
///
/// One bucket with no category when flat; one per non-empty category, in
/// [`Category::ALL`] order, when grouped. **A category with nothing in it is
/// not returned at all**, so no caller can paint an empty heading.
fn buckets<'a>(chats: &'a [ChatInfo], view: &View) -> Vec<(Option<Category>, Vec<&'a ChatInfo>)> {
    let needle = view.filter.trim().to_lowercase();
    let kept = || chats.iter().filter(|c| matches(c, &needle));

    let mut out: Vec<(Option<Category>, Vec<&ChatInfo>)> = if view.grouped {
        // Walked in the fixed order rather than sorted into one, which is how
        // the category order stays put whichever way the sort runs.
        Category::ALL
            .into_iter()
            .filter_map(|category| {
                let members: Vec<&ChatInfo> = kept()
                    .filter(|c| Category::of(c.kind) == category)
                    .collect();
                (!members.is_empty()).then_some((Some(category), members))
            })
            .collect()
    } else {
        let all: Vec<&ChatInfo> = kept().collect();
        if all.is_empty() {
            Vec::new()
        } else {
            vec![(None, all)]
        }
    };

    for (_, members) in out.iter_mut() {
        members.sort_by(|a, b| compare(a, b, view.sort));
    }
    out
}

/// Re-open every folded category a search has found something in.
///
/// **A closed category hides its matches, which reads as "no results" rather
/// than "closed".** So a search opens what it found — and it does so by
/// *changing the fold*, once, when the filter changes, rather than by
/// overriding it while painting.
///
/// That distinction is the whole of this function. An override left the fold
/// set saying one thing and the chevron another, so the first click on a
/// heading during a search removed a fold nobody could see and nothing moved;
/// it took two clicks to close a category the screen already showed as open.
/// Here the two can never disagree, and the user can still fold a category
/// while a search is running, because folding is a real state change and not a
/// paint rule.
///
/// Call it when the filter changes. Calling it on every frame would make a fold
/// during a search impossible to keep.
pub fn reopen_matched(chats: &[ChatInfo], view: &mut View) {
    if view.filter.trim().is_empty() || view.folded.is_empty() {
        return;
    }
    let needle = view.filter.trim().to_lowercase();
    for chat in chats.iter().filter(|c| matches(c, &needle)) {
        view.folded.remove(&Category::of(chat.kind));
    }
}

/// The rows to paint, in order.
pub fn rows<'a>(chats: &'a [ChatInfo], view: &View) -> Vec<Row<'a>> {
    let mut out = Vec::new();
    for (category, members) in buckets(chats, view) {
        let Some(category) = category else {
            // Flat mode: one list, the sort applied across every chat at once.
            out.extend(members.into_iter().map(Row::Chat));
            continue;
        };
        // Straight from the fold set: see [`reopen_matched`] for why a search
        // must not override this here.
        let folded = view.folded.contains(&category);
        out.push(Row::Heading {
            category,
            total: members.len(),
            folded,
        });
        if !folded {
            out.extend(members.into_iter().map(Row::Chat));
        }
    }
    out
}

/// The chats a filter leaves visible — what All / None / Invert / Only forums
/// act on.
///
/// **A folded category's chats still count as visible.** Folding is a way of
/// looking at the list, not a second filter: if it excluded chats, tidying the
/// view would silently change what All selects, with nothing on screen saying
/// so, and the ticks would disagree with the footer's total.
///
/// Returned in painting order, so a caller may zip it against [`rows`].
pub fn visible<'a>(chats: &'a [ChatInfo], view: &View) -> Vec<&'a ChatInfo> {
    buckets(chats, view)
        .into_iter()
        .flat_map(|(_, members)| members)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: i64, title: &str, count: Option<i64>) -> ChatInfo {
        ChatInfo {
            id,
            title: title.into(),
            kind: ChatKind::Supergroup,
            last_activity: 0,
            is_forum: false,
            public: false,
            message_count: count,
        }
    }

    fn kinded(id: i64, title: &str, kind: ChatKind) -> ChatInfo {
        ChatInfo {
            kind,
            ..chat(id, title, Some(1))
        }
    }

    fn flat(sort: SortMode) -> View {
        View {
            sort,
            grouped: false,
            ..View::default()
        }
    }

    /// The titles of the chat rows, in painted order.
    fn titles(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Chat(c) => Some(c.title.clone()),
                Row::Heading { .. } => None,
            })
            .collect()
    }

    fn headings(rows: &[Row<'_>]) -> Vec<Category> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Heading { category, .. } => Some(*category),
                Row::Chat(_) => None,
            })
            .collect()
    }

    #[test]
    fn sorting_by_size_orders_on_the_number_not_the_text() {
        // As text, "100" precedes "99". As a number it does not, and the
        // painted column is text.
        let chats = vec![
            chat(1, "ninety nine", Some(99)),
            chat(2, "hundred", Some(100)),
        ];
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Largest))),
            ["hundred", "ninety nine"]
        );
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Smallest))),
            ["ninety nine", "hundred"]
        );
    }

    #[test]
    fn an_uncounted_chat_sorts_last_in_both_directions() {
        // A blank row is not a row with nothing in it; it is a row nobody has
        // counted. A column of blanks at the top reads as an empty list.
        let chats = vec![
            chat(1, "unknown", None),
            chat(2, "small", Some(2)),
            chat(3, "big", Some(900)),
        ];
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Largest))),
            ["big", "small", "unknown"]
        );
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Smallest))),
            ["small", "big", "unknown"]
        );
    }

    #[test]
    fn an_uncounted_chat_is_not_a_chat_with_no_messages() {
        // Some(0) is a count and sorts as one; None has no place in the numbers
        // at all.
        let chats = vec![chat(1, "unknown", None), chat(2, "empty", Some(0))];
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Smallest))),
            ["empty", "unknown"]
        );
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Largest))),
            ["empty", "unknown"]
        );
    }

    #[test]
    fn ties_read_alphabetically_in_both_directions() {
        // Equal counts must not shuffle when the direction flips: the rows that
        // did not change value must not appear to have changed order.
        let chats = vec![
            chat(1, "Zulu", Some(5)),
            chat(2, "alpha", Some(5)),
            chat(3, "Mike", Some(5)),
        ];
        for sort in [SortMode::Largest, SortMode::Smallest] {
            assert_eq!(
                titles(&rows(&chats, &flat(sort))),
                ["alpha", "Mike", "Zulu"],
                "{}",
                sort.label()
            );
        }
        // And on a date sort, where every chat here shares a timestamp.
        for sort in [SortMode::Recent, SortMode::Oldest] {
            assert_eq!(
                titles(&rows(&chats, &flat(sort))),
                ["alpha", "Mike", "Zulu"]
            );
        }
    }

    #[test]
    fn a_name_sort_still_reverses_when_asked() {
        // The tie-break is un-inverted; the primary key is not. Z-A must
        // actually run Z-A.
        let chats = vec![chat(1, "alpha", None), chat(2, "Zulu", None)];
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Name))),
            ["alpha", "Zulu"]
        );
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::NameDesc))),
            ["Zulu", "alpha"]
        );
    }

    #[test]
    fn the_category_order_is_fixed_whichever_way_the_sort_runs() {
        let chats = vec![
            kinded(1, "a bot", ChatKind::Bot),
            kinded(2, "a channel", ChatKind::Channel),
            kinded(3, "a group", ChatKind::Group),
            kinded(4, "a supergroup", ChatKind::Supergroup),
            kinded(5, "a person", ChatKind::Private),
        ];
        for sort in SortMode::ALL {
            let view = View {
                sort,
                ..View::default()
            };
            assert_eq!(
                headings(&rows(&chats, &view)),
                Category::ALL,
                "{}",
                sort.label()
            );
        }
    }

    #[test]
    fn groups_and_supergroups_land_in_different_buckets() {
        // They differ in what an export of them costs and contains, so they are
        // worth choosing between rather than ticking as one block.
        let chats = vec![
            kinded(1, "basic", ChatKind::Group),
            kinded(2, "super", ChatKind::Supergroup),
        ];
        let painted = rows(&chats, &View::default());
        assert_eq!(
            headings(&painted),
            [Category::Groups, Category::Supergroups]
        );
        assert_ne!(
            Category::of(ChatKind::Group),
            Category::of(ChatKind::Supergroup)
        );
        // And a bot is not a private chat, though both are one-to-one.
        assert_ne!(Category::of(ChatKind::Bot), Category::of(ChatKind::Private));
    }

    #[test]
    fn a_search_reopens_a_folded_category_that_matched() {
        // A closed category hides its matches, which reads as "no results"
        // rather than "closed".
        let chats = vec![kinded(1, "news desk", ChatKind::Channel)];
        let mut view = View {
            folded: HashSet::from([Category::Channels]),
            ..View::default()
        };
        let closed = rows(&chats, &view);
        assert_eq!(
            closed,
            vec![Row::Heading {
                category: Category::Channels,
                total: 1,
                folded: true,
            }]
        );

        view.filter = "news".into();
        reopen_matched(&chats, &mut view);
        let searched = rows(&chats, &view);
        assert!(
            matches!(searched[0], Row::Heading { folded: false, .. }),
            "the fold must open, and say so, or the caller repaints it closed"
        );
        assert_eq!(titles(&searched), ["news desk"]);
        assert!(
            view.folded.is_empty(),
            "the re-open must change the fold, not paint over it"
        );
    }

    #[test]
    fn a_category_can_still_be_folded_while_a_search_is_running() {
        // The failure this rules out: with the re-open done in the painting,
        // the chevron said "open" while the set said "folded", so the first
        // click removed a fold nobody could see and nothing moved on screen.
        let chats = vec![kinded(1, "news desk", ChatKind::Channel)];
        let mut view = View {
            filter: "news".into(),
            ..View::default()
        };
        reopen_matched(&chats, &mut view);
        assert!(matches!(
            rows(&chats, &view)[0],
            Row::Heading { folded: false, .. }
        ));

        // Now fold it, with the search still in place. It stays folded until
        // the filter changes again.
        view.folded.insert(Category::Channels);
        let painted = rows(&chats, &view);
        assert!(matches!(painted[0], Row::Heading { folded: true, .. }));
        assert_eq!(titles(&painted), Vec::<&str>::new());
    }

    #[test]
    fn a_search_that_matches_nothing_in_a_category_leaves_its_fold_alone() {
        // Only categories a search actually found something in are opened —
        // opening the rest would quietly undo folds the user set.
        let chats = vec![
            kinded(1, "news desk", ChatKind::Channel),
            kinded(2, "a bot", ChatKind::Bot),
        ];
        let mut view = View {
            folded: HashSet::from([Category::Channels, Category::Bots]),
            filter: "news".into(),
            ..View::default()
        };
        reopen_matched(&chats, &mut view);
        assert!(!view.folded.contains(&Category::Channels));
        assert!(
            view.folded.contains(&Category::Bots),
            "Bots matched nothing"
        );
    }

    #[test]
    fn clearing_the_filter_reopens_nothing() {
        // An empty filter is not a search, and treating it as one would open
        // every fold the moment the box was cleared.
        let chats = vec![kinded(1, "news desk", ChatKind::Channel)];
        let mut view = View {
            folded: HashSet::from([Category::Channels]),
            ..View::default()
        };
        reopen_matched(&chats, &mut view);
        assert!(view.folded.contains(&Category::Channels));
        view.filter = "   ".into();
        reopen_matched(&chats, &mut view);
        assert!(
            view.folded.contains(&Category::Channels),
            "whitespace is not a search"
        );
    }

    #[test]
    fn an_empty_category_is_not_painted() {
        // No headings over nothing: an account with no bots has no Bots row.
        let chats = vec![kinded(1, "only a channel", ChatKind::Channel)];
        assert_eq!(
            headings(&rows(&chats, &View::default())),
            [Category::Channels]
        );
    }

    #[test]
    fn a_filter_matching_nothing_yields_no_chat_rows() {
        // Not one empty heading either — the window paints its own "nothing
        // matches" state over this, and a stray heading would sit beside it.
        let chats = vec![kinded(1, "news", ChatKind::Channel)];
        let view = View {
            filter: "zzz".into(),
            ..View::default()
        };
        assert!(rows(&chats, &view).is_empty());
        assert!(visible(&chats, &view).is_empty());
    }

    #[test]
    fn a_heading_counts_what_it_paints() {
        let chats = vec![
            kinded(1, "news desk", ChatKind::Channel),
            kinded(2, "news wire", ChatKind::Channel),
            kinded(3, "weather", ChatKind::Channel),
        ];
        let mut view = View::default();
        assert!(matches!(
            rows(&chats, &view)[0],
            Row::Heading { total: 3, .. }
        ));
        view.filter = "news".into();
        assert!(
            matches!(rows(&chats, &view)[0], Row::Heading { total: 2, .. }),
            "a heading must not claim more than the rows beneath it"
        );
    }

    #[test]
    fn the_filter_is_case_insensitive_and_reads_the_stored_title() {
        let chats = vec![chat(1, "UA KOLAB TELEGRAM", None)];
        for needle in ["ua kolab", "  KOLAB  ", "Telegram"] {
            let view = View {
                filter: needle.into(),
                ..View::default()
            };
            assert_eq!(visible(&chats, &view).len(), 1, "{needle:?}");
        }
    }

    #[test]
    fn nothing_here_marks_a_forum_in_the_title() {
        // A forum is a painted dot, never a suffix on the stored title:
        // presentation in the string is what the filter then searches, so
        // "topics" would match a chat that never said it.
        let forum = ChatInfo {
            is_forum: true,
            ..chat(1, "UA KOLAB TELEGRAM", None)
        };
        let chats = vec![forum];
        assert_eq!(
            visible(&chats, &View::default())[0].title,
            "UA KOLAB TELEGRAM"
        );
        let view = View {
            filter: "topics".into(),
            ..View::default()
        };
        assert!(visible(&chats, &view).is_empty());
    }

    #[test]
    fn a_folded_category_still_counts_as_visible_for_selection() {
        // Folding is a way of looking at the list, not a second filter. If it
        // excluded chats, All would quietly select a different set depending on
        // which headings happened to be closed.
        let chats = vec![
            kinded(1, "a channel", ChatKind::Channel),
            kinded(2, "a bot", ChatKind::Bot),
        ];
        let view = View {
            folded: HashSet::from([Category::Channels]),
            ..View::default()
        };
        assert_eq!(titles(&rows(&chats, &view)), ["a bot"]);
        assert_eq!(visible(&chats, &view).len(), 2);
    }

    #[test]
    fn flat_mode_sorts_across_every_chat_at_once() {
        // One list, no headings: the sort must cross the buckets rather than
        // run inside each of them.
        let chats = vec![
            kinded(1, "zeta", ChatKind::Channel),
            kinded(2, "alpha", ChatKind::Bot),
        ];
        let painted = rows(&chats, &flat(SortMode::Name));
        assert!(headings(&painted).is_empty());
        assert_eq!(titles(&painted), ["alpha", "zeta"]);
    }

    #[test]
    fn the_most_recent_chat_leads_and_the_least_recent_trails() {
        let mut old = chat(1, "old", None);
        old.last_activity = 100;
        let mut new = chat(2, "new", None);
        new.last_activity = 900;
        let chats = vec![old, new];
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Recent))),
            ["new", "old"]
        );
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Oldest))),
            ["old", "new"]
        );
    }

    #[test]
    fn an_unknown_sort_key_falls_back_rather_than_breaking() {
        // The sort arrives from a hand-editable file; a typo in it costs the
        // preference and nothing else.
        assert_eq!(SortMode::from_key("chartreuse"), SortMode::Recent);
        assert_eq!(SortMode::from_key(""), SortMode::Recent);
        assert_eq!(
            SortMode::from_key("Recent"),
            SortMode::Recent,
            "keys are exact"
        );
        for mode in SortMode::ALL {
            assert_eq!(SortMode::from_key(mode.key()), mode);
        }
    }

    #[test]
    fn every_sort_mode_and_category_is_named_once() {
        // The menu is built from ALL, so a duplicate label or key is two
        // identical-looking entries that do different things.
        let keys: HashSet<&str> = SortMode::ALL.iter().map(|m| m.key()).collect();
        let labels: HashSet<&str> = SortMode::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(keys.len(), SortMode::ALL.len());
        assert_eq!(labels.len(), SortMode::ALL.len());
        let cat_keys: HashSet<&str> = Category::ALL.iter().map(|c| c.key()).collect();
        assert_eq!(cat_keys.len(), Category::ALL.len());
    }

    #[test]
    fn the_kind_sort_groups_the_same_kinds_together() {
        let chats = vec![
            kinded(1, "b", ChatKind::Channel),
            kinded(2, "a", ChatKind::Bot),
            kinded(3, "c", ChatKind::Channel),
        ];
        // Bot, then Channel, then the tie-break inside Channel.
        assert_eq!(
            titles(&rows(&chats, &flat(SortMode::Kind))),
            ["a", "b", "c"]
        );
    }

    // -- the caption -------------------------------------------------------

    /// A fixed "now", so the thresholds are tested rather than the clock.
    fn at(text: &str) -> chrono::DateTime<Local> {
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_local_timezone(Local)
            .unwrap()
    }

    #[test]
    fn a_timestamp_reads_the_way_telegram_words_it() {
        let now = at("2026-08-26 14:00:00");
        // Today is a clock time; yesterday is a word, whatever the hour.
        assert_eq!(
            human_when(at("2026-08-26 09:30:00").timestamp(), now),
            "09:30"
        );
        assert_eq!(
            human_when(at("2026-08-25 23:50:00").timestamp(), now),
            "Yesterday"
        );
        // Inside the week, a weekday. Beyond it, a date; beyond the year, a year.
        assert_eq!(
            human_when(at("2026-08-22 10:00:00").timestamp(), now),
            "Saturday"
        );
        assert_eq!(
            human_when(at("2026-03-02 10:00:00").timestamp(), now),
            "02 Mar"
        );
        assert_eq!(
            human_when(at("2024-03-02 10:00:00").timestamp(), now),
            "02 Mar 2024"
        );
    }

    #[test]
    fn yesterday_is_decided_by_the_date_and_not_by_the_hours_between() {
        // A message at 23:50 last night is "Yesterday" at 00:10, not "today,
        // twenty minutes ago" — the comparison is between calendar dates.
        let now = at("2026-08-26 00:10:00");
        assert_eq!(
            human_when(at("2026-08-25 23:50:00").timestamp(), now),
            "Yesterday"
        );
    }

    #[test]
    fn a_chat_with_no_last_message_reads_as_its_kind_alone() {
        // Not its kind followed by a dangling separator.
        let now = at("2026-08-26 14:00:00");
        let mut c = kinded(1, "quiet", ChatKind::Channel);
        c.last_activity = 0;
        assert_eq!(caption(&c, now), "Channel");
        assert_eq!(human_when(0, now), "");
        assert_eq!(human_when(-1, now), "", "a negative stamp is not a date");
    }

    #[test]
    fn the_caption_carries_what_the_default_sort_orders_on() {
        // "Recent activity" sorts on a unix second that appears nowhere on
        // screen unless the row says so.
        let now = at("2026-08-26 14:00:00");
        let mut c = kinded(1, "news", ChatKind::Supergroup);
        c.last_activity = at("2026-08-25 12:00:00").timestamp();
        assert_eq!(caption(&c, now), "Supergroup  Yesterday");
    }
}
