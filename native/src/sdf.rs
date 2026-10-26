//! JSON view of PDF objects, used by the TypeScript layer for simple dictionary access
//! (PDFNet's SDF/Obj layer).
//!
//! Encoding:
//!   null, true/false, numbers            → as is
//!   text strings                          → JS strings (decoded)
//!   names                                 → {"$name": "Foo"}
//!   references                            → {"$ref": [num, gen]}
//!   binary strings (on output only)       → {"$bytes": "hex"}
//!   arrays / dictionaries                 → arrays / objects
//!   streams                               → {"$stream": {…dictionary…}}

use lopdf::{Dictionary, Object, StringFormat};
use serde_json::{Map, Number, Value, json};

use crate::strings::{decode_text, encode_text};

/// True when the bytes look like text (UTF-16 BOM or mostly printable).
fn is_text(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFE, 0xFF])
        || bytes
            .iter()
            .all(|&b| b >= 0x20 || b == b'\n' || b == b'\r' || b == b'\t')
}

pub fn to_json(obj: &Object) -> Value {
    match obj {
        Object::Null => Value::Null,
        Object::Boolean(b) => Value::Bool(*b),
        Object::Integer(i) => json!(i),
        Object::Real(r) => Number::from_f64(*r as f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Object::String(bytes, _) => {
            if is_text(bytes) {
                Value::String(decode_text(bytes))
            } else {
                json!({ "$bytes": bytes.iter().map(|b| format!("{b:02x}")).collect::<String>() })
            }
        }
        Object::Name(n) => json!({ "$name": String::from_utf8_lossy(n) }),
        Object::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        Object::Dictionary(d) => Value::Object(dict_to_json(d)),
        Object::Stream(s) => json!({ "$stream": dict_to_json(&s.dict) }),
        Object::Reference(id) => json!({ "$ref": [id.0, id.1] }),
    }
}

fn dict_to_json(d: &Dictionary) -> Map<String, Value> {
    d.iter()
        .map(|(k, v)| (String::from_utf8_lossy(k).into_owned(), to_json(v)))
        .collect()
}

pub fn from_json(v: &Value) -> Result<Object, String> {
    Ok(match v {
        Value::Null => Object::Null,
        Value::Bool(b) => Object::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Object::Integer(i),
            None => Object::Real(n.as_f64().unwrap_or(0.0) as f32),
        },
        Value::String(s) => encode_text(s),
        Value::Array(items) => {
            Object::Array(items.iter().map(from_json).collect::<Result<_, _>>()?)
        }
        Value::Object(map) => {
            if let Some(Value::String(name)) = map.get("$name") {
                Object::Name(name.as_bytes().to_vec())
            } else if let Some(Value::Array(r)) = map.get("$ref") {
                let num = r.first().and_then(Value::as_u64).ok_or("invalid $ref")? as u32;
                let generation = r.get(1).and_then(Value::as_u64).unwrap_or(0) as u16;
                Object::Reference((num, generation))
            } else if let Some(Value::String(hex)) = map.get("$bytes") {
                let bytes = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|_| "invalid $bytes")?;
                Object::String(bytes, StringFormat::Hexadecimal)
            } else if map.contains_key("$stream") {
                return Err("streams cannot be created from JSON".into());
            } else {
                let mut d = Dictionary::new();
                for (k, v) in map {
                    d.set(k.as_bytes(), from_json(v)?);
                }
                Object::Dictionary(d)
            }
        }
    })
}
