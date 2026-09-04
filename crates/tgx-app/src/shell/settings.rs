//! The settings panel — the site's readout tables, made editable.
//!
//! Six sections: Destination, Output, Beyond Desktop down the left; Media,
//! Performance, Appearance down the right. No group boxes: the design has no
//! such thing. A section is a letterspaced heading with space above it, and a
//! setting is one row — a control, its name, and a `[?]`.
//!
//! # What the last pass got wrong
//!
//! **The explanations were on the page.** Eight paragraphs of small print
//! interleaved with twenty-five switches, each hanging under the control it
//! belonged to. Every sentence was worth having and all of them together made
//! the panel unscannable: prose and controls alternating down a column, so
//! neither could be read as a list. They are behind [`help`] now — one `[?]` per
//! setting, in a column of its own down the right of each section, silent until
//! it is pressed. That is also how *every* setting comes to have an explanation
//! rather than the eight that happened to need defending.
//!
//! **Classic and Database were dimmed, not switched.** The mode was a pair of
//! tick boxes that unticked each other, and the losing mode's controls stayed on
//! screen greyed out — so the panel always showed both answers to a question
//! with one answer, and half of what was visible was unavailable by
//! construction. It is a [`segmented`] control now and the rows below it are
//! *replaced*: the panel shows the mode you are in and nothing about the one you
//! are not.
//!
//! **The rhythm did the opposite of grouping.** Every row paid `space::TIGHT` on
//! top of `item_spacing`, so a column of switches was as spread out as the gaps
//! between sections were meant to be. Rows sit on `item_spacing` alone now and
//! sections on `space::BREAK`, which is the four-to-one this file always claimed
//! and never had.
//!
//! **There is no "Chats at once".** This engine exports one chat at a time, and
//! a control that is enabled and does nothing teaches that the interface is
//! unreliable. The setting itself is gone too — keeping the field but offering
//! no control left a switch that did nothing, which is the same lie one layer
//! down. An old `settings.json` naming it still loads, because unknown keys are
//! dropped per field.

use super::{Shell, THEMES};
use eframe::egui::{self, Align, Layout, Ui};
use tgx_ui::components::{
    action, block, button, eyebrow, field, help, meta, row, segmented, text, tick_box,
};
use tgx_ui::tokens::{space, Palette};

/// The seven media categories Desktop offers, with the labels shown for each.
/// Keyed on `tgx_tg::config::MEDIA_KINDS`, which is what `plan.rs`
/// compares against — a label mismatch here would silently switch off a
/// category the settings file names.
const MEDIA_LABELS: [(&str, &str); 7] = [
    ("photos", "Photos"),
    ("video_files", "Videos"),
    ("voice_messages", "Voice messages"),
    ("video_messages", "Video messages"),
    ("stickers", "Stickers"),
    ("animations", "GIFs"),
    ("files", "Files"),
];

/// The extra requests, each with the sentence its `[?]` shows.
///
/// Each costs traffic, each is separately switchable, and each degrades to
/// nothing on failure — which is why they are one section rather than scattered
/// among the format options.
const EXTRAS: [(&str, &str, &str); 6] = [
    (
        "full_reactions",
        "Full reaction lists",
        "Ask Telegram who reacted, not only how many did. One extra request per \
         message that carries reactions, so it is the most expensive switch here \
         on a busy chat.",
    ),
    (
        "chat_metadata",
        "Chat details",
        "Fetch the chat's own description, photo and settings and write them \
         beside the messages. One request per chat.",
    ),
    (
        "invite_links",
        "Invite links",
        "Fetch the chat's invite links. Telegram only serves these to an account \
         that can administer the chat; anywhere else it degrades to nothing.",
    ),
    (
        "refresh_polls",
        "Refresh poll results",
        "Ask for each poll's results as they stand now, rather than the numbers \
         that were current when the message was sent.",
    ),
    (
        "scheduled_messages",
        "Scheduled messages",
        "Include messages queued to be sent later. Telegram Desktop's own export \
         does not, so a chat exported with this on will not match Desktop's byte \
         for byte.",
    ),
    (
        "member_roster",
        "Member list",
        "Fetch who is in the chat and write participants.json. See the cap below \
         — a roster that had to stop early says so in the run's log.",
    ),
];

