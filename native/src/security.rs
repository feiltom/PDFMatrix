//! Standard security handler: building encryption states for saving.

use std::collections::BTreeMap;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions, StringFormat};

use crate::doc::Res;

/// What to do with security when saving.
#[derive(Clone, Debug, Default)]
pub enum Security {
    /// Keep the security of the source document (if it was encrypted).
    #[default]
    Keep,
    /// Save without encryption.
    Remove,
    /// Encrypt with new parameters.
    Set(SecurityParams),
}

#[derive(Clone, Debug)]
pub struct SecurityParams {
    /// 1 RC4-40, 2 RC4-128, 3 AES-128, 4 AES-256 (PDFNet SecurityHandler.AlgorithmType).
    pub algorithm: u32,
    pub user_password: String,
    pub owner_password: String,
    /// Allowed operations, as PDF /P permission bits (ISO 32000-1 table 22).
    pub permissions: u64,
    pub encrypt_metadata: bool,
}

fn random_bytes<const N: usize>() -> Res<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).map_err(|e| format!("no randomness available: {e}"))?;
    Ok(b)
}

/// RC4/AES-128 keys are derived from the first /ID element, so make sure there is one.
pub fn ensure_id(doc: &mut Document) -> Res<()> {
    if doc.trailer.get(b"ID").is_err() {
        let a = random_bytes::<16>()?.to_vec();
        doc.trailer.set(
            "ID",
            Object::Array(vec![
                Object::String(a.clone(), StringFormat::Hexadecimal),
                Object::String(a, StringFormat::Hexadecimal),
            ]),
        );
    }
    Ok(())
}

fn filters(f: Arc<dyn CryptFilter>) -> BTreeMap<Vec<u8>, Arc<dyn CryptFilter>> {
    BTreeMap::from([(b"StdCF".to_vec(), f)])
}

/// Builds the encryption state for `doc` (which must already carry an /ID for algorithms 1-3).
pub fn state_for(doc: &Document, p: &SecurityParams) -> Res<EncryptionState> {
    let permissions = Permissions::from_bits_truncate(p.permissions);
    let owner = if p.owner_password.is_empty() {
        p.user_password.as_str()
    } else {
        p.owner_password.as_str()
    };
    let key = random_bytes::<32>()?;
    let version = match p.algorithm {
        1 => EncryptionVersion::V1 {
            document: doc,
            owner_password: owner,
            user_password: &p.user_password,
            permissions,
        },
        2 => EncryptionVersion::V2 {
            document: doc,
            owner_password: owner,
            user_password: &p.user_password,
            key_length: 128,
            permissions,
        },
        3 => EncryptionVersion::V4 {
            document: doc,
            encrypt_metadata: p.encrypt_metadata,
            crypt_filters: filters(Arc::new(Aes128CryptFilter)),
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: owner,
            user_password: &p.user_password,
            permissions,
        },
        4 => EncryptionVersion::V5 {
            encrypt_metadata: p.encrypt_metadata,
            crypt_filters: filters(Arc::new(Aes256CryptFilter)),
            file_encryption_key: &key,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: owner,
            user_password: &p.user_password,
            permissions,
        },
        other => return Err(format!("unsupported encryption algorithm {other}")),
    };
    EncryptionState::try_from(version).map_err(|e| format!("cannot set up encryption: {e}"))
}

pub struct SecurityInfo {
    pub encrypted: bool,
    /// 1 RC4-40, 2 RC4-128, 3 AES-128, 4 AES-256, 0 none.
    pub algorithm: u32,
    pub key_length: u32,
    pub permissions: u64,
    pub revision: i64,
}

pub fn info(doc: &Document) -> SecurityInfo {
    let Some(state) = doc.encryption_state.as_ref() else {
        return SecurityInfo {
            encrypted: false,
            algorithm: 0,
            key_length: 0,
            permissions: u64::MAX,
            revision: 0,
        };
    };
    let key_bits = state
        .key_length()
        .map(|l| if l <= 32 { l * 8 } else { l })
        .unwrap_or(40) as u32;
    let algorithm = match (state.version(), state.revision()) {
        (5, _) | (_, 5) | (_, 6) => 4,
        (4, _) => {
            if state
                .crypt_filters()
                .values()
                .any(|f| f.method() == b"AESV2")
            {
                3
            } else {
                2
            }
        }
        (2, _) | (3, _) => 2,
        _ if key_bits > 40 => 2,
        _ => 1,
    };
    SecurityInfo {
        encrypted: true,
        algorithm,
        key_length: match algorithm {
            4 => 256,
            3 => 128,
            _ => key_bits,
        },
        permissions: state.permissions().bits(),
        revision: state.revision(),
    }
}
