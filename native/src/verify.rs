//! Verification of PDF signatures (CMS detached, RSA PKCS#1 v1.5 and ECDSA P-256/P-384).
//!
//! The status codes follow PDFNet's VerificationResult enums.

use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier, SignerInfo};
use der::asn1::OctetStringRef;
use der::{Decode, Encode};
use lopdf::{Document, ObjectId};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};
use signature::hazmat::PrehashVerifier;
use spki::DecodePublicKey;
use x509_cert::Certificate;

use crate::doc::{deref, number};
use crate::forms;
use crate::sign::common_name;
use crate::strings::object_to_text;

#[allow(dead_code)]
pub mod status {
    // DocumentStatus
    pub const DOC_NO_ERROR: u32 = 0;
    pub const DOC_CORRUPT_FILE: u32 = 1;
    pub const DOC_UNSIGNED: u32 = 2;
    pub const DOC_BAD_BYTERANGES: u32 = 3;
    pub const DOC_CORRUPT_CMS: u32 = 4;
    // DigestStatus
    pub const DIGEST_INVALID: u32 = 0;
    pub const DIGEST_VERIFIED: u32 = 1;
    pub const DIGEST_WEAK_BUT_VERIFIED: u32 = 3;
    pub const DIGEST_NONE: u32 = 4;
    pub const DIGEST_UNSUPPORTED: u32 = 5;
    // TrustStatus
    pub const TRUST_VERIFIED: u32 = 0;
    pub const TRUST_UNTRUSTED: u32 = 1;
    pub const TRUST_DISABLED: u32 = 2;
    pub const TRUST_NONE: u32 = 3;
    // ModificationPermissionsStatus
    pub const PERM_HAS_ALLOWED_CHANGES: u32 = 1;
    pub const PERM_UNMODIFIED: u32 = 2;
    pub const PERM_NONE: u32 = 4;
}
use status::*;

pub struct Options {
    /// DER-encoded trusted certificates (roots or the signer itself).
    pub trusted: Vec<Vec<u8>>,
    pub check_trust: bool,
}

#[derive(Default)]
pub struct Report {
    pub field: Option<ObjectId>,
    pub field_name: String,
    pub signed: bool,
    pub subfilter: String,
    pub signer_name: String,
    pub signing_time: String,
    pub reason: String,
    pub location: String,
    pub contact_info: String,
    pub document_status: u32,
    pub digest_status: u32,
    pub trust_status: u32,
    pub permissions_status: u32,
    pub digest_algorithm: String,
    pub verified: bool,
    pub message: String,
    /// Signer certificate first, then the other certificates of the CMS (DER).
    pub certificates: Vec<Vec<u8>>,
    pub byte_range: Vec<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn from_oid(oid: &str) -> Option<Hash> {
        Some(match oid {
            "1.3.14.3.2.26" | "1.2.840.113549.1.1.5" | "1.2.840.10045.4.1" => Hash::Sha1,
            "2.16.840.1.101.3.4.2.1" | "1.2.840.113549.1.1.11" | "1.2.840.10045.4.3.2" => {
                Hash::Sha256
            }
            "2.16.840.1.101.3.4.2.2" | "1.2.840.113549.1.1.12" | "1.2.840.10045.4.3.3" => {
                Hash::Sha384
            }
            "2.16.840.1.101.3.4.2.3" | "1.2.840.113549.1.1.13" | "1.2.840.10045.4.3.4" => {
                Hash::Sha512
            }
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Hash::Sha1 => "SHA-1",
            Hash::Sha256 => "SHA-256",
            Hash::Sha384 => "SHA-384",
            Hash::Sha512 => "SHA-512",
        }
    }

    fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut h = D::new();
            for p in parts {
                h.update(p);
            }
            h.finalize().to_vec()
        }
        match self {
            Hash::Sha1 => run::<Sha1>(parts),
            Hash::Sha256 => run::<Sha256>(parts),
            Hash::Sha384 => run::<Sha384>(parts),
            Hash::Sha512 => run::<Sha512>(parts),
        }
    }
}