/// What the mode really chooses, which is more than a file format.
const MODE_HELP: &str = "Classic reproduces Telegram Desktop's own export: a folder per chat, \
     browsable HTML and result.json, read from the beginning of the chat every \
     run. Database gives each chat its own <chat>.sqlite that is added to — a \
     re-run syncs what is new instead of exporting everything again, and it \
     keeps messages that were later deleted on Telegram, along with every \
     earlier version of an edited one. The two are exclusive on purpose: \
     Classic re-reads the whole chat anyway, so running both would cost exactly \
     what Classic costs and be incremental in name only.";

impl Shell {
    pub(super) fn settings_panel(&mut self, ui: &mut Ui) {
        // No `SETTINGS` heading: the view bar above already says so, and a view
        // does not need to introduce itself.
        //
        // **Two columns, because there is a window's width to use.** These were
        // one 400pt strip sharing a side panel with the queue and the log, so
        // twenty-five controls became a scrolling tube that showed four of them
        // and cut the fifth in half.
        //
        // Left is what the export *is* — where it goes, what shape it takes,
        // and what it asks Telegram for beyond what Desktop would. Right is the
        // media, which is eleven controls on its own, plus the two dials.
        // Balanced by height rather than by section count, because a column
        // that runs off the bottom while the other stops halfway is the
        // scrolling tube again in a wider frame.
        egui::ScrollArea::vertical()
            .id_salt("settings-body")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.columns(2, |cols| {
                    self.destination_section(&mut cols[0]);
                    self.output_section(&mut cols[0]);
                    self.beyond_desktop_section(&mut cols[0]);

                    self.media_section(&mut cols[1]);
                    self.performance_section(&mut cols[1]);
                    self.appearance_section(&mut cols[1]);
                });
                ui.add_space(space::BREAK);
            });
    }

    /// **The path shows its start, not its end, and the whole of it is the
    /// tooltip.** A field scrolled to its caret renders the default path as a
    /// drive letter sliced in half, which reads as a typo rather than as a
    /// path. Both are redone on every change — not once at construction —
    /// because Browse writes back afterwards.
    fn destination_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        section(ui, "Destination", &p);
        row(ui, |ui| {
            ui.label(text("Folder", &p));
            help(ui, "output_dir", DESTINATION_HELP, &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // A button, like the analyser's "Choose": the one way to get a
                // path that certainly exists should not be small print.
                if button(ui, "Browse", true, false, &p).clicked() {
                    self.browse_for_output_dir();
                }
                let entry = ui.add(
                    field(&mut self.form.output_dir, &p)
                        .hint_text("Where exports go")
                        .desired_width(ui.available_width()),
                );
                // Committing on losing focus rather than on every keystroke: a
                // half-typed path is not a decision, and writing settings.json
                // per character would save every prefix on the way.
                if entry.lost_focus() {
                    self.commit_settings();
                }
            });
        });
        let full = self.settings.output_dir.clone();
        row(ui, |ui| {
            ui.label(meta(
                format!(
                    "Writing to {}",
                    crate::settings_form::elided_start(&full, 44)
                ),
                &p,
            ))
            .on_hover_text(full);
        });
    }

    /// **Classic or Database — an exclusive choice, drawn as one, and it
    /// changes what is on the page.**
    ///
    /// The mode is a [`segmented`] strip and the rows under it are the chosen
    /// mode's alone. Two things that fixes: a tick box promises independence,
    /// so a pair that unticked each other was a control lying about its own
    /// kind; and a greyed row is a control offering something it will not do,
    /// which over half a section is a panel that looks broken.
    ///
    /// Split forum topics stays below the switch because it is the one setting
    /// both modes read — Classic makes a folder per topic, the database records
    /// which topic a message came from.
    fn output_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let db = self.settings.export_db;
        section(ui, "Output", &p);

        let mut chosen = None;
        row(ui, |ui| {
            chosen = segmented(ui, &["Classic", "Database"], usize::from(db), &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                help(ui, "export_db", MODE_HELP, &p);
            });
        });
        if let Some(i) = chosen {
            self.toggle_setting(move |s| s.export_db = i == 1);
        }
        ui.add_space(space::TIGHT);

        if db {
            if self.number_row(
                ui,
                "reread_window",
                "Messages to re-read",
                REREAD_HELP,
                true,
                |f| &mut f.reread_window,
            ) {
                self.commit_settings();
            }
            if self.check(
                ui,
                "reread_all",
                "Re-read the whole history",
                REREAD_ALL_HELP,
                self.settings.reread_all,
                true,
            ) {
                self.toggle_setting(|s| s.reread_all = !s.reread_all);
            }
        } else {
            for (key, label, why, on) in [
                (
                    "export_html",
                    "HTML pages",
                    HTML_HELP,
                    self.settings.export_html,
                ),
                (
                    "export_json",
                    "result.json",
                    JSON_HELP,
                    self.settings.export_json,
                ),
            ] {
                if self.check(ui, key, label, why, on, true) {
                    self.toggle_setting(move |s| match key {
                        "export_html" => s.export_html = !s.export_html,
                        _ => s.export_json = !s.export_json,
                    });
                }
            }
            if self.number_row(ui, "page_size", "Messages per page", PAGE_HELP, true, |f| {
                &mut f.page_size
            }) {
                self.commit_settings();
            }
        }

        ui.add_space(space::TIGHT);
        if self.check(
            ui,
            "split_topics",
            "Split forum topics",
            SPLIT_HELP,
            self.settings.split_topics,
            true,
        ) {
            self.toggle_setting(|s| s.split_topics = !s.split_topics);
        }
    }

    fn media_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let media_on = self.settings.download_media;
        section(ui, "Media", &p);
        if self.check(
            ui,
            "download_media",
            "Download media",
            MEDIA_HELP,
            media_on,
            true,
        ) {
            self.toggle_setting(|s| s.download_media = !s.download_media);
        }

        // **Seven switches across, not seven rows down.** They are one
        // question — which categories — and as a column they took as much of
        // the panel as everything else in this section put together. Wrapped,
        // so a narrow window turns them into three rows rather than clipping
        // the last two.
        let mut toggled: Option<&'static str> = None;
        // A block, not a row: `horizontal_wrapped` needs a vertical context to
        // wrap *into*, and nesting one inside `row`'s own horizontal is asking
        // a strip that cannot grow downwards to grow downwards.
        block(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(space::STEP, 6.0);
                for (key, label) in MEDIA_LABELS {
                    let on = self.settings.media_kinds.iter().any(|k| k == key);
                    // Nested, so the gap between a box and its own label is
                    // `TIGHT` while the gap between two categories is `STEP`.
                    // One `item_spacing` cannot say both, and at a single value
                    // the seven read either as fourteen controls or as one long
                    // sentence.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = space::TIGHT;
                        if tick(ui, label, on, media_on, &p) {
                            toggled = Some(key);
                        }
                    });
                }
            });
        });
        if let Some(key) = toggled {
            self.toggle_setting(move |s| {
                if let Some(at) = s.media_kinds.iter().position(|k| k == key) {
                    s.media_kinds.remove(at);
                } else {
                    s.media_kinds.push(key.to_string());
                }
            });
        }
        // One `[?]` for the group, on a row of its own: seven marks in a
        // wrapped strip would outnumber the controls they annotate.
        row(ui, |ui| {
            ui.label(eyebrow("Categories", &p));
            help(ui, "media_kinds", KINDS_HELP, &p);
        });

        ui.add_space(space::TIGHT);
        if self.number_row(
            ui,
            "size_limit_mb",
            "Size limit, MB",
            SIZE_HELP,
            media_on,
            |f| &mut f.size_limit,
        ) {
            self.commit_settings();
        }
        if self.check(
            ui,
            "link_previews",
            "Save link-preview images",
            PREVIEW_HELP,
            self.settings.link_previews,
            media_on,
        ) {
            self.toggle_setting(|s| s.link_previews = !s.link_previews);
        }
    }

    fn beyond_desktop_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        section(ui, "Beyond Desktop", &p);
        for (key, label, why) in EXTRAS {
            let on = extra(&self.settings, key);
            if self.check(ui, key, label, why, on, true) {
                self.toggle_setting(move |s| set_extra(s, key, !extra(s, key)));
            }
        }
        // Not in EXTRAS: that section is the requests, and this costs none —
        // it changes which of two names already in hand gets written.
        if self.check(
            ui,
            "own_names",
            "Contacts' own names",
            OWN_NAMES_HELP,
            self.settings.own_names,
            true,
        ) {
            self.toggle_setting(|s| s.own_names = !s.own_names);
        }
        if self.number_row(
            ui,
            "member_limit",
            "Member list cap",
            MEMBERS_HELP,
            true,
            |f| &mut f.member_limit,
        ) {
            self.commit_settings();
        }
    }

    fn performance_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        section(ui, "Performance", &p);
        if self.number_row(
            ui,
            "download_concurrency",
            "Parallel downloads",
            DOWNLOADS_HELP,
            true,
            |f| &mut f.downloads,
        ) {
            self.commit_settings();
        }
    }

    /// **The appearance is a setting, and this is where settings live.**
    ///
    /// It was a chip on the nav bar that named the appearance it switched *to*,
    /// which is a fine control and was in the wrong place: the bar's job is the
    /// three steps of an export, and a sixth kind of mark on it was part of what
    /// made the top of the window read as two menus. Here it is one more
    /// exclusive choice, drawn with the same strip as Classic and Database.
    fn appearance_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        section(ui, "Appearance", &p);
        // The stored name decides which cell is filled, so an edited settings
        // file naming neither shows Dark — which is what `Palette::named` will
        // actually have produced.
        let at = usize::from(self.settings.theme != "light");
        let mut chosen = None;
        row(ui, |ui| {
            chosen = segmented(ui, &["Light", "Dark"], at, &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                help(ui, "theme", THEME_HELP, &p);
            });
        });
        if let Some(i) = chosen {
            self.set_theme(THEMES[i]);
        }
    }

    /// One setting row: a tick box, its label, and the `[?]` that explains it.
    ///
    /// **The box and its label are one control.** `Response::union` is how egui
    /// says that: one response, one click, whichever half the pointer landed on.
    /// The first pass or-ed two independently sensed widgets together, which
    /// left the gap between them dead and made the label choose its own disabled
    /// colour and its own `Sense` — two decisions that could disagree with the
    /// box beside them. `tick_box` still takes `enabled`, because it paints
    /// itself and a control that is off must not look like one that is
    /// unavailable.
    ///
    /// The `[?]` is right-aligned, so every section has one narrow column of
    /// marks rather than a ragged edge that tracks the length of each label.
    /// It stays live when the setting is disabled: the reason a control is
    /// unavailable is exactly when someone wants to read about it.
    fn check(
        &self,
        ui: &mut Ui,
        key: &str,
        label: &str,
        why: &str,
        on: bool,
        enabled: bool,
    ) -> bool {
        let p = self.palette;
        let mut clicked = false;
        row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::TIGHT;
            clicked = tick(ui, label, on, enabled, &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                help(ui, key, why, &p);
            });
        });
        clicked
    }

    /// A label with a narrow number field on the right, and a `[?]`. Returns
    /// whether the field just lost focus, which is when the value is a decision.
    fn number_row(
        &mut self,
        ui: &mut Ui,
        key: &str,
        label: &str,
        why: &str,
        enabled: bool,
        which: impl Fn(&mut crate::settings_form::SettingsForm) -> &mut String,
    ) -> bool {
        let p = self.palette;
        let mut committed = false;
        row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::TIGHT;
            // Ink, like a tick-box label. A numeric setting and a switch are the
            // same kind of decision, and setting one of the two muted made half
            // the panel's rows look unavailable.
            ui.label(text(label, &p));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                help(ui, key, why, &p);
                let entry = ui.add_enabled(
                    enabled,
                    field(which(&mut self.form), &p).desired_width(NUMBER_W),
                );
                committed = entry.lost_focus();
            });
        });
        committed
    }

    /// Pick a destination folder.
    ///
    /// The path is free text and may be unwritable, disconnected or invalid, so
    /// a picker is not a nicety — it is the one way to get a path that exists.
    ///
    /// **Modal, and that is the change.** Under GPUI this was a future spawned
    /// onto the foreground executor with a write-back closure, because the
    /// prompt returned a `Task`. `rfd`'s blocking picker holds this thread until
    /// the user answers, which for a native dialog they are already looking at
    /// is what they expect — and it removes the window in which the fields
    /// could be edited underneath a pending write-back.
    fn browse_for_output_dir(&mut self) {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose export folder")
            .set_directory(&self.settings.output_dir)
            .pick_folder()
        else {
            return;
        };
        // **Read the other fields back first.** `commit_settings` rewrites all
        // of them from `settings`, so anything typed and not yet blurred —
        // `2000` in Messages per page — would be replaced by the stored value
        // with nothing said.
        self.form.collect(&mut self.settings);
        self.settings.output_dir = dir.to_string_lossy().into_owned();
        self.form.output_dir = self.settings.output_dir.clone();
        self.commit_settings();
    }
}

