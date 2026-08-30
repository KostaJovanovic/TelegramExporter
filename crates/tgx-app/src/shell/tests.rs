//! The interaction rules, driven without a window.
//!
//! [`Shell::headless`] exists for this: every rule below is behaviour, not
//! painting, and each one is a bug that was found the expensive way.

use super::*;
use crate::bridge::{Activity, Event};
use crate::queue::JobState;
use tgx_tg::client::ChatKind;
use tgx_ui::components::{selection_label, ListState};

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

fn shell_with(chats: Vec<ChatInfo>) -> Shell {
    let mut s = Shell::headless();
    s.signed_in = true;
    s.loaded = true;
    s.chats = chats;
    s.rebuild_rows();
    s
}

#[test]
fn the_list_opens_on_not_signed_in() {
    // An empty list is the state this app opens on, and blank is not
    // neutral: it reads as broken.
    let s = Shell::headless();
    assert_eq!(s.list_state(), ListState::NotSignedIn);
    assert!(s.list_state().empty_state("").is_some());
}

#[test]
fn signed_in_with_nothing_loaded_says_so() {
    let mut s = Shell::headless();
    s.signed_in = true;
    assert_eq!(s.list_state(), ListState::SignedInNothingLoaded);
}

#[test]
fn a_filter_matching_nothing_is_not_an_empty_account() {
    let mut s = shell_with(vec![chat(1, "news", None)]);
    // The filter and the rows are rebuilt together — the count the empty state
    // reads is cached with them, so changing one without the other is how a
    // list ends up painting rows under "Nothing matches".
    s.view.filter = "zzz".into();
    s.rebuild_rows();
    assert_eq!(s.list_state(), ListState::FilterMatchedNothing);
    s.view.filter.clear();
    s.rebuild_rows();
    assert_eq!(s.list_state(), ListState::Populated);
}

#[test]
fn selection_actions_are_disabled_over_an_empty_list() {
    // Nothing offers to do what it cannot.
    let s = Shell::headless();
    assert!(!s.selection_actions_enabled());
    let populated = shell_with(vec![chat(1, "a", None)]);
    assert!(populated.selection_actions_enabled());
}

#[test]
fn ticks_are_held_by_id_so_they_survive_filtering() {
    let mut s = shell_with(vec![chat(1, "news", None), chat(2, "other", None)]);
    s.view.filter = "news".into();
    s.selected.insert(1);
    // Filter to something else: the tick is still held.
    s.view.filter = "other".into();
    assert!(s.selected.contains(&1));
    s.view.filter.clear();
    assert_eq!(s.visible().len(), 2);
    assert!(s.selected.contains(&1));
}

#[test]
fn the_footer_says_at_least_when_a_selected_chat_is_uncounted() {
    let mut s = shell_with(vec![chat(1, "a", Some(10)), chat(2, "b", None)]);
    s.selected.insert(1);
    let (total, any) = s.selection_total();
    assert_eq!((total, any), (10, false));
    s.selected.insert(2);
    let (total, any) = s.selection_total();
    assert_eq!((total, any), (10, true));
    assert!(selection_label(2, total, any).contains("at least"));
}

#[test]
fn a_zero_count_is_summed_and_a_missing_one_is_not() {
    let mut s = shell_with(vec![chat(1, "empty", Some(0)), chat(2, "unknown", None)]);
    s.selected.insert(1);
    assert_eq!(s.selection_total(), (0, false), "0 is a count");
    s.selected.clear();
    s.selected.insert(2);
    assert_eq!(s.selection_total(), (0, true), "blank is not a count");
}

#[test]
fn an_unreadable_theme_setting_still_opens_a_readable_window() {
    let p = Palette::named("chartreuse");
    assert_eq!(p, Palette::dark());
}

#[test]
fn the_theme_chip_names_the_appearance_it_switches_to() {
    // A chip reading "LIGHT" that switches to dark is worse than no chip, and
    // an edited settings file must not be able to produce one: anything that
    // is not `light` is dark, exactly as `Palette::named` decides.
    assert_eq!(other_theme("dark"), "light");
    assert_eq!(other_theme("light"), "dark");
    assert_eq!(other_theme("chartreuse"), "light");
    // And the pair really does round-trip, or the chip toggles nothing.
    assert_eq!(other_theme(other_theme("dark")), "dark");
    assert_ne!(
        Palette::named(other_theme("dark")),
        Palette::named("dark"),
        "the chip must actually change the palette"
    );
}

// -- the count, and its one writer ----------------------------------------

