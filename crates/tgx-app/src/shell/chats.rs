//! The chat panel: filter, sort, categories, rows, selection.
//!
//! Three things were missing here rather than merely wrong.
//!
//! **Nothing scrolled.** The list was a plain flex column: at roughly 45px a
//! row, about nine chats fitted and the rest were unreachable by wheel, drag,
//! keyboard or filter — and every row, including the invisible ones, was cloned
//! and laid out on every frame. It is a `uniform_list` now, which lays out only
//! the visible range.
//!
//! **There was no search bar.** `Shell.filter` was read by `visible()`, by
//! `list_state()` and by the `FilterMatchedNothing` empty state, and written
//! *only in tests* — so one of the four carefully built empty states could not
//! be reached in the shipped app at all.
//!
//! **There was no sort and no grouping.** The list preserved raw `iter_dialogs`
//! order; neither the five buckets `ChatKind`'s own doc comment describes nor
//! any of the seven sort modes existed. The logic for both lives in
//! [`crate::list`], which is pure and tested; this file only paints it.

use super::{PaintedRow, Shell};
use crate::list::{self, SortMode};
use chrono::{DateTime, Local};
use gpui::prelude::*;
use gpui::{deferred, div, px, uniform_list, Context, Div, SharedString};
use gpui_component::input::Input;
use gpui_component::scroll::Scrollbar;
use tgx_ui::components::{
    caps, count_text, eyebrow, forum_dot, rule, selection_label, soft_rule, tick_box,
};
use tgx_ui::tokens::type_scale;

/// Every row is this tall, heading and chat alike.
///
/// `uniform_list` measures one item and lays the rest out from it, which is
/// exactly what makes a list of several thousand chats cost the same as a list
/// of nine. The price is that a category heading cannot be shorter than a chat
/// row; it is set in tracked micro-type instead, which distinguishes it by
/// weight rather than by size.
const ROW_HEIGHT: gpui::Pixels = px(46.0);