/// How wide a number field is. Every value in this panel is at most five
/// digits, and a field sized for a sentence invites one.
const NUMBER_W: f32 = 80.0;

/// A tick box and its label, as one control.
///
/// Free rather than a method because the media categories draw it inside a
/// wrapping strip where there is no room for a row, a gutter or a `[?]` — and
/// the alternative was a second assembly of a box and a label, which is exactly
/// the duplication [`Shell::check`]'s doc warns about.
fn tick(ui: &mut Ui, label: &str, on: bool, enabled: bool, p: &Palette) -> bool {
    tick_box(ui, on, enabled, p)
        .union(action(ui, text(label, p), enabled))
        .clicked()
}

/// A section heading. **Space above it, and no line under it.**
///
/// There were rules down this panel, one per section, and they were the loudest
/// thing in a column of quiet grey labels — so the panel read as a stack of
/// boxes rather than as a list of decisions. `BREAK` above the heading does the
/// same work: a gap four times the size of the one between two rows is not
/// ambiguous about where a group starts, and now that the rows themselves sit on
/// `item_spacing` alone the ratio is what it always claimed to be.
fn section(ui: &mut Ui, heading: &str, p: &Palette) {
    ui.add_space(space::BREAK);
    row(ui, |ui| ui.label(eyebrow(heading, p)));
    ui.add_space(space::TIGHT);
}