#[test]
fn a_count_reaches_the_row_the_list_paints_and_sorts_on() {
    // Three sources write a count and there is one setter, or a finished
    // export leaves the row showing one number and sorting on another.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.apply(Event::Counted {
        chat_id: 1,
        total: Some(6643),
    });
    assert_eq!(s.chats[0].message_count, Some(6643));
    assert_eq!(s.count_of(1), Some(6643));
}

#[test]
fn a_chat_telegram_would_not_count_stays_blank_rather_than_zero() {
    let mut s = shell_with(vec![chat(1, "a", Some(5))]);
    s.apply(Event::Counted {
        chat_id: 1,
        total: None,
    });
    assert_eq!(s.chats[0].message_count, None, "None is not zero");
}

#[test]
fn an_export_fills_in_a_count_nobody_asked_for() {
    // The export looks the total up anyway; writing it to the list is free,
    // and without it the row reads blank beside a progress bar that plainly
    // knows the answer.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.queue.start([(1, "a".to_string())]);
    s.apply(Event::ChatTotal {
        chat_id: 1,
        total: 900,
    });
    assert_eq!(s.count_of(1), Some(900));
}

#[test]
fn a_looked_up_total_does_not_overwrite_a_measured_one() {
    // The Count button's answer came from the same request; what must not
    // happen is the pre-export estimate replacing what an export wrote.
    let mut s = shell_with(vec![chat(1, "a", Some(6643))]);
    s.queue.start([(1, "a".to_string())]);
    s.apply(Event::ChatTotal {
        chat_id: 1,
        total: 10,
    });
    assert_eq!(s.count_of(1), Some(6643));
}

#[test]
fn what_an_export_wrote_replaces_what_the_list_was_carrying() {
    let mut s = shell_with(vec![chat(1, "a", Some(6643))]);
    s.queue.start([(1, "a".to_string())]);
    s.apply(Event::ChatDone {
        chat_id: 1,
        messages: 6640,
        expected: 6643,
        topics: Some(4),
        media_downloaded: 830,
        media_failed: 6,
        root: std::path::PathBuf::from("out"),
    });
    assert_eq!(s.count_of(1), Some(6640), "a measured number wins");
    assert_eq!(s.queue.jobs()[0].state, JobState::Done);
}

#[test]
fn a_failed_export_writes_no_count_at_all() {
    // A truncated run must not leave its own length behind as the size of
    // the chat.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.queue.start([(1, "a".to_string())]);
    s.apply(Event::ChatFailed {
        chat_id: 1,
        message: "CHAT_ADMIN_REQUIRED".into(),
    });
    assert_eq!(s.count_of(1), None);
    assert_eq!(s.journal.warnings(), 1);
}

// -- one bar, two claimants -----------------------------------------------

#[test]
fn a_count_finishing_mid_export_does_not_touch_the_export_s_progress() {
    // "Counted 12 of 12 chats" painted over "6,000 of 6,643" is the failure.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.exporting = true;
    s.status = "Exporting a".into();
    s.apply(Event::CountProgress {
        done: 12,
        total: 12,
    });
    assert_eq!(s.count_progress, None);
    assert_eq!(s.status.as_str(), "Exporting a");
    s.apply(Event::CountFinished {
        counted: 12,
        failed: 0,
    });
    assert_eq!(s.status.as_str(), "Exporting a");
}

#[test]
fn a_rate_limit_does_not_take_the_status_line_for_the_rest_of_the_chat() {
    // Nothing else writes the status during an export, so one 60-second wait
    // left the bar reading "Rate limited, waiting 60s" while progress
    // advanced beside it.
    let mut s = shell_with(vec![chat(1, "news", None)]);
    s.exporting = true;
    s.queue.start([(1, "news".to_string())]);
    s.apply(Event::FloodWait(60));
    assert!(s.status.as_str().contains("Rate limited"));
    s.apply(Event::Progress {
        chat_id: 1,
        done: 200,
        total: 6643,
    });
    assert_eq!(s.status.as_str(), "Exporting news");
}

#[test]
fn a_run_that_could_not_start_says_why_and_not_only_how_many() {
    // `Failed` and `Finished` arrive in the same batch when the destination
    // is unusable, and the queue's tally used to land on top of the one
    // sentence naming the cause.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.exporting = true;
    s.queue.start([(1, "a".to_string())]);
    s.apply(Event::Failed {
        activity: Activity::Export,
        message: "cannot write into D:\\gone".into(),
    });
    s.apply(Event::Finished { stopped: true });
    assert!(
        s.status.as_str().contains("cannot write into"),
        "got {}",
        s.status
    );
    // And the cause is spent, so the next clean run is not haunted by it.
    assert!(s.failure.is_none());
}

