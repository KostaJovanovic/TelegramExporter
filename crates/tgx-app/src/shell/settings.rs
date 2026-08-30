//! The settings panel — the site's readout tables, made editable.
//!
//! Five sections, in this order: Destination, Format, Media, Beyond Desktop,
//! Performance. No group boxes: the design has no such thing. A section is a
//! letterspaced heading over a hairline, and a setting is a muted label on the
//! left with its control on the right.
//!
//! **There is no "Chats at once".** This engine exports one chat at a time, and
//! a control that is enabled and does nothing teaches that the interface is
//! unreliable. The setting itself is gone too — keeping the field but offering
//! no control left a switch that did nothing, which is the same lie one layer
//! down. An old `settings.json` naming it still loads, because unknown keys are
//! dropped per field.

use super::Shell;
use eframe::egui::{self, Align, Layout, Ui};
use tgx_ui::components::{
    action, block, button, eyebrow, field, meta, row, rule, text, tick_box, title,
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

/// The extra requests. Each costs traffic, each is separately switchable, and
/// each degrades to nothing on failure — which is why they are one section
/// rather than scattered among the format options.
const EXTRAS: [(&str, &str); 6] = [
    ("full_reactions", "Full reaction lists"),
    ("chat_metadata", "Chat details"),
    ("invite_links", "Invite links"),
    ("refresh_polls", "Refresh poll results"),
    ("scheduled_messages", "Scheduled messages"),
    ("member_roster", "Member list"),
];

impl Shell {
    pub(super) fn settings_panel(&mut self, ui: &mut Ui) {
        let p = self.palette;
        ui.add_space(space::STEP);
        row(ui, |ui| ui.label(title("Settings", &p)));
        ui.add_space(space::TIGHT);
        rule(ui, &p);

        egui::ScrollArea::vertical()
            .id_salt("settings-body")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.destination_section(ui);
                self.format_section(ui);
                self.media_section(ui);
                self.beyond_desktop_section(ui);
                self.performance_section(ui);
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
        ui.add_space(space::TIGHT);
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
        ui.add_space(space::STEP);
    }

    fn format_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        section(ui, "Format", &p);
        for (label, on, change) in [
            (
                "HTML (browsable, like Telegram Desktop)",
                self.settings.export_html,
                0,
            ),
            (
                "JSON (machine-readable result.json)",
                self.settings.export_json,
                1,
            ),
            (
                "Split forum topics into separate folders",
                self.settings.split_topics,
                2,
            ),
        ] {
            if self.check(ui, label, on, true) {
                self.toggle_setting(|s| match change {
                    0 => s.export_html = !s.export_html,
                    1 => s.export_json = !s.export_json,
                    _ => s.split_topics = !s.split_topics,
                });
            }
        }
        let page = self.number_row(ui, "Messages per HTML page", true, |f| &mut f.page_size);
        if page {
            self.commit_settings();
        }
    }

    fn media_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let media_on = self.settings.download_media;
        section(ui, "Media", &p);
        if self.check(ui, "Download media", media_on, true) {
            self.toggle_setting(|s| s.download_media = !s.download_media);
        }
        for (key, label) in MEDIA_LABELS {
            let on = self.settings.media_kinds.iter().any(|k| k == key);
            if self.check(ui, label, on, media_on) {
                self.toggle_setting(move |s| {
                    if let Some(at) = s.media_kinds.iter().position(|k| k == key) {
                        s.media_kinds.remove(at);
                    } else {
                        s.media_kinds.push(key.to_string());
                    }
                });
            }
        }
        if self.number_row(ui, "Size limit (MB, 0 = no limit)", media_on, |f| {
            &mut f.size_limit
        }) {
            self.commit_settings();
        }
        hint(
            ui,
            "Larger files are recorded in the export but not downloaded.",
            &p,
        );
        if self.check(
            ui,
            "Save link-preview images",
            self.settings.link_previews,
            media_on,
        ) {
            self.toggle_setting(|s| s.link_previews = !s.link_previews);
        }
        hint(
            ui,
            "Telegram Desktop does not, and doing so shifts every later \
             photo_N in the folder.",
            &p,
        );
    }

    fn beyond_desktop_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        section(ui, "Beyond Desktop", &p);
        for (key, label) in EXTRAS {
            let on = extra(&self.settings, key);
            if self.check(ui, label, on, true) {
                self.toggle_setting(move |s| set_extra(s, key, !extra(s, key)));
            }
        }
        // Not in EXTRAS: that section is the requests, and this costs none —
        // it changes which of two names already in hand gets written.
        if self.check(
            ui,
            "Name contacts as they name themselves",
            self.settings.own_names,
            true,
        ) {
            self.toggle_setting(|s| s.own_names = !s.own_names);
        }
        hint(
            ui,
            "Telegram sends your address-book name for anyone you have saved, \
             and never sends theirs — so a contact is written as their \
             @username instead, or keeps your name if they have none.",
            &p,
        );
        if self.number_row(ui, "Member list cap (0 = no cap)", true, |f| {
            &mut f.member_limit
        }) {
            self.commit_settings();
        }
        hint(
            ui,
            "A public channel can have millions of members and Telegram stops \
             serving the listing long before that.",
            &p,
        );
    }

    fn performance_section(&mut self, ui: &mut Ui) {
        let p = self.palette;
        section(ui, "Performance", &p);
        if self.number_row(ui, "Parallel downloads", true, |f| &mut f.downloads) {
            self.commit_settings();
        }
        hint(ui, "1 to 16. 4-6 is a good balance.", &p);
    }

    /// One tick-box row. Returns whether it was clicked.
    ///
    /// **The box and its label are one control.** `Response::union` is how egui
    /// says that: one response, one click, whichever half the pointer landed on.
    /// The first pass or-ed two independently sensed widgets together, which
    /// left the gap between them dead and made the label choose its own disabled
    /// colour and its own `Sense` — two decisions that could disagree with the
    /// box beside them. `tick_box` still takes `enabled`, because it paints
    /// itself and a control that is off must not look like one that is
    /// unavailable.
    fn check(&self, ui: &mut Ui, label: &str, on: bool, enabled: bool) -> bool {
        let p = self.palette;
        let mut clicked = false;
        row(ui, |ui| {
            ui.spacing_mut().item_spacing.x = space::TIGHT;
            let box_hit = tick_box(ui, on, enabled, &p);
            clicked = box_hit
                .union(action(ui, text(label, &p), enabled))
                .clicked();
        });
        ui.add_space(space::STEP);
        clicked
    }

    /// A label with a narrow number field on the right. Returns whether the
    /// field just lost focus, which is when the value is a decision.
    fn number_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        enabled: bool,
        which: impl Fn(&mut crate::settings_form::SettingsForm) -> &mut String,
    ) -> bool {
        let p = self.palette;
        let mut committed = false;
        row(ui, |ui| {
            // Ink, like a tick-box label. A numeric setting and a switch are the
            // same kind of decision, and setting one of the two muted made half
            // the panel's rows look unavailable.
            ui.label(text(label, &p));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let entry = ui.add_enabled(
                    enabled,
                    field(which(&mut self.form), &p).desired_width(80.0),
                );
                committed = entry.lost_focus();
            });
        });
        ui.add_space(space::STEP);
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
        // five from `settings`, so anything typed and not yet blurred — `2000`
        // in Messages per page — would be replaced by the stored value with
        // nothing said.
        self.form.collect(&mut self.settings);
        self.settings.output_dir = dir.to_string_lossy().into_owned();
        self.form.output_dir = self.settings.output_dir.clone();
        self.commit_settings();
    }
}