fn extra(s: &tgx_tg::config::Settings, key: &str) -> bool {
    match key {
        "full_reactions" => s.full_reactions,
        "chat_metadata" => s.chat_metadata,
        "invite_links" => s.invite_links,
        "refresh_polls" => s.refresh_polls,
        "scheduled_messages" => s.scheduled_messages,
        "member_roster" => s.member_roster,
        _ => false,
    }
}

fn set_extra(s: &mut tgx_tg::config::Settings, key: &str, on: bool) {
    match key {
        "full_reactions" => s.full_reactions = on,
        "chat_metadata" => s.chat_metadata = on,
        "invite_links" => s.invite_links = on,
        "refresh_polls" => s.refresh_polls = on,
        "scheduled_messages" => s.scheduled_messages = on,
        "member_roster" => s.member_roster = on,
        _ => {}
    }
}

// -- what every `[?]` says -------------------------------------------------
//
// Constants rather than literals at the call site, so a sentence can be read
// and revised as prose without a control's layout wrapped around it — and so
// `every_setting_offers_an_explanation` can find them.

const DESTINATION_HELP: &str =
    "Where exports go. Classic writes one folder per chat here; Database writes \
     one <chat>.sqlite per chat beside them. What is typed is used as-is and may \
     not exist, so Browse is the one way to be certain of the path.";

