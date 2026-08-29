//! One message, converted and enriched, as it goes into the output.
//!
//! The map built here is handed whole to both writers: `output.rs` strips the
//! presentation-only `_p` key for `result.json` and passes the rest to the
//! HTML. That is what stops the two outputs drifting.

use super::*;

impl<'a> ChatExporter<'a> {
    pub(super) fn payload(
        &mut self,
        msg: &grammers_client::message::Message,
        extras: &MessageExtras,
        names: &mut MediaNames,
        jobs: &mut Vec<PendingDownload>,
    ) -> Map<String, Value> {
        match &msg.raw {
            tl::enums::Message::Message(m) => {
                let mut out = base_message(m, &self.names);
                let mut preview_src: Option<String> = None;
                // Filenames are decided **before** bytes are fetched, so the
                // JSON and HTML stream out now and the pool catches up later.
                if let Some(media) = &m.media {
                    if let Some(facts) = plan::classify(media, self.settings.link_previews) {
                        let stamp = media_stamp(m.date);
                        let (fields, job) =
                            plan::plan(&facts, m.id as i64, &stamp, names, self.settings);
                        for (k, v) in fields {
                            out.insert(k, v);
                        }
                        // Read before the job is moved: the preview's name was
                        // claimed by the planner, and deriving it again here
                        // would miss any `(1)` collision suffix and point the
                        // `<img>` at a file the pool never writes.
                        preview_src = job.as_ref().and_then(|j| j.preview_dest.clone());
                        if let Some(job) = job {
                            // `plan::downloadable`, not `msg.media()`: the two
                            // differ on exactly the media `classify` had to
                            // reach inside — a link preview — and the second
                            // hands the pool a `Media::WebPage` grammers
                            // refuses to download.
                            let handle = if job.inline_bytes.is_some() {
                                None
                            } else {
                                plan::downloadable(media, self.settings.link_previews)
                            };
                            jobs.push(PendingDownload { job, media: handle });
                        }
                        out = tgx_format::order::ordered(&out);
                    }
                    // Media that is not a *file*. `classify` only answers "what
                    // would we download", so a poll and a location fell through
                    // it and the message reached the JSON as bare text — all
                    // seven polls and all three locations in the reference.
                    if let tl::enums::MessageMedia::Poll(p) = media {
                        out.insert(
                            "poll".into(),
                            convert::poll_of(p, extras.poll_results.as_ref()),
                        );
                        out = tgx_format::order::ordered(&out);
                    }
                    if let Some((place, period)) = convert::location_of(media) {
                        out.insert("location_information".into(), place);
                        if let Some(seconds) = period {
                            out.insert("live_location_period_seconds".into(), json!(seconds));
                        }
                        out = tgx_format::order::ordered(&out);
                    }
                }
                if let Some(r) = &m.reactions {
                    let tl::enums::MessageReactions::Reactions(r) = r;
                    if let Some(v) =
                        convert::reactions_of(r, extras.reactors.as_deref(), &self.names)
                    {
                        out.insert("reactions".into(), v);
                        out = tgx_format::order::ordered(&out);
                    }
                }
                // Last, because it reads the finished map: the media paths and
                // sizes the plan decided are what the preview points at.
                // `Output::close` strips `_p` before the JSON is written, so
                // this reaches the HTML writer and nothing else.
                if let Some(p) = convert::presentation(m, &out, &self.names, preview_src.as_deref())
                {
                    out.insert("_p".into(), Value::Object(p));
                }
                out
            }
            tl::enums::Message::Service(s) => {
                let mut out = base_service(s, &self.names);
                // A service message can be reacted to like any other.
                if let Some(r) = &s.reactions {
                    let tl::enums::MessageReactions::Reactions(r) = r;
                    if let Some(v) = convert::reactions_of(r, None, &self.names) {
                        out.insert("reactions".into(), v);
                        out = tgx_format::order::ordered(&out);
                    }
                }
                out
            }
            tl::enums::Message::Empty(e) => {
                let mut out = Map::new();
                out.insert("id".into(), json!(e.id));
                out.insert("type".into(), json!("message"));
                out.insert("text".into(), json!(""));
                out.insert("text_entities".into(), json!([]));
                out
            }
        }
    }
}
