//! ICO directory parsing, shared by the window icon and the tray icon.
//!
//! An `.ico` entry is already an `RT_ICON` resource: a 40-byte `BITMAPINFOHEADER` whose
//! `biHeight` is doubled for the XOR bitmap plus AND mask, followed by the pixels and the mask.
//! [`entries`] reads the directory, [`entry_for`] applies the size rule, and [`rgba`] decodes one
//! entry to the straight RGBA rows Rune's tray takes. The window icon hands the payload bytes
//! themselves to `CreateIconFromResourceEx` — no decode.
//!
//! Caps: an entry's decoded side is at most [`MAX_ICON_SIDE`] pixels; a directory or payload that
//! does not fit the file is refused, never guessed.

/// The fixed ICO header: reserved, type, count.
pub(crate) const HEADER_BYTES: usize = 6;
/// One directory entry: width, height, colours, reserved, planes, bit count, bytes, offset.
pub(crate) const ENTRY_BYTES: usize = 16;

/// An ICO's PNG-compressed entries are legal files but not `RT_ICON` bitmaps, so neither
/// `CreateIconFromResourceEx` nor [`rgba`] can read them.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The largest side [`rgba`] decodes. The embedded icons are 16–128 px; a swapped asset asking
/// for more is refused rather than allocated.
pub(crate) const MAX_ICON_SIDE: u32 = 256;

/// One usable image inside an ICO.
#[derive(Debug)]
pub(crate) struct Entry<'a> {
    pub width: u32,
    pub height: u32,
    pub payload: &'a [u8],
}

impl Entry<'_> {
    fn area(&self) -> u32 {
        self.width * self.height
    }
}

/// Parses the ICO directory into its bitmap entries. Each payload is exactly the file's own
/// `RT_ICON` image bytes.
pub(crate) fn entries(ico: &[u8]) -> Result<Vec<Entry<'_>>, &'static str> {
    let header = ico.get(..HEADER_BYTES).ok_or("truncated header")?;
    let reserved = u16::from_le_bytes([header[0], header[1]]);
    if reserved != 0 {
        return Err("reserved field is not zero");
    }
    let kind = u16::from_le_bytes([header[2], header[3]]);
    if kind != 1 {
        return Err("not an icon file");
    }
    let count = u16::from_le_bytes([header[4], header[5]]) as usize;
    let directory = ico
        .get(HEADER_BYTES..HEADER_BYTES + count * ENTRY_BYTES)
        .ok_or("truncated directory")?;
    let mut entries = Vec::with_capacity(count);
    for entry in directory.as_chunks::<ENTRY_BYTES>().0 {
        let width = entry[0] as u32;
        let height = entry[1] as u32;
        let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        let end = offset.checked_add(size).ok_or("payload overflows")?;
        let payload = ico.get(offset..end).ok_or("payload is outside the file")?;
        if payload.is_empty() || payload.starts_with(&PNG_SIGNATURE) {
            continue;
        }
        entries.push(Entry {
            // The directory stores 256 as 0.
            width: if width == 0 { 256 } else { width },
            height: if height == 0 { 256 } else { height },
            payload,
        });
    }
    Ok(entries)
}

/// The entry to hand to Windows for `size`: the exact size when present, else the smallest entry
/// at least as large, else the largest entry. Exact first keeps 32×32 and 64×64 exact; the
/// fallbacks avoid upscaling, then avoid no icon at all.
pub(crate) fn entry_for<'e, 'a>(entries: &'e [Entry<'a>], size: u32) -> Option<&'e Entry<'a>> {
    entries
        .iter()
        .find(|entry| entry.width == size && entry.height == size)
        .or_else(|| {
            entries
                .iter()
                .filter(|entry| entry.width >= size && entry.height >= size)
                .min_by_key(|entry| entry.area())
        })
        .or_else(|| entries.iter().max_by_key(|entry| entry.area()))
}

/// Straight RGBA rows (`width × height × 4`), the shape [`rune::TrayIcon::rgba`] takes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct IconPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes the entry [`entry_for`] picks for `size` to straight RGBA, top row first.
///
/// The ICO stores 32-bit BGRA rows bottom-up with a doubled `biHeight`, then the 1-bit AND mask.
/// When the entry has an alpha channel that is used (Windows 2000+); a legacy all-zero alpha
/// falls back to the mask (a set bit means transparent).
pub(crate) fn rgba(ico: &[u8], size: u32) -> Option<IconPixels> {
    let entries = entries(ico).ok()?;
    let payload = entry_for(&entries, size)?.payload;
    if payload.len() < 40 {
        return None;
    }
    let header = u32::from_le_bytes(payload[0..4].try_into().ok()?);
    if header != 40 {
        return None;
    }
    let width = i32::from_le_bytes(payload[4..8].try_into().ok()?);
    let doubled_height = i32::from_le_bytes(payload[8..12].try_into().ok()?);
    let bit_count = u16::from_le_bytes(payload[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(payload[16..20].try_into().ok()?);
    if width <= 0 || doubled_height <= 0 || doubled_height % 2 != 0 || bit_count != 32 {
        return None;
    }
    if compression != 0 {
        return None;
    }
    let width = width as u32;
    let height = doubled_height as u32 / 2;
    if width > MAX_ICON_SIDE || height > MAX_ICON_SIDE {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let xor_len = w * h * 4;
    let mask_row = w.div_ceil(32) * 4;
    let mask_len = mask_row * h;
    if payload.len() < 40 + xor_len + mask_len {
        return None;
    }
    let pixels = &payload[40..40 + xor_len];
    let mask = &payload[40 + xor_len..40 + xor_len + mask_len];
    let any_alpha = pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0);
    let mut rgba = vec![0u8; xor_len];
    for y in 0..h {
        let source_row = h - 1 - y; // ICO rows are bottom-up.
        for x in 0..w {
            let source = (source_row * w + x) * 4;
            let target = (y * w + x) * 4;
            let mask_bit = (mask[source_row * mask_row + x / 8] >> (7 - (x % 8))) & 1;
            let alpha = if any_alpha {
                pixels[source + 3]
            } else if mask_bit == 1 {
                0
            } else {
                255
            };
            rgba[target] = pixels[source + 2];
            rgba[target + 1] = pixels[source + 1];
            rgba[target + 2] = pixels[source];
            rgba[target + 3] = alpha;
        }
    }
    Some(IconPixels {
        width,
        height,
        rgba,
    })
}
