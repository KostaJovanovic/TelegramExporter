//! The HTML leg: replay a reference `result.json` through our writer and diff
//! the pages against Desktop's own.
//!
//! Five values live only in Desktop's HTML and never reach its JSON, so they
//! are **lifted out of the reference and fed back in** rather than tested:
//!
//! | value | why it cannot be derived |
//! |---|---|
//! | `from_name` | Desktop's HTML writes `first + " " + last` *untrimmed*; the JSON trims |
//! | `forwarded_date` | the original timestamp, shown only in the forward header |
//! | `reactions_chosen` | which reaction is yours |
//! | `group` | album membership; Desktop's JSON has no `grouped_id` at all |
//! | preview `src` | a `" (1)"` collision suffix depends on folder history the JSON does not record |
//!
//! Everything else is under test. The preview's *pixel size* stays under test
//! even though its file name is lifted.

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tgx_html::preview::{preview_size, PREVIEW_BOX, STICKER_BOX};
use tgx_html::writer::HtmlWriter;

pub fn run(topics: &[PathBuf]) -> Result<u32> {
    let mut failures = 0u32;
    let mut total_lines = 0usize;
    let mut exact_topics = 0usize;

    for topic in topics {
        let name = topic.file_name().unwrap_or_default().to_string_lossy();
        println!("  {name}");
        match check(topic) {
            Ok(reports) => {
                let bad = reports.iter().filter(|r| r.differing > 0).count();
                for r in &reports {
                    total_lines += r.total;
                    if r.differing == 0 {
                        println!("    {}: identical ({} lines)", r.page, r.total);
                    } else {
                        println!(
                            "    {}: {} differing lines of {}",
                            r.page, r.differing, r.total
                        );
                        if let Some(d) = &r.detail {
                            println!("      {d}");
                        }
                    }
                }
                if bad == 0 && !reports.is_empty() {
                    exact_topics += 1;
                } else {
                    failures += 1;
                }
            }
            Err(e) => {
                failures += 1;
                println!("    {e:#}");
            }
        }
    }
    println!(
        "\n{exact_topics} of {} topics reproduced exactly ({total_lines} lines compared)",
        topics.len()
    );
    Ok(failures)
}

struct PageReport {
    page: String,
    differing: usize,
    total: usize,
    detail: Option<String>,
}

fn pages_of(folder: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("messages") && n.ends_with(".html"))
        })
        .collect();
    // messages.html, messages2.html, … messages10.html — shortest stem first.
    out.sort_by_key(|p| {
        let stem = p
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        (stem.len(), stem)
    });
    out
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid pattern")
}

/// Everything lifted out of one reference page, keyed by message id.
#[derive(Default, Clone)]
struct Lifted {
    from_name: Option<String>,
    forwarded_from_name: Option<String>,
    forwarded_date: Option<String>,
    fwd_header: bool,
    preview_src: Option<String>,
    reactions_chosen: Vec<bool>,
    /// `(is_forward, colour, initials)` for each 42px avatar, in order.
    letters: Vec<(bool, i64, String)>,
}

/// `{(display name, colour): initials}` harvested from the 20px reaction
/// avatars — the only place those are readable back for a peer whose entity the
/// JSON never describes.
type ByName = HashMap<(String, i64), String>;