const HTML_HELP: &str = "Desktop's browsable messages1.html and the pages after it, reproduced \
     line for line. Switch both this and result.json off and a Classic run has \
     nothing to write, which it refuses rather than doing quietly.";

const JSON_HELP: &str =
    "Desktop's result.json: every message as structured data, in Desktop's own \
     key order, escaping and indentation. This is the file other tools read.";

const PAGE_HELP: &str = "How many messages go on one HTML page before the next one starts. \
     Desktop's own default is 1,000. Anything outside 50 to 20,000 is clamped, \
     and the field is rewritten so you can see what was stored.";

const REREAD_HELP: &str = "A sync always fetches what is new. This is how far back into what it \
     already holds it looks as well — far enough to notice an edit, a deletion, \
     a changed reaction count, or a file an earlier run could not fetch. 500 is \
     the default; 0 means new messages only. Up to 10,000.";

const REREAD_ALL_HELP: &str =
    "Read the whole chat again instead of the newest few hundred. Slow, and \
     the only way to catch an edit or a deletion older than the window above — \
     or to pick up media after raising the size limit.";

const SPLIT_HELP: &str =
    "A forum supergroup becomes one folder per topic instead of one folder for \
     all of them. This is the thing Telegram Desktop cannot do. In Database mode \
     there are no folders, so each message records the topic it came from \
     instead.";

const MEDIA_HELP: &str = "Fetch the photos, videos and files the messages point at. With this off \
     the export still records every filename, size and type — there is simply \
     nothing behind them.";

const KINDS_HELP: &str = "Which categories are fetched. A category switched off is still recorded \
     in the export exactly as Desktop records one it skipped, so the message \
     keeps its filename and size and only the bytes are missing.";

const SIZE_HELP: &str = "Files larger than this are recorded and not downloaded. 0 is no limit. \
     Raising it later only reaches messages a run actually re-reads — in \
     Database mode that is the window above, or the whole history.";