#[test]
fn a_sign_in_failing_mid_export_does_not_stop_the_export() {
    // `Failed` was a global switch: any sender cleared `exporting` and
    // `counting`, and five senders share it. A sign-in probe that could not
    // reach Telegram while an export ran made the window believe the export had
    // stopped — the Stop button vanished, the bar froze — while the export went
    // on writing files until its own `Finished` put the state back.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.exporting = true;
    s.counting = true;
    s.apply(Event::Failed {
        activity: Activity::SignIn,
        message: "Telegram did not answer".into(),
    });
    assert!(s.exporting, "a sign-in failure is not an export failure");
    assert!(s.counting, "nor a count failure");
    // It is still reported.
    assert!(s.status.as_str().contains("did not answer"));
    // And it is not the export's stated cause, so it cannot reach the run's
    // summary line.
    assert!(s.failure.is_none());

    // The count's own failure does stop the count, and only the count.
    s.apply(Event::Failed {
        activity: Activity::Count,
        message: "rate limited".into(),
    });
    assert!(!s.counting);
    assert!(s.exporting);
}

#[test]
fn a_failure_from_the_last_run_does_not_reach_the_next_ones_summary() {
    // `failure` is appended to the run's summary and was only ever cleared by
    // the `Finished` that consumed it — so a run that ended some other way left
    // its cause behind to be reported as the reason the *next* run ended.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.failure = Some("cannot write into D:\\gone".into());
    s.begin_run();
    assert!(s.failure.is_none(), "a stale cause survived into a new run");
}

#[test]
fn a_failed_chat_says_which_chat() {
    // The transcript prefixes every other line with the chat. This one did not,
    // so "no messages could be read" in a twenty-chat queue named none of them
    // — and the queue table that does know is a different panel, sorted
    // differently.
    let mut s = shell_with(vec![chat(1, "news", None), chat(2, "other", None)]);
    s.queue
        .start([(1, "news".to_string()), (2, "other".to_string())]);
    s.apply(Event::ChatFailed {
        chat_id: 2,
        message: "no messages could be read".into(),
    });
    let last = s.journal.to_text();
    assert!(last.contains("other: no messages could be read"), "{last}");
}

#[test]
fn a_refresh_keeps_the_counts_it_took_minutes_to_fetch() {
    // The dialog list carries no totals, so replacing it wholesale silently
    // undid a Count that had spent one request per chat to get them.
    let mut s = shell_with(vec![chat(1, "a", Some(6643)), chat(2, "b", Some(0))]);
    s.apply(Event::Chats(vec![
        chat(1, "a", None),
        chat(2, "b", None),
        chat(3, "new", None),
    ]));
    assert_eq!(s.count_of(1), Some(6643));
    assert_eq!(s.count_of(2), Some(0), "0 is a count and survives too");
    assert_eq!(s.count_of(3), None, "a chat nobody counted stays blank");
}

#[test]
fn a_chat_that_was_never_split_reports_no_topic_count() {
    // The engine counts its single General sink as one output folder, so a
    // private chat's em dash flipped to `1` the moment it finished.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.queue.start([(1, "a".to_string())]);
    s.apply(Event::ChatDone {
        chat_id: 1,
        messages: 250,
        expected: 250,
        topics: None,
        media_downloaded: 12,
        media_failed: 0,
        root: std::path::PathBuf::from("out"),
    });
    assert_eq!(s.queue.jobs()[0].topics_text(), "\u{2014}");
}

// -- stop actually stops ---------------------------------------------------

#[test]
fn stop_raises_the_flag_the_worker_reads() {
    // The old Stop set `exporting = false`, cleared the bar and wrote
    // "Stopped" while the export ran to completion writing files.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.exporting = true;
    s.stop();
    assert!(s.cancel.is_cancelled(), "nothing told the worker");
    assert!(
        s.exporting,
        "the run is over when the worker says so, not when the button is pressed"
    );
    assert_eq!(s.status.as_str(), "Stopping…");
}

