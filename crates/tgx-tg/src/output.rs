//! One output folder: the streamed `result.json` and the paginated HTML.
//!
//! **Both outputs come from one map.** `add` takes the payload that goes into
//! the JSON, strips the presentation-only `_p` key, and hands the *whole* map
//! to the HTML writer — so the two cannot drift, and the writer stays testable
//! with no Telegram connection. That property is what lets the parity harness
//! replay a real export through it.
//!
//! **Closing drains.** The JSON is streamed, so a run that is abandoned without
//! `close()` leaves a file that is not merely truncated but **zero bytes** —
//! the writes are still buffered. Every path that can end an export has to come
//! through here.

use crate::config::Settings;
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tgx_format::json;
use tgx_html::writer::HtmlWriter;

pub struct Output {
    pub root: PathBuf,
    json: Option<BufWriter<File>>,
    html: Option<HtmlWriter>,
    count: usize,
    /// Type names the encoder could not map, reported at the end of the chat.
    pub degraded: json::Degraded,
}

impl Output {
    pub fn new(
        root: &Path,
        name: &str,
        kind: &str,
        chat_id: i64,
        settings: &Settings,
        back_href: Option<String>,
        extra_head: Option<Map<String, Value>>,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;

        let html = if settings.export_html {
            Some(HtmlWriter::new(root, name, settings.page_size).with_back_href(back_href))
        } else {
            None
        };

        let json_writer = if settings.export_json {
            let file = File::create(root.join("result.json"))?;
            let mut w = BufWriter::new(file);
            // Desktop's three header keys stay first and keep their order; a
            // topic's own metadata follows them.
            let mut header = Map::new();
            header.insert("name".into(), Value::String(name.to_string()));
            header.insert("type".into(), Value::String(kind.to_string()));
            header.insert("id".into(), Value::Number(chat_id.into()));
            if let Some(extra) = extra_head {
                for (k, v) in extra {
                    header.insert(k, v);
                }
            }
            w.write_all(json::header_prelude(&header).as_bytes())?;
            Some(w)
        } else {
            None
        };

        Ok(Self {
            root: root.to_path_buf(),
            json: json_writer,
            html,
            count: 0,
            degraded: json::Degraded::default(),
        })
    }

    pub fn add(&mut self, payload: &Map<String, Value>) -> std::io::Result<()> {
        if let Some(w) = self.json.as_mut() {
            // `_p` carries what Desktop shows in HTML but keeps out of JSON.
            let body: Map<String, Value> = payload
                .iter()
                .filter(|(k, _)| k.as_str() != "_p")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if self.count > 0 {
                w.write_all(b",\n")?;
            }
            w.write_all(json::message_block(&body).as_bytes())?;
        }
        if let Some(h) = self.html.as_mut() {
            h.add(payload)?;
        }
        self.count += 1;
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Finish both outputs. Safe to call twice.
    pub fn close(&mut self) -> std::io::Result<()> {
        if let Some(mut w) = self.json.take() {
            w.write_all(json::footer().as_bytes())?;
            w.flush()?;
        }
        if let Some(mut h) = self.html.take() {
            h.close()?;
        }
        Ok(())
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // A last line of defence, not the mechanism. An abandoned Output still
        // produces a valid file rather than a zero-byte one — but the error is
        // unreportable here, so every real path calls `close()` explicitly.
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json as j;

    fn settings() -> Settings {
        Settings {
            export_html: false,
            ..Settings::default()
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tgx-out-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_closed_output_is_valid_json() {
        let dir = tmp("valid");
        let mut o =
            Output::new(&dir, "t", "public_supergroup", 123, &settings(), None, None).unwrap();
        o.add(j!({ "id": 1, "type": "message" }).as_object().unwrap())
            .unwrap();
        o.add(j!({ "id": 2, "type": "message" }).as_object().unwrap())
            .unwrap();
        o.close().unwrap();

        let raw = std::fs::read_to_string(dir.join("result.json")).unwrap();
        let parsed: Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(parsed["id"], 123);
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 2);
        // And it ends on the brace, with no trailing newline.
        assert!(raw.ends_with('}'), "got {:?}", &raw[raw.len() - 4..]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_chat_still_produces_valid_json() {
        let dir = tmp("empty");
        let mut o = Output::new(&dir, "t", "private", 1, &settings(), None, None).unwrap();
        o.close().unwrap();
        let raw = std::fs::read_to_string(dir.join("result.json")).unwrap();
        let parsed: Value = serde_json::from_str(&raw).expect("valid JSON");
        assert!(parsed["messages"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dropping_without_closing_still_writes_a_valid_file() {
        // The zero-byte failure: the writes are buffered, so abandoning the
        // output loses everything rather than truncating it.
        let dir = tmp("dropped");
        {
            let mut o = Output::new(&dir, "t", "private", 1, &settings(), None, None).unwrap();
            o.add(j!({ "id": 1 }).as_object().unwrap()).unwrap();
            // no close()
        }
        let raw = std::fs::read_to_string(dir.join("result.json")).unwrap();
        assert!(!raw.is_empty(), "zero-byte result.json");
        serde_json::from_str::<Value>(&raw).expect("valid JSON");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn closing_twice_is_harmless() {
        let dir = tmp("twice");
        let mut o = Output::new(&dir, "t", "private", 1, &settings(), None, None).unwrap();
        o.close().unwrap();
        o.close().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_presentation_key_never_reaches_the_json() {
        let dir = tmp("strip-p");
        let mut o = Output::new(&dir, "t", "private", 1, &settings(), None, None).unwrap();
        o.add(
            j!({ "id": 1, "_p": { "from_name": "Ivana " } })
                .as_object()
                .unwrap(),
        )
        .unwrap();
        o.close().unwrap();
        let raw = std::fs::read_to_string(dir.join("result.json")).unwrap();
        assert!(!raw.contains("_p"), "got {raw}");
        assert!(!raw.contains("from_name"), "got {raw}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn topic_metadata_follows_desktops_three_header_keys() {
        let dir = tmp("head");
        let mut extra = Map::new();
        extra.insert("topic_id".into(), j!(42));
        let mut o = Output::new(
            &dir,
            "Backend",
            "public_supergroup",
            123,
            &settings(),
            None,
            Some(extra),
        )
        .unwrap();
        o.close().unwrap();
        let raw = std::fs::read_to_string(dir.join("result.json")).unwrap();
        let name_at = raw.find("\"name\"").unwrap();
        let type_at = raw.find("\"type\"").unwrap();
        let id_at = raw.find("\"id\"").unwrap();
        let topic_at = raw.find("\"topic_id\"").unwrap();
        assert!(name_at < type_at && type_at < id_at && id_at < topic_at);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn both_outputs_come_from_one_map() {
        let dir = tmp("both");
        let s = Settings {
            export_html: true,
            export_json: true,
            ..Settings::default()
        };
        let mut o = Output::new(&dir, "t", "private", 1, &s, None, None).unwrap();
        o.add(
            j!({ "id": 1, "type": "message", "date": "2025-12-18T16:00:00",
                 "from": "A", "from_id": "user1", "text": "hello" })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        o.close().unwrap();
        let json_raw = std::fs::read_to_string(dir.join("result.json")).unwrap();
        let html_raw = std::fs::read_to_string(dir.join("messages.html")).unwrap();
        assert!(json_raw.contains("hello"));
        assert!(html_raw.contains("hello"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