const PREVIEW_HELP: &str =
    "Telegram Desktop does not save these. Turning it on shifts every later \
     photo_N in the folder, so an export made with it on will not compare byte \
     for byte against one made with it off.";

const OWN_NAMES_HELP: &str =
    "Telegram sends your address-book name for anyone you have saved, and never \
     sends theirs. With this on a saved contact is written as their @username \
     instead — or keeps your name for them if they have no username.";

const MEMBERS_HELP: &str =
    "Stop the member list after this many. 0 is no cap. A public channel can \
     have millions of members and Telegram stops serving the listing long before \
     that, so a roster can end early either way — when it does, the run says so \
     rather than presenting a short list as a complete one.";

const DOWNLOADS_HELP: &str =
    "How many files are fetched at once. 1 to 16, and 4 to 6 is the balance. \
     Higher is not faster: Telegram starts rate-limiting the account instead, \
     and the run spends the time waiting.";

const THEME_HELP: &str =
    "Light or dark. Stored with the rest of the settings, so the window opens \
     the way you left it.";

#[cfg(test)]
mod tests {
    use super::*;
    use tgx_tg::config::{Settings, MEDIA_KINDS};

    #[test]
    fn every_media_category_the_planner_knows_has_a_label() {
        // A key here that `plan.rs` does not compare against is a tick box that
        // switches nothing off; a key there with no box is a category the user
        // cannot reach.
        let labelled: Vec<&str> = MEDIA_LABELS.iter().map(|(k, _)| *k).collect();
        for key in MEDIA_KINDS {
            assert!(labelled.contains(&key), "{key} has no tick box");
        }
        assert_eq!(labelled.len(), MEDIA_KINDS.len());
    }

    #[test]
    fn every_extra_reads_and_writes_its_own_field() {
        // A copy-paste in `set_extra` would silently point two rows at one
        // field, and the panel would look fine while switching the wrong thing.
        for (key, _, _) in EXTRAS {
            let mut s = Settings::default();
            set_extra(&mut s, key, false);
            assert!(!extra(&s, key), "{key} did not clear");
            for (other, _, _) in EXTRAS {
                if other != key {
                    assert!(extra(&s, other), "clearing {key} also cleared {other}");
                }
            }
            set_extra(&mut s, key, true);
            assert!(extra(&s, key), "{key} did not set");
        }
    }

    #[test]
    fn an_unknown_extra_is_ignored_rather_than_matching_the_first_arm() {
        let mut s = Settings::default();
        set_extra(&mut s, "nonsense", false);
        assert_eq!(s, Settings::default());
        assert!(!extra(&s, "nonsense"));
    }

    #[test]
    fn every_explanation_says_something_and_no_two_say_the_same_thing() {
        // The `[?]` is the whole reason the inline hints could go, so an empty
        // or duplicated one is a setting whose explanation was quietly lost —
        // and a duplicate is the shape a copy-paste takes here, the same defect
        // `every_extra_reads_and_writes_its_own_field` guards one row up.
        let mut all: Vec<&str> = EXTRAS.iter().map(|(_, _, why)| *why).collect();
        all.extend([
            MODE_HELP,
            DESTINATION_HELP,
            HTML_HELP,
            JSON_HELP,
            PAGE_HELP,
            REREAD_HELP,
            REREAD_ALL_HELP,
            SPLIT_HELP,
            MEDIA_HELP,
            KINDS_HELP,
            SIZE_HELP,
            PREVIEW_HELP,
            OWN_NAMES_HELP,
            MEMBERS_HELP,
            DOWNLOADS_HELP,
            THEME_HELP,
        ]);
        for why in &all {
            assert!(why.len() > 40, "too short to explain anything: {why}");
            assert!(why.trim_end().ends_with('.'), "not a sentence: {why}");
        }
        let mut unique: Vec<&str> = all.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), all.len(), "two settings share an explanation");
    }

    #[test]
    fn the_appearance_control_shows_the_palette_that_is_actually_on_screen() {
        // `Palette::named` treats anything that is not `light` as dark, so a
        // settings file naming something else must fill the Dark cell — a strip
        // showing Light over a dark window is worse than no strip.
        for (name, expect) in [("light", 0), ("dark", 1), ("chartreuse", 1)] {
            assert_eq!(usize::from(name != "light"), expect, "{name}");
            assert_eq!(
                Palette::named(THEMES[usize::from(name != "light")]),
                Palette::named(name)
            );
        }
    }
}
