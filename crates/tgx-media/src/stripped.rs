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
    // stripped[1] to 164 and stripped[2] to 166, exactly as tdesktop does.
    // Those offsets are the *height* and the *width*, in that order — see
    // [`HEIGHT_OFFSET`]. The two constants used to be named the other way
    // round, which left these two lines byte-correct and read backwards.
    out[HEIGHT_OFFSET] = stripped[1];
    out[WIDTH_OFFSET] = stripped[2];
    out.extend_from_slice(&stripped[3..]);
    out.extend_from_slice(&[0xff, 0xd9]);
    debug_assert!(out.starts_with(&[0xff, 0xd8]));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Height first: the payload is `[1, height, width]`, not `[1, w, h]`.
    fn payload(h: u8, w: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![1, h, w];
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
                HEIGHT_OFFSET => 0x28,
                WIDTH_OFFSET => 0x1e,
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
    fn the_offsets_are_decoded_from_sof0_rather_than_named_by_hand() {
        // They were named the other way round, and that alone had produced a
        // second bug: a dimensions() returning (height, width) under a doc
        // comment promising (width, height). So walk the marker chain instead
        // of trusting either name — SOF0 is `FF C0`, a 2-byte segment length,
        // one precision byte, then height and width as big-endian 16-bit.
        assert_eq!(HEADER.len(), 623);
        let mut i = 2; // past SOI
        let sof0 = loop {
            assert_eq!(HEADER[i], 0xff, "not on a marker boundary at {i}");
            if HEADER[i + 1] == 0xc0 {
                break i;
            }
            let len = usize::from(u16::from_be_bytes([HEADER[i + 2], HEADER[i + 3]]));
            i += 2 + len;
        };
        assert_eq!(sof0, 158);
        let (height_field, width_field) = (sof0 + 5, sof0 + 7);
        assert_eq!(HEIGHT_OFFSET, height_field + 1, "height's low byte");
        assert_eq!(WIDTH_OFFSET, width_field + 1, "width's low byte");
        assert_eq!((HEIGHT_OFFSET, WIDTH_OFFSET), (164, 166));
        // The template ships both fields zeroed; expand fills them in.
        assert_eq!(&HEADER[height_field..height_field + 2], &[0, 0]);
        assert_eq!(&HEADER[width_field..width_field + 2], &[0, 0]);
    }

    #[test]
    fn the_second_payload_byte_is_the_height_not_the_width() {
        // Cross-checked outside Rust: splicing 40 and 30 in this order and
        // handing the result to Pillow reports a 30-wide, 40-tall image.
        let out = expand(&payload(40, 30, &[0xaa; 100])).unwrap();
        assert_eq!(u16::from_be_bytes([out[163], out[164]]), 40, "height");
        assert_eq!(u16::from_be_bytes([out[165], out[166]]), 30, "width");
    }
}
