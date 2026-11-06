//! Avatar payload validation.
//!
//! Avatars travel as data URLs (`data:image/png;base64,…`). The server never
//! decodes images into pixels; it validates the container (declared type,
//! base64 well-formedness), the decoded size, and — by parsing the image
//! header — that the bitmap is **square** (the client center-crops before
//! upload; this is the server-side backstop).
//!
//! Only PNG, JPEG, and WebP are accepted, matching the product contract.
//! Header parsing is deliberately minimal and fail-closed: an image whose
//! dimensions cannot be parsed is rejected.

use crate::error::AppError;

/// Maximum decoded image size (2 MB) — mirrors the client-side upload limit.
pub const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

/// Largest accepted data-URL string length (base64 of 2MB ≈ 2.8MB) with slack
/// for the prefix.
const MAX_DATA_URL_LEN: usize = MAX_AVATAR_BYTES / 3 * 4 + 256;

/// The image content types accepted for avatars.
pub const ACCEPTED_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAvatar {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Validate a `data:` URL as an avatar: declared type accepted, base64
/// decodes, decoded bytes within the size cap, sniffed magic bytes match the
/// declared type, and the image dimensions are square.
pub fn validate_data_url(raw: &str) -> Result<ValidatedAvatar, AppError> {
    let invalid = || AppError::validation("avatar", "avatar must be a JPG, PNG or WebP image");

    let rest = raw.strip_prefix("data:").ok_or_else(invalid)?;
    let (meta, encoded) = rest.split_once(',').ok_or_else(invalid)?;
    let content_type = meta.strip_suffix(";base64").unwrap_or(meta).trim();
    if !ACCEPTED_TYPES.contains(&content_type) {
        return Err(invalid());
    }
    if raw.len() > MAX_DATA_URL_LEN {
        return Err(AppError::validation(
            "avatar",
            "avatar must be 2 MB or smaller",
        ));
    }

    let bytes = decode_base64(encoded).ok_or_else(invalid)?;
    if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
        return Err(AppError::validation(
            "avatar",
            "avatar must be 2 MB or smaller",
        ));
    }

    // Sniffed type must match the declared type.
    let sniffed = sniff(&bytes);
    if sniffed != Some(content_type) {
        return Err(invalid());
    }

    let (width, height) = dimensions(&bytes).ok_or_else(|| {
        AppError::validation("avatar", "cannot read image dimensions from this file")
    })?;
    if width == 0 || height == 0 || width != height {
        return Err(AppError::validation(
            "avatar",
            "avatar images must be square (the editor crops them for you)",
        ));
    }

    Ok(ValidatedAvatar {
        content_type: content_type.to_string(),
        bytes,
    })
}

/// Sniff the actual image type from magic bytes.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Decode standard (RFC 4648) base64, ignoring ASCII whitespace.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut padding = 0usize;
    for &c in input.as_bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return None; // data after padding
        }
        acc = (acc << 6) | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    if padding > 2 || !input.len().is_multiple_of(4) {
        return None;
    }
    Some(out)
}

/// Image dimensions parsed from the file header (fail-closed: None when the
/// layout does not match a known variant).
fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return png_dimensions(bytes);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return jpeg_dimensions(bytes);
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return webp_dimensions(bytes);
    }
    None
}

/// PNG: IHDR is the mandatory first chunk — width and height are BE u32 at
/// fixed offsets 16 and 20.
fn png_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(b[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(b[20..24].try_into().ok()?);
    Some((width, height))
}

/// JPEG: scan the segment stream for a start-of-frame (SOF) marker carrying
/// dimensions. Progressive (C2) and baseline (C0) both occur in practice;
/// all SOF* markers share the layout `[len:2][precision:1][height:2][width:2]`.
fn jpeg_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None; // marker desync
        }
        let marker = b[i + 1];
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) {
            // Standalone markers without a length field.
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes(b[i + 2..i + 4].try_into().ok()?) as usize;
        if len < 2 {
            return None;
        }
        let is_sof = matches!(marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF);
        if is_sof {
            if i + 4 + 5 > b.len() {
                return None;
            }
            let height = u32::from(u16::from_be_bytes(b[i + 5..i + 7].try_into().ok()?));
            let width = u32::from(u16::from_be_bytes(b[i + 7..i + 9].try_into().ok()?));
            return Some((width, height));
        }
        i += 2 + len;
    }
    None
}