/// A section heading over a hairline. No group boxes: the design has no such
/// thing.
/// A section heading. **Space above it, and no line under it.**
///
/// There were five rules down this panel, one per section, and they were the
/// loudest thing in a column of quiet grey labels — so the panel read as five
/// boxes rather than as a list of decisions. `BREAK` above the heading does the
/// same work: a gap four times the size of the one between two rows is not
/// ambiguous about where a group starts.
fn section(ui: &mut Ui, heading: &str, p: &Palette) {
    ui.add_space(space::BREAK);
    row(ui, |ui| ui.label(eyebrow(heading, p)));
    ui.add_space(space::TIGHT);
}

/// The sentence under a control that explains what it costs.
///
/// A block rather than a wrapping row with a leading space: a space allocated
/// inside a wrapping row is a space only the first line gets, so every hint here
/// used to start indented and then run flush against the panel edge on its
/// second line.
///
/// It hangs directly under the control it belongs to — `TIGHT` above, `STEP`
/// below — so that it reads as part of that row rather than as the opening of
/// the next one.
fn hint(ui: &mut Ui, sentence: &str, p: &Palette) {
    ui.add_space(space::TIGHT);
    block(ui, |ui| ui.label(meta(sentence, p)));
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
        for (key, _) in EXTRAS {
            let mut s = Settings::default();
            set_extra(&mut s, key, false);
            assert!(!extra(&s, key), "{key} did not clear");
            for (other, _) in EXTRAS {
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
}