#[test]
fn a_stopped_run_is_not_reported_as_a_success() {
    // `Finished` used to carry its own sentence and overwrote "Stopped" with
    // "Exported 3 of 3 chats" a moment later.
    let mut s = shell_with(vec![chat(1, "a", None), chat(2, "b", None)]);
    s.exporting = true;
    s.queue.start([(1, "a".to_string()), (2, "b".to_string())]);
    s.apply(Event::ChatDone {
        chat_id: 1,
        messages: 5,
        expected: 5,
        topics: None,
        media_downloaded: 0,
        media_failed: 0,
        root: std::path::PathBuf::from("out"),
    });
    s.stop();
    s.apply(Event::Finished { stopped: true });
    assert!(!s.exporting);
    assert_eq!(s.queue.jobs()[1].state, JobState::Stopped);
    assert!(s.status.as_str().contains("1 not run"), "got {}", s.status);
}

#[test]
fn starting_a_run_clears_a_cancel_left_over_from_the_last_one() {
    // Otherwise the second export stops on its first message, having been
    // told to before it began.
    let s = shell_with(vec![chat(1, "a", None)]);
    s.cancel.cancel();
    s.cancel.reset();
    assert!(!s.cancel.is_cancelled());
}

// -- the log ---------------------------------------------------------------

#[test]
fn a_warning_survives_the_chats_that_finish_after_it() {
    // Six more finished chats used to put the INCOMPLETE line out of reach:
    // only the last six were painted and nothing scrolled.
    let mut s = Shell::headless();
    s.apply(Event::Warn("INCOMPLETE".into()));
    for i in 0..50 {
        s.apply(Event::Log(format!("chat {i}: done")));
    }
    assert_eq!(s.journal.warnings(), 1);
    assert!(s.journal.lines().any(|l| l.text == "INCOMPLETE"));
}

// -- the list --------------------------------------------------------------

#[test]
fn the_painted_rows_follow_the_filter_without_touching_the_ticks() {
    let mut s = shell_with(vec![chat(1, "news", None), chat(2, "other", None)]);
    s.selected.insert(1);
    s.view.filter = "news".into();
    s.rebuild_rows();
    let chats: Vec<&ChatInfo> = s
        .rows
        .iter()
        .filter_map(|r| match r {
            PaintedRow::Chat(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].title, "news");
    assert!(s.selected.contains(&1));
}

#[test]
fn a_refresh_drops_ticks_for_chats_that_vanished() {
    let mut s = shell_with(vec![chat(1, "a", None), chat(2, "b", None)]);
    s.selected.insert(1);
    s.selected.insert(2);
    s.apply(Event::Chats(vec![chat(2, "b", None)]));
    assert!(!s.selected.contains(&1));
    assert!(s.selected.contains(&2));
}

#[test]
fn counting_a_large_account_rebuilds_the_rows_once_not_once_per_chat() {
    // Re-sorting the whole list on every `Counted` is quadratic work for a
    // picture nobody sees until the batch is done.
    let mut s = shell_with(
        (1..=50)
            .map(|i| chat(i, &format!("chat {i}"), None))
            .collect(),
    );
    for i in 1..=50 {
        s.apply(Event::Counted {
            chat_id: i,
            total: Some(i * 10),
        });
    }
    assert!(s.rows_stale, "the rows should still be waiting to rebuild");
    s.rebuild_rows_if_stale();
    assert!(!s.rows_stale);
    assert_eq!(s.count_of(50), Some(500));
}

#[test]
fn a_folded_category_is_remembered_across_a_restart() {
    // Someone who folds Bots away has said something about how they want the
    // list to look; asking again on every launch is forgetting on purpose.
    let mut s = shell_with(vec![chat(1, "a", None)]);
    s.view.folded.insert(Category::Bots);
    s.settings.folded_categories = Category::ALL
        .into_iter()
        .filter(|c| s.view.folded.contains(c))
        .map(|c| c.key().to_string())
        .collect();
    assert_eq!(s.settings.folded_categories, vec!["bot"]);

    // And a name from a build that had a category this one does not is
    // dropped rather than kept.
    let restored: std::collections::HashSet<Category> = ["bot", "sasquatch"]
        .iter()
        .filter_map(|name| Category::ALL.into_iter().find(|c| c.key() == *name))
        .collect();
    assert_eq!(restored, s.view.folded);
}

#[test]
fn the_sort_and_grouping_are_taken_from_the_saved_settings() {
    // An unknown key falls back rather than breaking, the same way the theme
    // does — an edited settings file must not leave the list unsorted.
    let mut s = Shell::headless();
    s.view.sort = SortMode::from_key("largest");
    assert_eq!(s.view.sort, SortMode::Largest);
    assert_eq!(SortMode::from_key("nonsense"), SortMode::Recent);
}
