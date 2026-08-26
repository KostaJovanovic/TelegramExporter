//! Polls.
//!
//! Two details here are Desktop's own and both are load-bearing for a
//! line-by-line diff:
//!
//! * **The tally is appended only when someone voted for that answer.** An
//!   answer with no votes gets no `<span>` at all.
//! * **The total's class name contains a tab** — `class="total details&#x09;"`.
//!   That is a stray character in Desktop's own writer, escaped rather than
//!   emitted raw, and reproducing it is what makes the markup diff clean.

use crate::escape::esc;
use crate::tree::{a, Tree};
use serde_json::{Map, Value};

fn int_of(m: &Map<String, Value>, k: &str) -> i64 {
    match m.get(k) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn plural(n: i64, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

pub fn render(t: &mut Tree, poll: &Map<String, Value>) {
    t.open("div", &[a("class", "media_wrap clearfix")]);
    t.open("div", &[a("class", "media_poll")]);

    let question = poll.get("question").and_then(Value::as_str).unwrap_or("");
    t.leaf("div", &esc(question), &[a("class", "question bold")]);
    t.leaf("div", "Anonymous poll", &[a("class", "details")]);

    if let Some(Value::Array(answers)) = poll.get("answers") {
        for answer in answers {
            let Some(answer) = answer.as_object() else {
                continue;
            };
            let votes = int_of(answer, "voters");
            let text = answer.get("text").and_then(Value::as_str).unwrap_or("");
            let mut line = esc(&format!("- {text}"));
            if votes != 0 {
                let mut label = plural(votes, "vote");
                if answer.get("chosen").and_then(Value::as_bool) == Some(true) {
                    label.push_str(", chosen vote");
                }
                line.push_str(&format!(" <span class=\"details\">{}</span>", esc(&label)));
            }
            t.leaf("div", &line, &[a("class", "answer")]);
        }
    }

    let total = int_of(poll, "total_voters");
    t.leaf(
        "div",
        &esc(&plural(total, "vote")),
        // The tab is Desktop's own. Tree escapes it to &#x09;.
        &[a("class", "total details\t")],
    );
    t.close("div");
    t.close("div");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render_of(v: Value) -> String {
        let m = v.as_object().unwrap().clone();
        let mut t = Tree::new();
        render(&mut t, &m);
        t.into_string()
    }

    #[test]
    fn the_reference_poll_is_reproduced() {
        // Transcribed from ćaskanje/messages.html, message 728.
        let out = render_of(json!({
            "question": "klip",
            "answers": [
                { "text": "da", "voters": 0 },
                { "text": "ne", "voters": 0 }
            ],
            "total_voters": 8
        }));
        let expect = concat!(
            "<div class=\"media_wrap clearfix\">\n",
            "\n",
            " <div class=\"media_poll\">\n",
            "\n",
            "  <div class=\"question bold\">\n",
            "klip\n",
            "  </div>\n",
            "\n",
            "  <div class=\"details\">\n",
            "Anonymous poll\n",
            "  </div>\n",
            "\n",
            "  <div class=\"answer\">\n",
            "- da\n",
            "  </div>\n",
            "\n",
            "  <div class=\"answer\">\n",
            "- ne\n",
            "  </div>\n",
            "\n",
            "  <div class=\"total details&#x09;\">\n",
            "8 votes\n",
            "  </div>\n",
            // Two consecutive closing tags, so the blank-line rule applies
            // between them. Hand-transcribing this from the reference is
            // exactly where it is easy to drop them.
            "\n",
            " </div>\n",
            "\n",
            "</div>\n",
        );
        assert_eq!(out, expect);
    }

    #[test]
    fn the_stray_tab_in_the_class_name_survives_as_a_reference() {
        let out = render_of(json!({ "question": "q", "total_voters": 1 }));
        assert!(out.contains("class=\"total details&#x09;\""), "got:\n{out}");
        // And never raw, which would break the attribute.
        assert!(!out.contains("details\t"), "got:\n{out}");
    }

    #[test]
    fn an_answer_with_no_votes_gets_no_span() {
        let out = render_of(json!({
            "question": "q",
            "answers": [{ "text": "a", "voters": 0 }],
            "total_voters": 0
        }));
        assert!(out.contains("- a\n"), "got:\n{out}");
        assert!(!out.contains("<span"), "got:\n{out}");
    }

    #[test]
    fn a_voted_answer_gains_its_tally() {
        let out = render_of(json!({
            "question": "q",
            "answers": [{ "text": "a", "voters": 3 }],
            "total_voters": 3
        }));
        assert!(
            out.contains("<span class=\"details\">3 votes</span>"),
            "got:\n{out}"
        );
    }

    #[test]
    fn your_own_pick_is_marked() {
        let out = render_of(json!({
            "question": "q",
            "answers": [{ "text": "a", "voters": 1, "chosen": true }],
            "total_voters": 1
        }));
        assert!(out.contains("1 vote, chosen vote"), "got:\n{out}");
    }

    #[test]
    fn one_vote_is_singular() {
        assert!(render_of(json!({ "question": "q", "total_voters": 1 })).contains("1 vote\n"));
        assert!(render_of(json!({ "question": "q", "total_voters": 2 })).contains("2 votes\n"));
        assert!(render_of(json!({ "question": "q", "total_voters": 0 })).contains("0 votes\n"));
    }

    #[test]
    fn a_hostile_question_is_escaped() {
        let out = render_of(json!({
            "question": "<img src=x onerror=alert(1)>",
            "answers": [{ "text": "<script>", "voters": 1 }],
            "total_voters": 1
        }));
        assert!(!out.contains("<img"), "got:\n{out}");
        assert!(!out.contains("<script>"), "got:\n{out}");
    }
}