/// Verifies `sig` over `hashed` (already digested with `hash`) with the key in `spki_der`.
fn verify_prehashed(spki_der: &[u8], hash: Hash, hashed: &[u8], sig: &[u8]) -> bool {
    if let Ok(pk) = rsa::RsaPublicKey::from_public_key_der(spki_der) {
        let scheme = match hash {
            Hash::Sha1 => rsa::Pkcs1v15Sign::new::<Sha1>(),
            Hash::Sha256 => rsa::Pkcs1v15Sign::new::<Sha256>(),
            Hash::Sha384 => rsa::Pkcs1v15Sign::new::<Sha384>(),
            Hash::Sha512 => rsa::Pkcs1v15Sign::new::<Sha512>(),
        };
        return pk.verify(scheme, hashed, sig).is_ok();
    }
    if let Ok(vk) = p256::ecdsa::VerifyingKey::from_public_key_der(spki_der) {
        return p256::ecdsa::Signature::from_der(sig)
            .is_ok_and(|s| vk.verify_prehash(hashed, &s).is_ok());
    }
    if let Ok(vk) = p384::ecdsa::VerifyingKey::from_public_key_der(spki_der) {
        return p384::ecdsa::Signature::from_der(sig)
            .is_ok_and(|s| vk.verify_prehash(hashed, &s).is_ok());
    }
    false
}

/// Checks that `issuer` signed `cert`.
fn issued_by(cert: &Certificate, issuer: &Certificate) -> bool {
    if cert.tbs_certificate.issuer != issuer.tbs_certificate.subject {
        return false;
    }
    let (Ok(tbs), Ok(spki)) = (
        cert.tbs_certificate.to_der(),
        issuer.tbs_certificate.subject_public_key_info.to_der(),
    ) else {
        return false;
    };
    let Some(hash) = Hash::from_oid(&cert.signature_algorithm.oid.to_string()) else {
        return false;
    };
    verify_prehashed(
        &spki,
        hash,
        &hash.digest(&[&tbs]),
        cert.signature.raw_bytes(),
    )
}

/// Builds a chain from `leaf` to one of the trusted certificates.
fn chain_is_trusted(leaf: &Certificate, pool: &[Certificate], trusted: &[Certificate]) -> bool {
    let mut current = leaf.clone();
    for _ in 0..10 {
        if trusted.iter().any(|t| t == &current) {
            return true;
        }
        if let Some(_root) = trusted.iter().find(|t| issued_by(&current, t)) {
            return true;
        }
        match pool
            .iter()
            .find(|c| *c != &current && issued_by(&current, c))
        {
            Some(next) => current = next.clone(),
            None => return false,
        }
    }
    false
}

/// Length of the DER TLV at the start of `b` (the /Contents value is zero padded).
fn der_len(b: &[u8]) -> Option<usize> {
    if b.len() < 2 {
        return None;
    }
    let first = b[1] as usize;
    if first < 0x80 {
        return Some(2 + first);
    }
    let n = first & 0x7F;
    if n == 0 || n > 4 || b.len() < 2 + n {
        return None;
    }
    let len = b[2..2 + n]
        .iter()
        .fold(0usize, |acc, x| (acc << 8) | *x as usize);
    Some(2 + n + len)
}

fn hex_decode(s: &[u8]) -> Option<Vec<u8>> {
    let s: Vec<u8> = s
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(std::str::from_utf8(&s[i * 2..i * 2 + 2]).ok()?, 16).ok())
        .collect()
}

fn text_of(doc: &Document, d: &lopdf::Dictionary, key: &[u8]) -> String {
    d.get(key)
        .ok()
        .and_then(|o| object_to_text(deref(doc, o)))
        .unwrap_or_default()
}

pub fn verify_document(doc: &Document, bytes: &[u8], opts: &Options) -> Vec<Report> {
    forms::fields(doc)
        .into_iter()
        .filter(|f| f.kind() == "signature")
        .map(|f| {
            let mut r = verify_field(doc, bytes, f.id, opts);
            r.field_name = f.name.clone();
            r
        })
        .collect()
}

