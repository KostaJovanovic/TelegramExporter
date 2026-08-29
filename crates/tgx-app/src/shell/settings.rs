//! The settings panel — the site's readout tables, made editable.
//!
//! It was read-only text, and not by accident: `settings_panel(&self)` took no
//! `Context`, so it *could not* be interactive whatever it painted. Six of
//! roughly twenty-four fields were printed as labels, fourteen had no UI at all
//! — including `media_kinds` and `download_media`, which `plan.rs` reads on
//! every media message — and the only editable fields anywhere in the app were
//! the three in the sign-in dialog.
//!
//! Four sections, in this order: Destination, Format,
//! Media, Performance, with the extra requests that have no counterpart in
//! Desktop's format gathered under Beyond Desktop. No group boxes: the design
//! has no such thing. A section is a letterspaced heading over a hairline, and
//! a setting is a muted label on the left with its control on the right.
//!
//! **There is no "Chats at once".** This engine
//! exports one chat at a time, and a control that is enabled and does nothing
//! teaches that the interface is unreliable. The setting itself is gone too —
//! keeping the field but offering no control left a switch that did nothing,
//! which is the same lie one layer down. An old `settings.json` naming it
//! still loads, because unknown keys are dropped per field.

use super::Shell;
use gpui::prelude::*;
use gpui::{div, px, Context, Div, SharedString};
use gpui_component::input::Input;
use gpui_component::scroll::Scrollbar;
use tgx_ui::components::{caps, eyebrow, rule, soft_rule, tick_box};
use tgx_ui::tokens::{type_scale, Palette};

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
    pub(super) fn settings_panel(&self, cx: &mut Context<Self>) -> Div {
        let p = &self.palette;
        let Some(form) = &self.form else {
            // No window, no input states: the headless shell paints nothing.
            return div();
        };

        let mut body = div()
            .id("settings-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .pb(px(12.0));

        // -- Destination ---------------------------------------------------
        // **The path shows its start, not its end, and the whole of it goes in
        // the tooltip.** A field scrolled to its caret renders the default path
        // as a drive letter sliced in half, which reads as a typo rather than
        // as a path. Both are redone on every change — not once at
        // construction — because Browse writes back afterwards.
        let full = self.settings.output_dir.clone();
        body = body
            .child(section("Destination", p))
            .child(
                row("Folder", p).child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.0))
                        .child(div().flex_1().min_w_0().child(Input::new(&form.output_dir)))
                        .child(
                            div()
                                .id("browse")
                                .flex_none()
                                .cursor_pointer()
                                .child(caps("Browse", type_scale::MICRO, p.accent))
                                .on_click(cx.listener(Self::browse_for_output_dir)),
                        ),
                ),
            )
            .child(
                div()
                    .id("output-dir-echo")
                    .px(px(16.0))
                    .pb(px(8.0))
                    .text_size(type_scale::MICRO)
                    .text_color(p.muted)
                    .child(SharedString::from(format!(
                        "Writing to {}",
                        crate::settings_form::elided_start(&full, 44)
                    )))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                    }),
            );

        // -- Format --------------------------------------------------------
        body = body
            .child(section("Format", p))
            .child(self.check(
                "format-html",
                "HTML (browsable, like Telegram Desktop)",
                self.settings.export_html,
                true,
                cx,
                |s| s.export_html = !s.export_html,
            ))
            .child(self.check(
                "format-json",
                "JSON (machine-readable result.json)",
                self.settings.export_json,
                true,
                cx,
                |s| s.export_json = !s.export_json,
            ))
            .child(self.check(
                "format-topics",
                "Split forum topics into separate folders",
                self.settings.split_topics,
                true,
                cx,
                |s| s.split_topics = !s.split_topics,
            ))
            .child(row("Messages per HTML page", p).child(narrow(&form.page_size)));

        // -- Media ---------------------------------------------------------
        let media_on = self.settings.download_media;
        body = body.child(section("Media", p)).child(self.check(
            "media-download",
            "Download media",
            media_on,
            true,
            cx,
            |s| s.download_media = !s.download_media,
        ));
        for (i, (key, label)) in MEDIA_LABELS.into_iter().enumerate() {
            let on = self.settings.media_kinds.iter().any(|k| k == key);
            body = body.child(
                self.check(("media-kind", i), label, on, media_on, cx, move |s| {
                    if let Some(at) = s.media_kinds.iter().position(|k| k == key) {
                        s.media_kinds.remove(at);
                    } else {
                        s.media_kinds.push(key.to_string());
                    }
                }),
            );
        }
        body = body
            .child(
                row("Size limit (MB, 0 = no limit)", p)
                    .child(narrow(&form.size_limit).disabled(!media_on)),
            )
            .child(hint(
                "Larger files are recorded in the export but not downloaded.",
                p,
            ))
            .child(self.check(
                "media-previews",
                "Save link-preview images",
                self.settings.link_previews,
                media_on,
                cx,
                |s| s.link_previews = !s.link_previews,
            ))
            .child(hint(
                "Telegram Desktop does not, and doing so shifts every later \
                 photo_N in the folder.",
                p,
            ));

        // -- Beyond Desktop ------------------------------------------------
        body = body.child(section("Beyond Desktop", p));
        for (i, (key, label)) in EXTRAS.into_iter().enumerate() {
            let on = extra(&self.settings, key);
            body = body.child(self.check(("extra", i), label, on, true, cx, move |s| {
                set_extra(s, key, !extra(s, key));
            }));
        }
        // Not in EXTRAS: that section is the requests, and this costs none —
        // it changes which of two names already in hand gets written.
        body = body
            .child(self.check(
                "own-names",
                "Name contacts as they name themselves",
                self.settings.own_names,
                true,
                cx,
                |s| s.own_names = !s.own_names,
            ))
            .child(hint(
                "Telegram sends your address-book name for anyone you have \
                 saved, and never sends theirs — so a contact is written as \
                 their @username instead, or keeps your name if they have none.",
                p,
            ));
        body = body.child(row("Member list cap (0 = no cap)", p).child(narrow(&form.member_limit)));
        body = body.child(hint(
            "A public channel can have millions of members and Telegram stops \
             serving the listing long before that.",
            p,
        ));

        // -- Performance ---------------------------------------------------
        body = body
            .child(section("Performance", p))
            .child(row("Parallel downloads", p).child(narrow(&form.downloads)))
            .child(hint("1 to 16. 4-6 is a good balance.", p));

        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex_none()
                    .px(px(16.0))
                    .py(px(12.0))
                    .child(eyebrow("Settings", p)),
            )
            .child(rule(p))
            .child(body)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(&self.settings_scroll)),
            )
    }

    /// One tick-box row.
    ///
    /// Disabled is passed through to [`tick_box`] rather than simply not
    /// binding a click: a control that is off and a control that is unavailable
    /// must not look identical.
    fn check(
        &self,
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        on: bool,
        enabled: bool,
        cx: &mut Context<Self>,
        change: impl Fn(&mut tgx_tg::config::Settings) + 'static,
    ) -> gpui::Stateful<Div> {
        let p = &self.palette;
        let mut cell = div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(8.0))
            .child(tick_box(on, enabled, p))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(type_scale::TINY)
                    .text_color(if enabled { p.fg } else { p.muted })
                    .child(SharedString::from(label)),
            );
        if enabled {
            cell = cell
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_setting(&change, window, cx);
                }));
        }
        cell
    }

    /// Pick a destination folder.
    ///
    /// The path is free text and may be unwritable, disconnected or invalid, so
    /// a picker is not a nicety — it is the one way to get a path that exists.
    /// The chosen path is written into the field and committed immediately;
    /// `SettingsForm::sync` then reruns, which is what keeps the field showing
    /// the *start* of a long path rather than its end.
    fn browse_for_output_dir(
        &mut self,
        _: &gpui::ClickEvent,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose export folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(chosen))) = paths.await else {
                return;
            };
            let Some(dir) = chosen.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // **Read the other fields back first.** The sync that follows
                // rewrites all five from `settings`, so anything typed and not
                // yet blurred — `2000` in Messages per page — would be replaced
                // by the stored value with nothing said. Every other path into
                // `settings` goes through `commit_settings` for this reason;
                // this one cannot, because the folder it is about comes from
                // the picker rather than from the field.
                if let Some(form) = this.form.take() {
                    form.collect(&mut this.settings, cx);
                    this.form = Some(form);
                }
                this.settings.output_dir = dir.to_string_lossy().into_owned();
                this.needs_field_sync = true;
                if let Err(e) = this.settings.save() {
                    this.journal.warn(format!("could not save settings: {e}"));
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// A section heading over a hairline. No group boxes: the design has no such
/// thing.
fn section(title: &'static str, p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .pt(px(18.0))
        .child(div().px(px(16.0)).pb(px(8.0)).child(eyebrow(title, p)))
        .child(soft_rule(p))
}

/// A label on the left, its control on the right.
fn row(label: &'static str, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .px(px(16.0))
        .py(px(8.0))
        .child(
            div()
                .flex_none()
                .text_size(type_scale::TINY)
                .text_color(p.muted)
                .child(SharedString::from(label)),
        )
}

/// The sentence under a control that explains what it costs.
fn hint(text: &'static str, p: &Palette) -> Div {
    div()
        .px(px(16.0))
        .pb(px(8.0))
        .text_size(type_scale::MICRO)
        .text_color(p.muted)
        .child(SharedString::from(text))
}

/// Keep a small control small inside a stretched value column.
fn narrow(state: &gpui::Entity<gpui_component::input::InputState>) -> Input {
    Input::new(state).w(px(96.0))
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
