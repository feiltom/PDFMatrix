//! Embedding raster images as PDF Image XObjects.

use std::io::Cursor;

use image::{ColorType, ImageDecoder, ImageReader};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::doc::{Res, err};

pub struct EmbeddedImage {
    pub obj: ObjectId,
    pub width: u32,
    pub height: u32,
}

fn image_dict(width: u32, height: u32, color_space: &str, bpc: i64) -> Dictionary {
    Dictionary::from_iter(vec![
        ("Type", Object::Name(b"XObject".to_vec())),
        ("Subtype", Object::Name(b"Image".to_vec())),
        ("Width", Object::Integer(width as i64)),
        ("Height", Object::Integer(height as i64)),
        ("ColorSpace", Object::Name(color_space.as_bytes().to_vec())),
        ("BitsPerComponent", Object::Integer(bpc)),
    ])
}

/// Embeds an image file's bytes (PNG, JPEG, JPEG 2000, GIF, BMP, TIFF, WebP).
pub fn embed(doc: &mut Document, data: &[u8]) -> Res<EmbeddedImage> {
    if let Some(img) = embed_jpx_passthrough(doc, data) {
        return Ok(img);
    }
    if data.starts_with(&[0xFF, 0xD8])
        && let Some(img) = embed_jpeg_passthrough(doc, data)
    {
        return Ok(img);
    }
    let decoded = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(err("cannot read image"))?
        .decode()
        .map_err(err("cannot decode image"))?;
    let (width, height) = (decoded.width(), decoded.height());
    let has_alpha = decoded.color().has_alpha();
    let gray = matches!(
        decoded.color(),
        ColorType::L8 | ColorType::La8 | ColorType::L16 | ColorType::La16
    );

    let (pixels, cs) = if gray {
        (decoded.to_luma8().into_raw(), "DeviceGray")
    } else {
        (decoded.to_rgb8().into_raw(), "DeviceRGB")
    };
    let mut dict = image_dict(width, height, cs, 8);
    if has_alpha {
        let alpha: Vec<u8> = decoded.to_rgba8().pixels().map(|p| p.0[3]).collect();
        if alpha.iter().any(|a| *a != 255) {
            let mut mask = Stream::new(image_dict(width, height, "DeviceGray", 8), alpha);
            let _ = mask.compress();
            let mask_id = doc.add_object(mask);
            dict.set("SMask", Object::Reference(mask_id));
        }
    }
    let mut s = Stream::new(dict, pixels);
    let _ = s.compress();
    Ok(EmbeddedImage {
        obj: doc.add_object(s),
        width,
        height,
    })
}

/// Keeps baseline/progressive JPEG data as is (DCTDecode) for gray and RGB images.
fn embed_jpeg_passthrough(doc: &mut Document, data: &[u8]) -> Option<EmbeddedImage> {
    let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(data)).ok()?;
    let (width, height) = decoder.dimensions();
    let cs = match decoder.color_type() {
        ColorType::L8 => "DeviceGray",
        ColorType::Rgb8 => "DeviceRGB",
        _ => return None,
    };
    let mut dict = image_dict(width, height, cs, 8);
    dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
    let mut s = Stream::new(dict, data.to_vec());
    s.allows_compression = false;
    Some(EmbeddedImage {
        obj: doc.add_object(s),
        width,
        height,
    })
}

/// JPEG 2000 (.jp2 container or raw .j2k codestream) is embedded as is (JPXDecode); PDF readers
/// decode it natively and the colorspace comes from the codestream.
fn embed_jpx_passthrough(doc: &mut Document, data: &[u8]) -> Option<EmbeddedImage> {
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let jp2_signature = [
        0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
    ];
    let (width, height) = if data.len() > 12 && data[..12] == jp2_signature {
        // JP2: the image header box holds height then width.
        let at = data.windows(4).position(|w| w == b"ihdr")?;
        let b = data.get(at + 4..at + 12)?;
        (be32(&b[4..8]), be32(&b[0..4]))
    } else if data.len() > 24 && data[..4] == [0xFF, 0x4F, 0xFF, 0x51] {
        // Raw codestream: SIZ marker (Xsiz, Ysiz, XOsiz, YOsiz).
        let s = &data[8..];
        (
            be32(&s[0..4]).checked_sub(be32(&s[8..12]))?,
            be32(&s[4..8]).checked_sub(be32(&s[12..16]))?,
        )
    } else {
        return None;
    };
    let dict = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"XObject".to_vec())),
        ("Subtype", Object::Name(b"Image".to_vec())),
        ("Width", Object::Integer(width as i64)),
        ("Height", Object::Integer(height as i64)),
        ("Filter", Object::Name(b"JPXDecode".to_vec())),
    ]);
    let mut s = Stream::new(dict, data.to_vec());
    s.allows_compression = false;
    Some(EmbeddedImage {
        obj: doc.add_object(s),
        width,
        height,
    })
}
