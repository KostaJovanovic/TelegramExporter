//! Stripped thumbnails: the blur preview Telegram embeds *inside* the message.
//!
//! **No image is converted and no request is made.** Telegram's size table
//! lists entries that merely *describe* a file on the server, and exactly one
//! `PhotoStrippedSize`, whose `bytes` field **is** a ~180-byte, ~40px image
//! sitting in the message itself — it is what a client shows you while a photo
//! loads, so it has to arrive with the message.
//!
//! Nor is expanding it a conversion. Telegram deletes the boilerplate JPEG
//! header because every stripped thumbnail shares the same quantisation and
//! Huffman tables; putting it back is `header + payload + FFD9`, with exactly
//! two header bytes patched. Measured at **0.9 µs each**.
//!
//! This is the only image that will ever exist for a file too large to
//! download — 1,848 of the 2,684 media messages in the reference, 68.9% —
//! because the preview Desktop renders itself needs the full file on disk.

use crate::jpeg_header::{HEADER, HEIGHT_OFFSET, WIDTH_OFFSET};

/// Expand a stripped payload into a real JPEG.
///
/// Returns `None` when the payload is not one — a too-short buffer, or a first
/// byte that is not the format marker. **Telethon's equivalent returns its
/// input unchanged in that case**, which is not a JPEG and must not be written
/// as one; returning `None` makes that unrepresentable rather than relying on
/// every caller to re-check the magic afterwards.
pub fn expand(stripped: &[u8]) -> Option<Vec<u8>> {
    if stripped.len() < 3 || stripped[0] != 1 {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER.len() + stripped.len() + 2);
    out.extend_from_slice(&HEADER);
    out[WIDTH_OFFSET] = stripped[1];
    out[HEIGHT_OFFSET] = stripped[2];
    out.extend_from_slice(&stripped[3..]);
    out.extend_from_slice(&[0xff, 0xd9]);
    debug_assert!(out.starts_with(&[0xff, 0xd8]));
    Some(out)
}

/// The width and height a stripped payload encodes, without expanding it.
pub fn dimensions(stripped: &[u8]) -> Option<(u8, u8)> {
    if stripped.len() < 3 || stripped[0] != 1 {
        return None;
    }
    Some((stripped[1], stripped[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(w: u8, h: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![1, w, h];
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn the_result_is_a_real_jpeg() {
        let out = expand(&payload(40, 30, &[0xaa; 100])).unwrap();
        assert_eq!(&out[..2], &[0xff, 0xd8], "SOI marker");
        assert_eq!(&out[out.len() - 2..], &[0xff, 0xd9], "EOI marker");
    }

    #[test]
    fn exactly_two_header_bytes_are_patched() {
        // The whole claim is that this is a splice, not a conversion. Verify
        // it byte for byte against the pristine template.
        let out = expand(&payload(0x28, 0x1e, &[])).unwrap();
        for i in 0..HEADER.len() {
            let expected = match i {
                WIDTH_OFFSET => 0x28,
                HEIGHT_OFFSET => 0x1e,
                _ => HEADER[i],
            };
            assert_eq!(out[i], expected, "header byte {i} changed unexpectedly");
        }
    }

    #[test]
    fn the_payload_is_spliced_in_untouched() {
        let body: Vec<u8> = (0..=255u8).collect();
        let out = expand(&payload(1, 1, &body)).unwrap();
        let start = HEADER.len();
        assert_eq!(&out[start..start + body.len()], &body[..]);
    }

    #[test]
    fn the_length_is_header_plus_payload_plus_footer() {
        let body = [0u8; 177];
        let out = expand(&payload(1, 1, &body)).unwrap();
        assert_eq!(out.len(), HEADER.len() + body.len() + 2);
    }

    #[test]
    fn a_payload_it_cannot_expand_returns_none_not_the_input() {
        // Telethon returns the input unchanged here, which is not a JPEG. A
        // caller that trusted it wrote a broken file.
        assert_eq!(expand(&[]), None);
        assert_eq!(expand(&[1, 2]), None, "too short");
        assert_eq!(expand(&[0, 40, 30, 1, 2, 3]), None, "wrong format marker");
        assert_eq!(expand(&[2, 40, 30, 1, 2, 3]), None, "wrong format marker");
    }

    #[test]
    fn dimensions_are_readable_without_expanding() {
        assert_eq!(dimensions(&payload(40, 30, &[])), Some((40, 30)));
        assert_eq!(dimensions(&[0, 1, 2]), None);
    }

    #[test]
    fn the_header_is_the_documented_size() {
        assert_eq!(HEADER.len(), 623);
        assert_eq!(WIDTH_OFFSET, 164);
        assert_eq!(HEIGHT_OFFSET, 166);
    }
}