pub fn verify_field(doc: &Document, bytes: &[u8], field: ObjectId, opts: &Options) -> Report {
    let mut r = Report {
        field: Some(field),
        document_status: DOC_UNSIGNED,
        digest_status: DIGEST_NONE,
        trust_status: TRUST_NONE,
        permissions_status: PERM_NONE,
        ..Default::default()
    };
    let Some(v) = forms::field_attr(doc, field, b"V") else {
        r.message = "the field is not signed".into();
        return r;
    };
    let Ok(sig) = deref(doc, &v).as_dict() else {
        r.message = "the field is not signed".into();
        return r;
    };
    r.signed = true;
    r.subfilter = sig
        .get(b"SubFilter")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    r.signer_name = text_of(doc, sig, b"Name");
    r.signing_time = text_of(doc, sig, b"M");
    r.reason = text_of(doc, sig, b"Reason");
    r.location = text_of(doc, sig, b"Location");
    r.contact_info = text_of(doc, sig, b"ContactInfo");

    // ---- byte range
    let br: Vec<i64> = sig
        .get(b"ByteRange")
        .ok()
        .and_then(|o| deref(doc, o).as_array().ok())
        .map(|a| {
            a.iter()
                .filter_map(|x| number(doc, x))
                .map(|x| x as i64)
                .collect()
        })
        .unwrap_or_default();
    r.byte_range = br.clone();
    let valid_range = br.len() == 4
        && br[0] == 0
        && br.iter().all(|x| *x >= 0)
        && br[1] < br[2]
        && (br[2] + br[3]) as usize <= bytes.len()
        && bytes.get(br[1] as usize) == Some(&b'<')
        && bytes.get(br[2] as usize - 1) == Some(&b'>');
    if !valid_range {
        r.document_status = DOC_BAD_BYTERANGES;
        r.message = "invalid /ByteRange".into();
        return r;
    }
    let (a, b, c) = (br[1] as usize, br[2] as usize, (br[2] + br[3]) as usize);
    r.permissions_status = if c == bytes.len() {
        PERM_UNMODIFIED
    } else {
        PERM_HAS_ALLOWED_CHANGES
    };

    if !matches!(
        r.subfilter.as_str(),
        "adbe.pkcs7.detached" | "ETSI.CAdES.detached"
    ) {
        r.document_status = DOC_NO_ERROR;
        r.digest_status = DIGEST_UNSUPPORTED;
        r.message = format!("unsupported signature format {}", r.subfilter);
        return r;
    }

    // ---- CMS
    let Some(raw) = hex_decode(&bytes[a + 1..b - 1]) else {
        r.document_status = DOC_CORRUPT_CMS;
        r.message = "invalid /Contents".into();
        return r;
    };
    let cms_der = &raw[..der_len(&raw).unwrap_or(raw.len()).min(raw.len())];
    let signed_data = ContentInfo::from_der(cms_der)
        .ok()
        .and_then(|ci| ci.content.decode_as::<SignedData>().ok());
    let Some(sd) = signed_data else {
        r.document_status = DOC_CORRUPT_CMS;
        r.message = "cannot parse the CMS signature".into();
        return r;
    };
    r.document_status = DOC_NO_ERROR;
    let certs: Vec<Certificate> = sd
        .certificates
        .as_ref()
        .map(|s| {
            s.0.iter()
                .filter_map(|c| {
                    if let CertificateChoices::Certificate(x) = c {
                        Some(x.clone())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let Some(si) = sd.signer_infos.0.iter().next() else {
        r.document_status = DOC_CORRUPT_CMS;
        r.message = "the CMS has no signer".into();
        return r;
    };
    let signer = find_signer(si, &certs);
    if let Some(s) = &signer {
        r.certificates.push(s.to_der().unwrap_or_default());
        if r.signer_name.is_empty() {
            r.signer_name = common_name(s);
        }
    }
    r.certificates.extend(
        certs
            .iter()
            .filter(|c| Some(*c) != signer.as_ref())
            .filter_map(|c| c.to_der().ok()),
    );

    // ---- digest + signature
    let Some(hash) = Hash::from_oid(&si.digest_alg.oid.to_string()) else {
        r.digest_status = DIGEST_UNSUPPORTED;
        r.message = format!("unsupported digest algorithm {}", si.digest_alg.oid);
        return r;
    };
    r.digest_algorithm = hash.name().into();
    let content_digest = hash.digest(&[&bytes[..a], &bytes[b..c]]);
    let (signed_message_hash, digest_matches) = match &si.signed_attrs {
        Some(attrs) => {
            let md = attrs
                .iter()
                .find(|at| at.oid == const_oid::db::rfc5911::ID_MESSAGE_DIGEST)
                .and_then(|at| at.values.iter().next())
                .and_then(|v| v.decode_as::<OctetStringRef>().ok())
                .map(|o| o.as_bytes().to_vec());
            let der = attrs.to_der().unwrap_or_default();
            (
                hash.digest(&[&der]),
                md.as_deref() == Some(content_digest.as_slice()),
            )
        }
        None => (content_digest.clone(), true),
    };
    let sig_ok = signer.as_ref().is_some_and(|cert| {
        let spki = cert
            .tbs_certificate
            .subject_public_key_info
            .to_der()
            .unwrap_or_default();
        let sig_hash = Hash::from_oid(&si.signature_algorithm.oid.to_string()).unwrap_or(hash);
        let to_check = if sig_hash == hash {
            signed_message_hash.clone()
        } else {
            si.signed_attrs
                .as_ref()
                .map(|a| sig_hash.digest(&[&a.to_der().unwrap_or_default()]))
                .unwrap_or(content_digest.clone())
        };
        verify_prehashed(&spki, sig_hash, &to_check, si.signature.as_bytes())
    });
    r.digest_status = match (digest_matches && sig_ok, hash) {
        (false, _) => DIGEST_INVALID,
        (true, Hash::Sha1) => DIGEST_WEAK_BUT_VERIFIED,
        (true, _) => DIGEST_VERIFIED,
    };
    if !digest_matches {
        r.message = "the document was modified: digest mismatch".into();
    } else if !sig_ok {
        r.message = if signer.is_none() {
            "signer certificate not found".into()
        } else {
            "invalid signature value".into()
        };
    }

    // ---- trust
    r.trust_status = if !opts.check_trust {
        TRUST_DISABLED
    } else {
        let trusted: Vec<Certificate> = opts
            .trusted
            .iter()
            .filter_map(|d| Certificate::from_der(d).ok())
            .collect();
        match &signer {
            Some(s) if chain_is_trusted(s, &certs, &trusted) => TRUST_VERIFIED,
            _ => TRUST_UNTRUSTED,
        }
    };
    let digest_ok = matches!(r.digest_status, DIGEST_VERIFIED | DIGEST_WEAK_BUT_VERIFIED);
    let trust_ok = matches!(r.trust_status, TRUST_VERIFIED | TRUST_DISABLED);
    r.verified = r.document_status == DOC_NO_ERROR && digest_ok && trust_ok;
    if r.message.is_empty() && !trust_ok {
        r.message = "the signer certificate is not trusted".into();
    }
    r
}

fn find_signer(si: &SignerInfo, certs: &[Certificate]) -> Option<Certificate> {
    match &si.sid {
        SignerIdentifier::IssuerAndSerialNumber(ias) => certs
            .iter()
            .find(|c| {
                c.tbs_certificate.issuer == ias.issuer
                    && c.tbs_certificate.serial_number == ias.serial_number
            })
            .cloned(),
        SignerIdentifier::SubjectKeyIdentifier(ski) => certs
            .iter()
            .find(|c| {
                c.tbs_certificate.extensions.as_ref().is_some_and(|exts| {
                    exts.iter().any(|e| {
                        e.extn_id == const_oid::db::rfc5280::ID_CE_SUBJECT_KEY_IDENTIFIER
                            && e.extn_value.as_bytes().ends_with(ski.0.as_bytes())
                    })
                })
            })
            .cloned(),
    }
}
