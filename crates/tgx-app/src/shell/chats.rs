//! The chat list: the filter, the sort, the rows and the selection controls.
//!
//! **The rows are a cache**, rebuilt by `Shell::rebuild_rows` when something
//! changes what they would be. `show_rows` is virtualised, so it asks only for
//! the range on screen — re-filtering and re-sorting the whole account inside
//! that callback would be work done sixty times a second for an answer that
//! changed once.

use super::*;
use crate::list::SortMode;
use chrono::{DateTime, Local};
use eframe::egui::{Align, Layout, Sense, Ui};
use tgx_ui::components::{
    action, button, caps, count_text, eyebrow, field, figure, forum_dot, row, rule,
    selection_label, text, tick_box, title, GUTTER,
};
use tgx_ui::tokens::{space, window};

/// Every row is this tall, heading and chat alike.
///
/// The list is laid out from one row height, which is what makes several
/// thousand chats cost the same as nine. The price is that a category heading
/// cannot be shorter than a chat row; it is set in tracked micro-type instead,
/// which distinguishes it by weight rather than by size.
const ROW_HEIGHT: f32 = 46.0;

impl Shell {
    /// **Head, foot, then the list — as panels, not as measured heights.**
    ///
    /// The first pass reserved `62.0` off the bottom by hand before laying the
    /// list out, because that is what the flex column it came from did
    /// implicitly. egui already has the mechanism: a `TopBottomPanel` measures
    /// itself against its own contents and the central area gets exactly what is
    /// left. So the magic number goes, and the footer is correct at any height
    /// and any type size rather than at the one it was measured on.
    pub(super) fn chat_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let bare = egui::Frame::NONE.fill(p.bg);