impl Shell {
    pub(super) fn chat_panel(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let live = self.selection_actions_enabled();
        let (total, any_uncounted) = self.selection_total();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex_none()
                    .px(px(16.0))
                    .py(px(12.0))
                    .child(eyebrow("Chats", p)),
            )
            .child(rule(p))
            .child(self.search_row(cx))
            .child(soft_rule(p))
            .child(self.sort_row(cx))
            .child(rule(p))
            .child(self.list_body(cx))
            .child(rule(p))
            .child(self.selection_row(live, cx))
            .child(div().flex_none().px(px(16.0)).pb(px(10.0)).child(caps(
                selection_label(self.selected.len(), total, any_uncounted),
                type_scale::MICRO,
                p.muted,
            )))
    }

    fn search_row(&self, _cx: &mut Context<Self>) -> Div {
        let row = div().flex_none().px(px(16.0)).py(px(10.0));
        match &self.search {
            // The headless shell has no window and therefore no input state.
            None => row,
            Some(state) => row.child(Input::new(state).cleanable(true)),
        }
    }

    /// SORT and GROUP BY TYPE.
    ///
    /// The sort is a menu rather than seven chips: seven labels do not fit
    /// across a panel this width, and cycling through seven with one click
    /// means six clicks to undo a mistake.
    fn sort_row(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let current = self.view.sort;

        let mut row = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .py(px(8.0))
            .child(caps("Sort", type_scale::MICRO, p.muted))
            .child(
                div()
                    .id("sort-menu")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .text_size(type_scale::TINY)
                    .text_color(p.fg)
                    .child(SharedString::from(format!("{} \u{25be}", current.label())))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sort_open = !this.sort_open;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("group-by-type")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .child(tick_box(self.view.grouped, true, p))
                    .child(caps("Group by type", type_scale::MICRO, p.muted))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.view.grouped = !this.view.grouped;
                        this.rebuild_rows();
                        this.commit_settings(window, cx);
                    })),
            );

        if self.sort_open {
            // **The way out.** Choosing a mode was the only thing that closed
            // the menu: clicking away left it standing over the list while the
            // click did whatever it landed on. This catcher takes that click,
            // and `occlude` is what stops the click reaching anything else —
            // gpui hands a click to *every* hitbox under the pointer unless one
            // in front blocks the rest, and "in front" is paint order.
            //
            // It carries an `id`, without which it would take part in no hit
            // testing at all and look exactly like no catcher having been
            // added, and it is built only while the menu is open, so nothing is
            // swallowed the rest of the time.
            //
            // Sized past any display rather than to the row: it is positioned
            // against the sort row, which is a few pixels tall, and an
            // absolutely positioned child has no way to ask for the size of the
            // window. A catcher that stopped where the row stops would leave
            // every chat row under it live.
            let catcher = div()
                .id("sort-dismiss")
                .absolute()
                .left(px(-8000.0))
                .top(px(-8000.0))
                .size(px(16000.0))
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sort_open = false;
                    cx.notify();
                }));

            let mut menu = div()
                .absolute()
                .top(px(30.0))
                .left(px(16.0))
                .w(px(200.0))
                .bg(p.bg)
                .border_1()
                .border_color(p.hairline)
                .flex()
                .flex_col();
            for (i, mode) in SortMode::ALL.into_iter().enumerate() {
                let chosen = mode == current;
                menu = menu.child(
                    div()
                        .id(("sort-option", i))
                        .px(px(12.0))
                        .py(px(8.0))
                        .cursor_pointer()
                        .text_size(type_scale::TINY)
                        .text_color(if chosen { p.accent } else { p.fg })
                        .child(SharedString::from(mode.label()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.view.sort = mode;
                            this.sort_open = false;
                            this.rebuild_rows();
                            // Sorting by size with no counts fetched yet is not
                            // an error, but it is a list that will not move —
                            // say so rather than leaving it looking broken.
                            if matches!(mode, SortMode::Largest | SortMode::Smallest)
                                && this.chats.iter().all(|c| c.message_count.is_none())
                            {
                                this.status = "No message counts yet — press Count messages".into();
                            }
                            this.commit_settings(window, cx);
                        })),
                );
            }
            // The menu is a child of a `relative` row, so it overlays the list
            // instead of pushing it down — a menu that reflows the thing it is
            // sorting moves the row the pointer is over.
            //
            // **Both halves are deferred, and that is what makes the overlay
            // real.** `deferred` keeps the layout here — the absolute offsets
            // above still measure from this row — but moves the *paint* to
            // after every other element in the window. Without it the row
            // paints before the list does, which put the menu underneath: a
            // category heading's opaque background covered the options it
            // overlapped, a click on one of those options also ticked the chat
            // row beneath it, and the catcher could block nothing, because
            // occlusion only reaches hitboxes inserted earlier.
            //
            // Deferred draws are ordered by priority, so the menu's is higher
            // than the catcher's: the catcher blocks the window behind it while
            // the menu stays clickable in front of it. A click on an option
            // still runs the catcher's listener too — both are under the
            // pointer — but all that does is close a menu the option closes
            // anyway.
            row = row
                .relative()
                .child(deferred(catcher))
                .child(deferred(menu).with_priority(1));
        }
        row
    }

    fn list_body(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let wrap = div().flex().flex_col().flex_1().min_h_0().relative();

        if let Some(empty) = self.list_state().empty_state(&self.view.filter) {
            return wrap.child(div().flex_1().child(empty.render(p, true)));
        }

        let count = self.rows.len();
        // **The clock is read once a frame, here, and carried down.** Every
        // caption says how long ago the chat last moved, and reading the clock
        // inside the callback would be one syscall per visible row per frame —
        // for an answer that cannot change between two rows of the same list.
        let now = Local::now();
        wrap.child(
            uniform_list(
                "chats",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    let mut painted = Vec::with_capacity(range.len());
                    for i in range {
                        let Some(row) = this.rows.get(i) else {
                            continue;
                        };
                        painted.push(this.paint_row(i, row, now, cx));
                    }
                    painted
                }),
            )
            .flex_1()
            .track_scroll(self.chat_scroll.clone()),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .child(Scrollbar::vertical(&self.chat_scroll)),
        )
    }

    /// One row. `now` is passed in rather than read here — see [`Self::list_body`].
    fn paint_row(
        &self,
        index: usize,
        row: &PaintedRow,
        now: DateTime<Local>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = &self.palette;
        match row {
            PaintedRow::Heading {
                category,
                total,
                folded,
            } => {
                let category = *category;
                let folded = *folded;
                div()
                    .id(("heading", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(ROW_HEIGHT)
                    .px(px(16.0))
                    .cursor_pointer()
                    .bg(p.surface)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            // The disclosure marker is drawn, not a glyph from
                            // a font that may not have it.
                            .child(div().text_size(type_scale::TINY).text_color(p.muted).child(
                                SharedString::from(if folded { "\u{25b8}" } else { "\u{25be}" }),
                            ))
                            .child(caps(category.label(), type_scale::MICRO, p.fg)),
                    )
                    .child(
                        div()
                            .text_size(type_scale::TINY)
                            .text_color(p.muted)
                            .child(SharedString::from(total.to_string())),
                    )
                    // Folding is a way of looking at the list, not a second
                    // filter: a folded category's chats still count as visible,
                    // so tidying the view cannot silently change what All
                    // selects. See `crate::list`.
                    //
                    // **The click inverts the chevron it was painted beside**,
                    // rather than re-deriving the state from `view.folded`. A
                    // heading whose painted state and stored state ever part
                    // company — a search re-opening a category is where that
                    // happened — would otherwise take one click to undo a fold
                    // nobody could see and a second to do what was asked.
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if folded {
                            this.view.folded.remove(&category);
                        } else {
                            this.view.folded.insert(category);
                        }
                        this.rebuild_rows();
                        this.commit_settings(window, cx);
                    }))
                    .into_any_element()
            }
            PaintedRow::Chat(chat) => self.paint_chat(index, chat, now, cx).into_any_element(),
        }
    }

    fn paint_chat(
        &self,
        index: usize,
        chat: &tgx_tg::client::ChatInfo,
        now: DateTime<Local>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = &self.palette;
        let ticked = self.selected.contains(&chat.id);
        let id = chat.id;
        div()
            .id(("chat", index))
            .flex()
            .items_center()
            .gap(px(12.0))
            .h(ROW_HEIGHT)
            .px(px(16.0))
            .when(self.view.grouped, |d| d.pl(px(30.0)))
            .cursor_pointer()
            // Clicking anywhere on a row ticks it. Ticks are held by chat id,
            // so they survive re-sorting, regrouping and filtering.
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.selected.remove(&id) {
                    this.selected.insert(id);
                }
                cx.notify();
            }))
            .child(tick_box(ticked, true, p))
            // A forum is marked by a painted dot, never by a suffix on the
            // stored title — presentation in the string is what the filter then
            // searches.
            .when(chat.is_forum, |d| {
                d.child(div().flex_none().w(px(6.0)).h(px(6.0)).bg(forum_dot(p)))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    // A long title must not push the count off the panel: a
                    // flex child's automatic minimum size is its content.
                    .min_w_0()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(type_scale::SMALL)
                            .text_color(p.fg)
                            .child(SharedString::from(chat.title.clone())),
                    )
                    // The caption is the kind and the last activity, tracked
                    // uppercase micro-type in the mono — `theme.mono(T_TINY,
                    // Normal, LS_CAPS)` in the original's delegate. **The date
                    // is half its point**: the default sort is Recent activity,
                    // and without it the list is ordered on a unix second that
                    // appears nowhere on screen.
                    //
                    // `caps` letterspaces by splitting the string into one child
                    // per character, so it cannot wrap and it cannot be elided
                    // with `truncate` — a caption too wide for the column is
                    // clipped by the parent instead, which is the original's
                    // `ElideRight` minus the ellipsis. Safe to track because it
                    // is our own bounded wording; the title above it is a user
                    // string and stays one selectable run.
                    .child(
                        div().w_full().min_w_0().overflow_hidden().child(
                            caps(list::caption(chat, now), type_scale::MICRO, p.muted)
                                .font(tgx_ui::fonts::mono()),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    // The count is the row's one number, and the design sets
                    // every number in the mono. `font` and not `font_family`:
                    // the family name alone drops the rest of the `Font` — its
                    // weight, its fallbacks and whatever features it carries.
                    .font(tgx_ui::fonts::mono())
                    .text_size(type_scale::TINY)
                    .text_color(p.muted)
                    .child(count_text(chat.message_count)),
            )
    }

    /// All / None / Invert / Only forums, and the Count button.
    ///
    /// Nothing offers to do what it cannot: these act on the *visible* rows, so
    /// over an empty list every one is a no-op. A button that is enabled and
    /// does nothing teaches that the interface is unreliable.
    fn selection_row(&self, live: bool, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let mut actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(14.0))
            .px(px(16.0))
            .py(px(8.0));

        for (i, label) in ["All", "None", "Invert", "Only forums"].iter().enumerate() {
            let mut cell = div().id(("sel", i)).child(caps(
                *label,
                type_scale::MICRO,
                if live { p.fg } else { p.muted },
            ));
            if live {
                cell = cell
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Asked once: `visible()` sorts the whole account, and
                        // every one of these four needs the same answer.
                        let visible: Vec<(i64, bool)> =
                            this.visible().iter().map(|c| (c.id, c.is_forum)).collect();
                        match i {
                            0 => this.selected.extend(visible.iter().map(|(id, _)| *id)),
                            1 => {
                                for (id, _) in &visible {
                                    this.selected.remove(id);
                                }
                            }
                            2 => {
                                for (id, _) in &visible {
                                    if !this.selected.remove(id) {
                                        this.selected.insert(*id);
                                    }
                                }
                            }
                            // **"Only forums" replaces, it does not add.** The
                            // label promises an exclusive selection, and adding
                            // to one meant clicking All and then this left every
                            // chat ticked with the footer still counting the lot.
                            // Assigning the set outright would do it, but the
                            // replacement is scoped to the visible rows like
                            // the other three, so a tick on a chat the
                            // filter is hiding is left where it is rather than
                            // silently dropped.
                            3 => {
                                for (id, is_forum) in &visible {
                                    if *is_forum {
                                        this.selected.insert(*id);
                                    } else {
                                        this.selected.remove(id);
                                    }
                                }
                            }
                            _ => {}
                        }
                        cx.notify();
                    }));
            }
            actions = actions.child(cell);
        }

        // **The label states the action; the price goes in the row under it.**
        // "Count messages (enables sorting by size)" named a side effect rather
        // than the action and said nothing about what it costs — one request
        // per chat, minutes on a large account, and a rate-limit wait if
        // Telegram decides so.
        let countable = !self.chats.is_empty() && !self.exporting;
        let label = if self.counting {
            "Stop counting"
        } else {
            "Count messages"
        };
        actions = actions.child(div().flex_1()).child({
            let mut cell = div().id("count").child(caps(
                label,
                type_scale::MICRO,
                if countable { p.accent } else { p.muted },
            ));
            if countable {
                cell = cell
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_count();
                        cx.notify();
                    }));
            }
            cell
        });
        actions
    }
}
