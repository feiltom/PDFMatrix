//! PDF text strings (ISO 32000-1 §7.9.2.2): PDFDocEncoding or UTF-16BE with BOM.

use lopdf::{Object, StringFormat};

/// PDFDocEncoding code points 0x80..=0x9F differ from Latin-1; everything else maps 1:1
/// (0x7F and 0xAD are undefined and are kept as-is).
const PDF_DOC_HIGH: [char; 32] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}',
    '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}', '\u{201D}', '\u{2018}',
    '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}', '\u{0141}', '\u{0152}', '\u{0160}',
    '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017E}', '\u{FFFD}',
];

pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len() >= 3 && bytes[..3] == [0xEF, 0xBB, 0xBF] {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => PDF_DOC_HIGH[(b - 0x80) as usize],
            _ => b as char,
        })
        .collect()
}

/// Encodes as PDFDocEncoding when possible (ASCII + Latin-1 subset), UTF-16BE otherwise.
pub fn encode_text(s: &str) -> Object {
    if s.chars()
        .all(|c| (c as u32) < 0x80 || ((c as u32) >= 0xA0 && (c as u32) <= 0xFF))
    {
        return Object::String(s.chars().map(|c| c as u8).collect(), StringFormat::Literal);
    }
    let mut out = vec![0xFE, 0xFF];
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(out, StringFormat::Hexadecimal)
}

pub fn object_to_text(obj: &Object) -> Option<String> {
    match obj {
        Object::String(bytes, _) => Some(decode_text(bytes)),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}