        egui::TopBottomPanel::top("chats-head")
            .frame(bare)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                // **One rule, under the title.** The head used to carry three —
                // after the eyebrow, between search and sort, and under sort —
                // which is a line every twenty points down a panel that has one
                // idea in it. The controls are grouped by `BREAK` instead.
                ui.add_space(space::STEP);
                row(ui, |ui| ui.label(title("Chats", &p)));
                ui.add_space(space::TIGHT);
                rule(ui, &p);
                ui.add_space(space::STEP);
                self.search_row(ui);
                ui.add_space(space::STEP);
                self.sort_row(ui);
                ui.add_space(space::STEP);
            });

        egui::TopBottomPanel::bottom("chats-foot")
            .frame(bare)
            .show_separator_line(false)
            .show_inside(ui, |ui| {
                let live = self.selection_actions_enabled();
                let (total, any_uncounted) = self.selection_total();
                // The foot keeps its rule: below it is a summary of the list and
                // what to do with it, which is a different kind of thing from
                // the list itself.
                rule(ui, &p);
                ui.add_space(space::STEP);
                self.selection_row(ui, live);
                ui.add_space(space::TIGHT);
                row(ui, |ui| {
                    ui.label(eyebrow(
                        &selection_label(self.selected.len(), total, any_uncounted),
                        &p,
                    ))
                });
                ui.add_space(space::STEP);
            });

        egui::CentralPanel::default()
            .frame(bare)
            .show_inside(ui, |ui| self.list_body(ui));
    }

    fn search_row(&mut self, ui: &mut Ui) {
        let p = self.palette;
        row(ui, |ui| {
            let before = self.search.clone();
            // `desired_width` alone, with no `add_sized`: inside the gutter the
            // available width already is the width this field should take, so
            // there is nothing left to subtract by hand.
            ui.add(
                field(&mut self.search, &p)
                    .hint_text("Filter chats…")
                    .desired_width(ui.available_width()),
            );
            // Read from the field, never typed into a mirror of it: a second
            // copy of the same string is how the empty state ends up quoting
            // something other than what is in the box.
            if self.search != before {
                let typed = self.search.clone();
                self.set_filter(typed);
            }
        });
    }

    /// SORT and GROUP BY TYPE.
    ///
    /// The sort is a menu rather than seven chips: seven labels do not fit
    /// across a panel this width, and cycling through seven with one click means
    /// six clicks to undo a mistake.
    ///
    /// **It is a `ComboBox`, and that removed a field from the shell.** GPUI
    /// needed a whole catcher apparatus to dismiss a menu — an absolutely
    /// positioned 16000px hitbox with `occlude`, deferred below the menu — so
    /// the swap carried over a hand-driven `Popup` and a `sort_open: bool` to go
    /// with it. Openness is the widget's business, not the window's: egui keeps
    /// it against the widget's own id, and "is a menu open?" is no longer a
    /// question this struct can answer wrongly.
    fn sort_row(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let before = self.view.sort;
        let mut chosen = before;

        row(ui, |ui| {
            ui.label(eyebrow("Sort", &p));
            egui::ComboBox::from_id_salt("sort")
                .selected_text(text(before.label(), &p))
                .width(180.0)
                .show_ui(ui, |ui| {
                    for mode in SortMode::ALL {
                        // The chosen mode is not marked in the accent: the
                        // accent means *this run*, and a sort order is not one.
                        // `selectable_value` already fills the selected item.
                        ui.selectable_value(&mut chosen, mode, text(mode.label(), &p));
                    }
                });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(eyebrow("Group by type", &p));
                if tick_box(ui, self.view.grouped, true, &p).clicked() {
                    self.view.grouped = !self.view.grouped;
                    self.rebuild_rows();
                    self.commit_settings();
                }
            });
        });

        if chosen != before {
            self.view.sort = chosen;
            self.rebuild_rows();
            // Sorting by size with no counts fetched yet is not an error, but it
            // is a list that will not move — say so rather than leaving it
            // looking broken.
            if matches!(chosen, SortMode::Largest | SortMode::Smallest)
                && self.chats.iter().all(|c| c.message_count.is_none())
            {
                self.status = "No message counts yet — press Count messages".into();
            }
            self.commit_settings();
        }
    }

    fn list_body(&mut self, ui: &mut Ui) {
        let p = self.palette;
        if let Some(empty) = self.list_state().empty_state(&self.view.filter) {
            empty.show(ui, &p, true);
            return;
        }

        // **The clock is read once a frame, here, and carried down.** Every
        // caption says how long ago the chat last moved, and reading the clock
        // inside the row would be one syscall per visible row per frame — for
        // an answer that cannot change between two rows of the same list.
        let now = Local::now();
        let count = self.rows.len();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, count, |ui, range| {
                for i in range {
                    // Indexed rather than iterated: the row is read out of the
                    // cache and the body below needs `&mut self` to act on a
                    // click, so the borrow cannot be held across it.
                    let Some(row) = self.rows.get(i) else {
                        continue;
                    };
                    match row {
                        PaintedRow::Heading {
                            category,
                            total,
                            folded,
                        } => {
                            let (category, total, folded) = (*category, *total, *folded);
                            self.heading_row(ui, category, total, folded);
                        }
                        PaintedRow::Chat(chat) => {
                            let chat = chat.clone();
                            self.chat_row(ui, &chat, now);
                        }
                    }
                }
            });
    }

    /// A category heading.
    ///
    /// Folding is a way of looking at the list, not a second filter: a folded
    /// category's chats still count as visible, so tidying the view cannot
    /// silently change what All selects. See `crate::list`.
    fn heading_row(&mut self, ui: &mut Ui, category: Category, total: usize, folded: bool) {
        let p = self.palette;
        let (rect, response) = cell(ui, Sense::click());
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::ZERO, p.surface);
        inside(ui, rect, GUTTER, |ui| {
            // The disclosure marker is drawn from the text face rather than
            // assumed to exist as a glyph in some system font.
            ui.label(
                egui::RichText::new(if folded { "\u{25b8}" } else { "\u{25be}" })
                    .font(tgx_ui::fonts::mono(window::LABEL))
                    .color(p.muted),
            );
            ui.label(caps(category.label(), p.fg));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(figure(total.to_string(), &p));
            });
        });

        // **The click inverts the chevron it was painted beside**, rather than
        // re-deriving the state from `view.folded`. A heading whose painted
        // state and stored state ever part company — a search re-opening a
        // category is where that happened — would otherwise take one click to
        // undo a fold nobody could see and a second to do what was asked.
        if response.clicked() {
            if folded {
                self.view.folded.remove(&category);
            } else {
                self.view.folded.insert(category);
            }
            self.rebuild_rows();
            self.commit_settings();
        }
    }

    /// One chat. `now` is passed in rather than read here — see [`Self::list_body`].
    fn chat_row(&mut self, ui: &mut Ui, chat: &tgx_tg::client::ChatInfo, now: DateTime<Local>) {
        let p = self.palette;
        let ticked = self.selected.contains(&chat.id);
        let (rect, response) = cell(ui, Sense::click());
        // **A ticked row is a filled row, and a hovered one too.** The whole row
        // is the hit target, so a 12px box in the corner was the only thing
        // saying which rows were chosen — over four hundred of them, on the one
        // decision that governs what the export does.
        if ticked || response.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::ZERO, p.surface);
        }
        let indent = if self.view.grouped { 30.0 } else { GUTTER };
        inside(ui, rect, indent, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            tick_box(ui, ticked, true, &p);
            ui.add_space(12.0);
            // A forum is marked by a painted dot, never by a suffix on the
            // stored title — presentation in the string is what the filter then
            // searches.
            if chat.is_forum {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), Sense::hover());
                ui.painter()
                    .rect_filled(dot, egui::CornerRadius::ZERO, forum_dot(&p));
                ui.add_space(8.0);
            }

            // The count is the row's one number. Laid out from the right so a
            // long title cannot push it off the panel.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(figure(count_text(chat.message_count), &p));
                ui.add_space(space::STEP);
                // Title over caption, and the two are one block: no space
                // between them beyond the leading, because a gap there would
                // read as two rows rather than one chat with a subtitle.
                ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(text(&chat.title, &p)).truncate());
                    // The caption is the kind and the last activity. **The date
                    // is half its point**: the default sort is Recent activity,
                    // and without it the list is ordered on a unix second that
                    // appears nowhere on screen.
                    ui.add(egui::Label::new(caps(&list::caption(chat, now), p.muted)).truncate());
                });
            });
        });

        // Clicking anywhere on a row ticks it. Ticks are held by chat id, so
        // they survive re-sorting, regrouping and filtering.
        if response.clicked() && !self.selected.remove(&chat.id) {
            self.selected.insert(chat.id);
        }
    }

    /// All / None / Invert / Only forums, and the Count button.
    ///
    /// Nothing offers to do what it cannot: these act on the *visible* rows, so
    /// over an empty list every one is a no-op. A button that is enabled and
    /// does nothing teaches that the interface is unreliable.
    fn selection_row(&mut self, ui: &mut Ui, live: bool) {
        let p = self.palette;
        row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::STEP;
            for (i, label) in ["All", "None", "Invert", "Only forums"].iter().enumerate() {
                // **Flat, unlike the nav bar's.** Four boxes here would put more
                // border under the list than there is around the buttons that
                // matter, and these four only refine a selection the row above
                // already shows. The ink is unconditional: `action` hands the
                // disabled state to `add_enabled`, which greys the label and
                // refuses the click in the same call — the first pass chose a
                // colour here and a `Sense` beside it, and the two could
                // disagree.
                if action(ui, caps(label, p.fg), live).clicked() {
                    // Asked once: `visible()` sorts the whole account, and
                    // every one of these four needs the same answer.
                    let visible: Vec<(i64, bool)> =
                        self.visible().iter().map(|c| (c.id, c.is_forum)).collect();
                    match i {
                        0 => self.selected.extend(visible.iter().map(|(id, _)| *id)),
                        1 => {
                            for (id, _) in &visible {
                                self.selected.remove(id);
                            }
                        }
                        2 => {
                            for (id, _) in &visible {
                                if !self.selected.remove(id) {
                                    self.selected.insert(*id);
                                }
                            }
                        }
                        // **"Only forums" replaces, it does not add.** The
                        // label promises an exclusive selection, and adding to
                        // one meant clicking All and then this left every chat
                        // ticked with the footer still counting the lot.
                        // Assigning the set outright would do it, but the
                        // replacement is scoped to the visible rows like the
                        // other three, so a tick on a chat the filter is hiding
                        // is left where it is rather than silently dropped.
                        3 => {
                            for (id, is_forum) in &visible {
                                if *is_forum {
                                    self.selected.insert(*id);
                                } else {
                                    self.selected.remove(id);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            // **The label states the action; the price goes in the row under
            // it.** "Count messages (enables sorting by size)" named a side
            // effect rather than the action and said nothing about what it
            // costs — one request per chat, minutes on a large account, and a
            // rate-limit wait if Telegram decides so.
            let countable = !self.chats.is_empty() && !self.exporting;
            let label = if self.counting {
                "Stop counting"
            } else {
                "Count messages"
            };
            // A button, not small print: it costs a request per chat and can
            // take minutes, so it is a decision rather than a refinement.
            // Secondary, because the one red belongs to Start export.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, label, countable, false, &p).clicked() {
                    self.start_count();
                }
            });
        });
    }
}

/// One list row: full width, exactly [`ROW_HEIGHT`], allocated and sensed.
///
/// The height is fixed because `ScrollArea::show_rows` is told the same figure
/// and skips straight to the visible range on it — a row that measured itself
/// would put the list's scroll position and its contents out of step.
fn cell(ui: &mut Ui, sense: Sense) -> (egui::Rect, egui::Response) {
    ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), sense)
}

/// Lay a row's contents out inside the rect it was given, **vertically centred**.
///
/// `left` is the row's own indent; the right gutter is the panel's. The first
/// pass reached for `Ui::horizontal_centered` here, which allocates a strip one
/// `interact_size.y` tall — 30 points against this row's 46 — and centres within
/// *that*, leaving every row sitting eight points high in its own box. A layout
/// given the row rect centres in the row.
fn inside<R>(ui: &mut Ui, rect: egui::Rect, left: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = egui::Rect::from_min_max(
        egui::pos2(rect.left() + left, rect.top()),
        egui::pos2(rect.right() - GUTTER, rect.bottom()),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
        add,
    )
    .inner
}