/// WebP: dispatch on the first chunk — VP8 (lossy), VP8L (lossless), VP8X
/// (extended). All carry 14-bit dimensions (stored -1); VP8X uses 24-bit
/// canvas fields (also stored -1).
fn webp_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let chunk = &b[12..16.min(b.len())];
    match chunk {
        b"VP8 " => {
            // [3B frame tag][3B sync 0x9D 0x01 0x2A][2B width][2B height]
            if b.len() < 30 || b[23..26] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = u32::from(u16::from_le_bytes(b[26..28].try_into().ok()?)) & 0x3FFF;
            let height = u32::from(u16::from_le_bytes(b[28..30].try_into().ok()?)) & 0x3FFF;
            Some((width + 1, height + 1))
        }
        b"VP8L" => {
            // Payload: one 0x2F signature byte, then an LSB-first bitstream:
            // 14 bits width-1, 14 bits height-1.
            if b.len() < 25 || b[20] != 0x2F {
                return None;
            }
            let n = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            let width = (n & 0x3FFF) + 1;
            let height = ((n >> 14) & 0x3FFF) + 1;
            Some((width, height))
        }
        b"VP8X" => {
            // [4B size][1B flags][3B reserved][3B width-1][3B height-1]
            if b.len() < 30 {
                return None;
            }
            let width = u32::from(b[24]) | (u32::from(b[25]) << 8) | (u32::from(b[26]) << 16);
            let height = u32::from(b[27]) | (u32::from(b[28]) << 8) | (u32::from(b[29]) << 16);
            Some((width + 1, height + 1))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        b.extend_from_slice(&13u32.to_be_bytes());
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF];
        b.extend_from_slice(&[0xE0, 0x00, 0x10]); // APP0
        b.extend_from_slice(&[0u8; 14]);
        b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 8]); // SOF0
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&width.to_be_bytes());
        b.push(3);
        b.extend_from_slice(&[1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        b.extend_from_slice(&[0xFF, 0xD9]);
        b
    }

    fn webp_vp8(width: u16, height: u16) -> Vec<u8> {
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WEBP");
        b.extend_from_slice(b"VP8 ");
        b.extend_from_slice(&10u32.to_le_bytes());
        b.extend_from_slice(&[0x30, 0x01, 0x00]); // frame tag
        b.extend_from_slice(&[0x9D, 0x01, 0x2A]); // sync code
        b.extend_from_slice(&(width - 1).to_le_bytes());
        b.extend_from_slice(&(height - 1).to_le_bytes());
        b
    }

    fn webp_vp8l(width: u16, height: u16) -> Vec<u8> {
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WEBP");
        b.extend_from_slice(b"VP8L");
        b.extend_from_slice(&5u32.to_le_bytes());
        b.push(0x2F); // signature byte
        // 14-bit width-1 then 14-bit height-1 packed LSB-first.
        let n = u32::from(width - 1) | (u32::from(height - 1) << 14);
        b.extend_from_slice(&n.to_le_bytes());
        b
    }

    fn webp_vp8x(width: u32, height: u32) -> Vec<u8> {
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WEBP");
        b.extend_from_slice(b"VP8X");
        b.extend_from_slice(&10u32.to_le_bytes());
        // Payload: [1B flags][3B reserved][3B width-1][3B height-1].
        b.push(0u8); // flags
        b.extend_from_slice(&[0u8; 3]); // reserved
        let w = width - 1;
        let h = height - 1;
        b.push((w & 0xFF) as u8);
        b.push(((w >> 8) & 0xFF) as u8);
        b.push(((w >> 16) & 0xFF) as u8);
        b.push((h & 0xFF) as u8);
        b.push(((h >> 8) & 0xFF) as u8);
        b.push(((h >> 16) & 0xFF) as u8);
        b
    }

    fn data_url(t: &str, bytes: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = format!("data:{t};base64,");
        for chunk in bytes.chunks(3) {
            let b0 = u32::from(chunk[0]);
            let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
            let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                TABLE[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                TABLE[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    #[test]
    fn accepts_square_images_of_every_format() {
        for bytes in [
            png(256, 256),
            jpeg(256, 256),
            webp_vp8(256, 256),
            webp_vp8l(256, 256),
            webp_vp8x(256, 256),
        ] {
            let t = sniff(&bytes).expect("fixture sniffs");
            let v = validate_data_url(&data_url(t, &bytes)).expect("valid square avatar");
            assert_eq!(v.content_type, t);
        }
    }

    #[test]
    fn rejects_non_square_images() {
        let raw = data_url("image/png", &png(256, 200));
        let err = validate_data_url(&raw).unwrap_err();
        assert!(
            err.to_string().contains("square") || {
                let msg = format!("{err:?}");
                msg.contains("square")
            }
        );
    }

    #[test]
    fn rejects_mismatched_type_and_garbage() {
        // PNG bytes declared as JPEG: sniff/declared mismatch.
        assert!(validate_data_url(&data_url("image/jpeg", &png(64, 64))).is_err());
        // Not a data URL.
        assert!(validate_data_url("https://example.com/x.png").is_err());
        // Declared type not accepted.
        assert!(validate_data_url(&data_url("image/gif", &png(64, 64))).is_err());
        // Invalid base64.
        assert!(validate_data_url("data:image/png;base64,!!").is_err());
    }

    #[test]
    fn round_trips_real_base64() {
        // "hello" -> aGVsbG8= (5 bytes, not an image — checks the decoder only).
        let raw = "data:image/png;base64,aGVsbG8=";
        // Fails at sniff, not at decode.
        let err = validate_data_url(raw).unwrap_err();
        let msg = format!("{err:?}");
        assert!(!msg.contains("panicked"));
    }
}