fn lift(folder: &Path) -> Result<(HashMap<i64, Lifted>, ByName)> {
    static MSG: OnceLock<Regex> = OnceLock::new();
    static AVATAR20: OnceLock<Regex> = OnceLock::new();
    static AVATAR42: OnceLock<Regex> = OnceLock::new();
    static FROM_NAME: OnceLock<Regex> = OnceLock::new();
    static FWD: OnceLock<Regex> = OnceLock::new();
    static IMG: OnceLock<Regex> = OnceLock::new();
    static REACTION: OnceLock<Regex> = OnceLock::new();

    let msg = MSG.get_or_init(|| {
        re(r#"^<div class="message default clearfix(?: joined)?" id="message(\d+)">"#)
    });
    let avatar20 = AVATAR20.get_or_init(|| {
        re(r#"<div class="userpic userpic(\d+)" style="width: 20px[^>]*>\n\n\s*<div class="initials" style="line-height: 20px" title="([^"]*)">\n([^\n]*)\n"#)
    });
    let avatar42 = AVATAR42.get_or_init(|| {
        re(r#"<div class="pull_left( forwarded)? userpic_wrap">\n\n\s*<div class="userpic userpic(\d+)" style="width: 42px[^>]*>\n\n\s*<div class="initials" style="line-height: 42px">\n([^\n]*)\n"#)
    });
    let from_name = FROM_NAME.get_or_init(|| re(r#"<div class="from_name">\n([^\n]*)"#));
    let fwd = FWD.get_or_init(|| {
        re(r#"<div class="forwarded body">\n\n\s*<div class="from_name">\n([^\n<]*)<span class="date details" title="(\d\d)\.(\d\d)\.(\d{4}) (\d\d:\d\d:\d\d)"#)
    });
    let img = IMG
        .get_or_init(|| re(r#"<img class="(?:photo|sticker|animated|video_file)" src="([^"]+)""#));
    let reaction = REACTION.get_or_init(|| re(r#"<span class="reaction( active)?">"#));

    let mut out: HashMap<i64, Lifted> = HashMap::new();
    let mut by_name: ByName = HashMap::new();

    for page in pages_of(folder) {
        let text = std::fs::read_to_string(&page)
            .with_context(|| format!("reading {}", page.display()))?
            .replace("\r\n", "\n");

        for c in avatar20.captures_iter(&text) {
            let colour: i64 = c[1].parse().unwrap_or(1);
            let name = unescape(&c[2]);
            by_name
                .entry((name, colour))
                .or_insert_with(|| c[3].to_string());
        }

        // Split on the message marker without lookahead.
        const MARKER: &str = "<div class=\"message default clearfix";
        let mut starts: Vec<usize> = text.match_indices(MARKER).map(|(i, _)| i).collect();
        starts.push(text.len());
        for w in starts.windows(2) {
            let block = &text[w[0]..w[1]];
            let Some(head) = msg.captures(block) else {
                continue;
            };
            let id: i64 = head[1].parse().unwrap_or(0);

            let mut info = Lifted {
                fwd_header: block.contains(r#"class="pull_left forwarded userpic_wrap""#),
                ..Default::default()
            };

            // Capture whether each 42px avatar is the sender's or the
            // forward's: a joined forward draws only the second one, so
            // position alone maps the letters onto the wrong peer.
            for c in avatar42.captures_iter(block) {
                info.letters.push((
                    c.get(1).is_some(),
                    c[2].parse().unwrap_or(1),
                    c[3].to_string(),
                ));
            }

            if let Some(c) = from_name.captures(block) {
                // Desktop appends a `<span class="details">via bot</span>`; the
                // name is everything before it.
                let raw = &c[1];
                let name = raw.split("<span").next().unwrap_or(raw);
                info.from_name = Some(unescape(name));
            }

            if let Some(c) = fwd.captures(block) {
                info.forwarded_from_name = Some(unescape(&c[1]));
                info.forwarded_date = Some(format!("{}-{}-{}T{}", &c[4], &c[3], &c[2], &c[5]));
            }

            if let Some(c) = img.captures(block) {
                info.preview_src = Some(unescape(&c[1]));
            }

            info.reactions_chosen = reaction
                .captures_iter(block)
                .map(|c| c.get(1).is_some())
                .collect();

            out.insert(id, info);
        }
    }
    Ok((out, by_name))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

/// Rebuild the `<img>` Desktop chose for this message, and its CSS size.
fn add_preview(m: &Map<String, Value>, info: &mut Map<String, Value>) {
    let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("");
    let kind = s("media_type");
    let photo = s("photo");
    let file = s("file");
    let is_ph = |v: &str| v.starts_with("(File");

    let wh = |box_size: i64| -> (i64, i64) {
        let w = m.get("width").and_then(Value::as_i64).unwrap_or(0);
        let h = m.get("height").and_then(Value::as_i64).unwrap_or(0);
        preview_size(w, h, box_size).1
    };

    let thumb_of = |path: &str| -> String {
        match path.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}_thumb.{ext}"),
            None => format!("{path}_thumb"),
        }
    };

    if !photo.is_empty() && !is_ph(photo) {
        let (w, h) = wh(PREVIEW_BOX);
        *info = info.clone();
        info.insert(
            "preview".into(),
            json!({ "src": thumb_of(photo), "width": w, "height": h }),
        );
        return;
    }
    if file.is_empty() || is_ph(file) {
        return;
    }
    let lower = file.to_lowercase();
    if kind == "sticker" && (lower.ends_with(".webp") || lower.ends_with(".png")) {
        let (w, h) = wh(STICKER_BOX);
        info.insert(
            "preview".into(),
            json!({ "src": thumb_of(file), "width": w, "height": h }),
        );
    } else if matches!(kind, "video_file" | "video_message" | "animation") {
        if let Some(thumb) = m.get("thumbnail").and_then(Value::as_str) {
            let (w, h) = wh(PREVIEW_BOX);
            info.insert(
                "preview".into(),
                json!({ "src": thumb, "width": w, "height": h }),
            );
        }
    }
}

fn check(topic: &Path) -> Result<Vec<PageReport>> {
    let raw = std::fs::read_to_string(topic.join("result.json"))
        .with_context(|| format!("reading {}/result.json", topic.display()))?;
    let data: Value = serde_json::from_str(&raw)?;
    let title = data.get("name").and_then(Value::as_str).unwrap_or("");
    let messages = data
        .get("messages")
        .and_then(Value::as_array)
        .context("no messages array")?;

    let (lifted, by_name) = lift(topic)?;

    let out_dir = std::env::temp_dir().join(format!(
        "tgx-parity-{}-{}",
        std::process::id(),
        topic.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&out_dir);

    let mut writer = HtmlWriter::new(&out_dir, title, 1000);
    let mut last_forward: Option<(String, String, String)> = None;
    let mut album = 0i64;

    for msg in messages {
        let mut m = msg
            .as_object()
            .cloned()
            .context("a message is not an object")?;
        let id = m.get("id").and_then(Value::as_i64).unwrap_or(0);
        let info_src = lifted.get(&id).cloned().unwrap_or_default();
        let mut info = Map::new();

        // Album membership is a fourth HTML-only input. Desktop's result.json
        // has no grouped_id at all, so the only trace of an album is the very
        // thing it decides — whether a joined forward repeats its header. It is
        // fed in here rather than tested.
        if !info_src.fwd_header {
            info.insert("group".into(), json!(album.to_string()));
        } else {
            album += 1;
            info.insert("group".into(), json!(album.to_string()));
        }

        if let Some(n) = &info_src.from_name {
            info.insert("from_name".into(), json!(n));
        }
        if let Some(n) = &info_src.forwarded_from_name {
            info.insert("forwarded_from_name".into(), json!(n));
        }

        // Desktop prints a forward's original date only on the message that
        // *opens* a block, so a joined forward has none to lift. Inherit it
        // from the run this message continues — decided from the sender and
        // forward source alone, never from Desktop's own "joined" class, which
        // is the thing under test.
        let sv = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let source = (sv("from_id"), sv("forwarded_from_id"));
        if m.contains_key("forwarded_from") {
            if let Some(d) = &info_src.forwarded_date {
                info.insert("forwarded_date".into(), json!(d));
                last_forward = Some((source.0.clone(), source.1.clone(), d.clone()));
            } else if let Some(lf) = &last_forward {
                if lf.0 == source.0 && lf.1 == source.1 {
                    info.insert("forwarded_date".into(), json!(lf.2));
                }
            }
        } else {
            last_forward = None;
        }

        // The 42px avatars appear in order: the sender's, then the forward's.
        let mut table = Map::new();
        let mut colours = Map::new();
        // A hidden forward has no peer of its own; Desktop keys its avatar off
        // the message id, and so do we.
        let forward_peer = {
            let f = sv("forwarded_from_id");
            if f.is_empty() {
                id.to_string()
            } else {
                f
            }
        };
        for (is_forward, colour, drawn) in &info_src.letters {
            let peer = if *is_forward {
                forward_peer.clone()
            } else {
                sv("from_id")
            };
            if !peer.is_empty() {
                table.insert(peer.clone(), json!(drawn));
                colours.insert(peer, json!(colour - 1));
            }
        }
        if let Some(Value::Array(rs)) = m.get("reactions") {
            for r in rs {
                let Some(recent) = r.get("recent").and_then(Value::as_array) else {
                    continue;
                };
                for who in recent {
                    let peer = who.get("from_id").and_then(Value::as_str).unwrap_or("");
                    let name = who.get("from").and_then(Value::as_str).unwrap_or("");
                    if peer.is_empty() {
                        continue;
                    }
                    // Two people can share a display name, so match on the
                    // colour their id implies before falling back to the name.
                    let wanted = tgx_html::userpic::userpic_class(peer, None);
                    let mut chosen: Option<(i64, String)> = None;
                    for ((cand, colour), drawn) in &by_name {
                        if cand != name {
                            continue;
                        }
                        if chosen.is_none() || *colour == wanted {
                            chosen = Some((*colour, drawn.clone()));
                        }
                        if *colour == wanted {
                            break;
                        }
                    }
                    if let Some((colour, drawn)) = chosen {
                        table.insert(peer.to_string(), json!(drawn));
                        colours.insert(peer.to_string(), json!(colour - 1));
                    }
                }
            }
        }
        if !table.is_empty() {
            info.insert("initials".into(), Value::Object(table));
        }
        if !colours.is_empty() {
            info.insert("colours".into(), Value::Object(colours));
        }
        if !info_src.reactions_chosen.is_empty() {
            info.insert("reactions_chosen".into(), json!(info_src.reactions_chosen));
        }

        add_preview(&m, &mut info);
        if let Some(src) = &info_src.preview_src {
            if let Some(Value::Object(pv)) = info.get_mut("preview") {
                pv.insert("src".into(), json!(src));
            }
        }

        if !info.is_empty() {
            m.insert("_p".into(), Value::Object(info));
        }
        writer.add(&m)?;
    }
    writer.close()?;

    let theirs = pages_of(topic);
    let ours = pages_of(&out_dir);
    let mut reports = Vec::new();
    if theirs.len() != ours.len() {
        reports.push(PageReport {
            page: "page count".into(),
            differing: 1,
            total: theirs.len(),
            detail: Some(format!("desktop {} vs ours {}", theirs.len(), ours.len())),
        });
    }
    for (t, o) in theirs.iter().zip(ours.iter()) {
        let a = std::fs::read_to_string(t)?.replace("\r\n", "\n");
        let b = std::fs::read_to_string(o)?.replace("\r\n", "\n");
        reports.push(PageReport {
            page: t
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            differing: crate::differing_lines(&a, &b),
            total: a.lines().count(),
            detail: crate::first_difference(&a, &b),
        });
    }
    let _ = std::fs::remove_dir_all(&out_dir);
    Ok(reports)
}
