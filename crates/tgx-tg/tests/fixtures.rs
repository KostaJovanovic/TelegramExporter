//! Synthetic TL objects, for driving the converter without a connection.
//!
//! `convert.rs` and `plan.rs` are the two modules the parity harness cannot
//! reach: the reference export records Desktop's *output*, not Telegram's
//! *input*, so there is no recorded TL message to replay through them. A live
//! run is the only thing that proves the wire format — but it is not the only
//! thing that can exercise the mapping, and everything below is a shape
//! Telegram really sends.
//!
//! The builders take the full field list because grammers regenerates these
//! structs from Telegram's schema. When a bump breaks this file, that is the
//! point: it means a field appeared that the converter has not been shown.

#![allow(dead_code)]

use grammers_tl_types as tl;

pub fn peer_user(id: i64) -> tl::enums::Peer {
    tl::enums::Peer::User(tl::types::PeerUser { user_id: id })
}

pub fn peer_channel(id: i64) -> tl::enums::Peer {
    tl::enums::Peer::Channel(tl::types::PeerChannel { channel_id: id })
}

/// An ordinary message with nothing set.
pub fn message(id: i32, date: i32) -> tl::types::Message {
    tl::types::Message {
        out: false,
        mentioned: false,
        media_unread: false,
        silent: false,
        post: false,
        from_scheduled: false,
        legacy: false,
        edit_hide: false,
        pinned: false,
        noforwards: false,
        invert_media: false,
        offline: false,
        video_processing_pending: false,
        paid_suggested_post_stars: false,
        paid_suggested_post_ton: false,
        id,
        from_id: Some(peer_user(1)),
        from_boosts_applied: None,
        from_rank: None,
        peer_id: peer_channel(3_586_682_625),
        saved_peer_id: None,
        fwd_from: None,
        via_bot_id: None,
        via_business_bot_id: None,
        guestchat_via_from: None,
        reply_to: None,
        date,
        message: String::new(),
        media: None,
        reply_markup: None,
        entities: None,
        views: None,
        forwards: None,
        replies: None,
        edit_date: None,
        post_author: None,
        grouped_id: None,
        reactions: None,
        restriction_reason: None,
        ttl_period: None,
        quick_reply_shortcut_id: None,
        effect: None,
        factcheck: None,
        report_delivery_until_date: None,
        paid_message_stars: None,
        suggested_post: None,
        schedule_repeat_period: None,
        summary_from_language: None,
        rich_message: None,
    }
}

pub fn service(id: i32, date: i32, action: tl::enums::MessageAction) -> tl::types::MessageService {
    tl::types::MessageService {
        out: false,
        mentioned: false,
        media_unread: false,
        reactions_are_possible: false,
        silent: false,
        post: false,
        legacy: false,
        id,
        from_id: Some(peer_user(1)),
        peer_id: peer_channel(3_586_682_625),
        saved_peer_id: None,
        reply_to: None,
        date,
        action,
        reactions: None,
        ttl_period: None,
    }
}

/// A reply header, as a forum message carries one.
pub fn reply_to(top: Option<i32>, msg: Option<i32>, forum: bool) -> tl::enums::MessageReplyHeader {
    tl::enums::MessageReplyHeader::Header(tl::types::MessageReplyHeader {
        reply_to_scheduled: false,
        forum_topic: forum,
        quote: false,
        reply_to_ephemeral: false,
        reply_to_msg_id: msg,
        reply_to_peer_id: None,
        reply_from: None,
        reply_media: None,
        reply_to_top_id: top,
        quote_text: None,
        quote_entities: None,
        quote_offset: None,
        todo_item_id: None,
        poll_option: None,
    })
}

/// A document, with whatever attributes the caller needs.
pub fn document(
    id: i64,
    mime: &str,
    size: i64,
    attributes: Vec<tl::enums::DocumentAttribute>,
) -> tl::types::Document {
    tl::types::Document {
        id,
        access_hash: 0,
        file_reference: vec![],
        date: 0,
        mime_type: mime.to_string(),
        size,
        thumbs: None,
        video_thumbs: None,
        dc_id: 2,
        attributes,
    }
}

pub fn doc_media(doc: tl::types::Document, spoiler: bool) -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Document(tl::types::MessageMediaDocument {
        nopremium: false,
        spoiler,
        video: false,
        round: false,
        voice: false,
        document: Some(tl::enums::Document::Document(doc)),
        alt_documents: None,
        video_cover: None,
        video_timestamp: None,
        ttl_seconds: None,
    })
}

pub fn photo(id: i64, w: i32, h: i32, size: i32) -> tl::types::Photo {
    tl::types::Photo {
        has_stickers: false,
        id,
        access_hash: 0,
        file_reference: vec![],
        date: 0,
        sizes: vec![tl::enums::PhotoSize::Size(tl::types::PhotoSize {
            r#type: "y".into(),
            w,
            h,
            size,
        })],
        video_sizes: None,
        dc_id: 2,
    }
}

pub fn photo_media(p: tl::types::Photo, spoiler: bool) -> tl::enums::MessageMedia {
    tl::enums::MessageMedia::Photo(tl::types::MessageMediaPhoto {
        spoiler,
        live_photo: false,
        photo: Some(tl::enums::Photo::Photo(p)),
        ttl_seconds: None,
        video: None,
    })
}
