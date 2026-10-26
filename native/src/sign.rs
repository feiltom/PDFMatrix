//! PAdES-style digital signatures (adbe.pkcs7.detached / ETSI.CAdES.detached) with a PKCS#12
//! identity: RSA (PKCS#1 v1.5) or ECDSA P-256 / P-384.
//!
//! Flow: `prepare` adds the signature dictionary with placeholders (/ByteRange and a zeroed
//! /Contents), the document is serialized, then `sign` patches the byte range, hashes the
//! covered bytes, builds the CMS SignedData and writes it into /Contents.

use cms::builder::{SignedDataBuilder, SignerInfoBuilder, create_signing_time_attribute};
use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::signed_data::{EncapsulatedContentInfo, SignerIdentifier};
use der::{Decode, Encode};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use p12_keystore::{KeyStore, KeyStoreEntry, Pkcs12Archive, Pkcs12ImportPolicy};
use regex::bytes::Regex;
use sha2::{Digest, Sha256, Sha384};
use spki::{AlgorithmIdentifierOwned, EncodePublicKey};
use x509_cert::Certificate;

use crate::annots;
use crate::content::{Ops, Resources};
use crate::doc::{Doc, Res, err, rect_of};
use crate::fonts::StdFont;
use crate::forms;
use crate::strings::encode_text;

/// Bytes reserved for the CMS blob (hex-encoded in the file, so twice as many characters).
const CONTENTS_SIZE: usize = 16384;
const PLACEHOLDER: i64 = 9_999_999_999;

pub enum SignKey {
    Rsa(rsa::RsaPrivateKey),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

pub struct KeyMaterial {
    pub key: SignKey,
    pub cert_der: Vec<u8>,
    /// Other certificates of the PKCS#12 (CA chain), embedded in the CMS.
    pub chain: Vec<Vec<u8>>,
    pub common_name: String,
}

pub struct Pending {
    pub field: ObjectId,
    pub key: KeyMaterial,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    pub subfilter: String,
}

/// "CN=Jean Dupont,O=Bewai" → "Jean Dupont".
pub fn common_name(cert: &Certificate) -> String {
    let dn = cert.tbs_certificate.subject.to_string();
    dn.split(',')
        .find_map(|p| p.trim().strip_prefix("CN="))
        .map(|s| s.replace('\\', ""))
        .unwrap_or(dn)
}

fn parse_key(pkcs8: &[u8]) -> Res<(SignKey, Vec<u8>)> {
    use p256::pkcs8::DecodePrivateKey;
    if let Ok(k) = rsa::RsaPrivateKey::from_pkcs8_der(pkcs8) {
        let spki = k
            .to_public_key()
            .to_public_key_der()
            .map_err(err("public key"))?
            .into_vec();
        return Ok((SignKey::Rsa(k), spki));
    }
    if let Ok(k) = p256::SecretKey::from_pkcs8_der(pkcs8) {
        let spki = k
            .public_key()
            .to_public_key_der()
            .map_err(err("public key"))?
            .into_vec();
        return Ok((SignKey::P256(k.into()), spki));
    }
    if let Ok(k) = p384::SecretKey::from_pkcs8_der(pkcs8) {
        let spki = k
            .public_key()
            .to_public_key_der()
            .map_err(err("public key"))?
            .into_vec();
        return Ok((SignKey::P384(k.into()), spki));
    }
    Err("unsupported private key (RSA, ECDSA P-256 and P-384 are supported)".into())
}

/// Loads the signing identity from a PKCS#12 (.p12/.pfx) file.
pub fn load_p12(data: &[u8], password: &str) -> Res<KeyMaterial> {
    let mut keys: Vec<Vec<u8>> = Vec::new();
    let mut certs: Vec<Vec<u8>> = Vec::new();
    match KeyStore::from_pkcs12(data, password, Pkcs12ImportPolicy::Relaxed) {
        Ok(ks) => {
            for (_, entry) in ks.entries() {
                match entry {
                    KeyStoreEntry::PrivateKeyChain(c) => {
                        keys.push(c.key().as_der().to_vec());
                        certs.extend(c.certs().iter().map(|x| x.as_der().to_vec()));
                    }
                    KeyStoreEntry::Certificate(x) => certs.push(x.as_der().to_vec()),
                    KeyStoreEntry::Secret(_) => {}
                }
            }
        }
        Err(e) => return Err(format!("cannot open PKCS#12 (wrong password?): {e}")),
    }
    if keys.is_empty() {
        // Keys without a local key id are not linked by the keystore model.
        let archive = Pkcs12Archive::from_pkcs12(data, password)
            .map_err(|e| format!("cannot open PKCS#12: {e}"))?;
        keys.extend(archive.keys.iter().map(|k| k.key.as_der().to_vec()));
        certs.extend(archive.certs.iter().map(|c| c.cert.as_der().to_vec()));
    }
    let pkcs8 = keys.first().ok_or("the PKCS#12 contains no private key")?;
    let (key, spki) = parse_key(pkcs8)?;
    let mut leaf = None;
    let mut chain = Vec::new();
    for der in certs {
        let Ok(c) = Certificate::from_der(&der) else {
            continue;
        };
        let cert_spki = c
            .tbs_certificate
            .subject_public_key_info
            .to_der()
            .unwrap_or_default();
        if leaf.is_none() && cert_spki == spki {
            leaf = Some((der, c));
        } else if !chain.contains(&der) {
            chain.push(der);
        }
    }
    let (cert_der, cert) = leaf.ok_or("no certificate in the PKCS#12 matches its private key")?;
    Ok(KeyMaterial {
        key,
        common_name: common_name(&cert),
        cert_der,
        chain,
    })
}

pub fn has_signatures(doc: &Document) -> bool {
    forms::fields(doc).iter().any(|f| {
        f.kind() == "signature"
            && forms::field_attr(doc, f.id, b"V").is_some_and(|v| v.as_dict().is_ok())
    })
}

/// Adds the signature dictionary (with placeholders) to the field and draws its appearance.
pub fn prepare(doc: &mut Doc, p: &Pending) -> Res<()> {
    if doc.inner.was_encrypted() {
        return Err("signing encrypted documents is not supported yet".into());
    }
    let now = annots::pdf_now();
    let mut sig = Dictionary::from_iter(vec![
        ("Type", Object::Name(b"Sig".to_vec())),
        ("Filter", Object::Name(b"Adobe.PPKLite".to_vec())),
        ("SubFilter", Object::Name(p.subfilter.as_bytes().to_vec())),
        (
            "ByteRange",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(PLACEHOLDER),
                Object::Integer(PLACEHOLDER),
                Object::Integer(PLACEHOLDER),
            ]),
        ),
        (
            "Contents",
            Object::String(vec![0u8; CONTENTS_SIZE], StringFormat::Hexadecimal),
        ),
        ("M", Object::string_literal(now.clone())),
        ("Name", encode_text(&p.key.common_name)),
    ]);
    if let Some(r) = &p.reason {
        sig.set("Reason", encode_text(r));
    }
    if let Some(l) = &p.location {
        sig.set("Location", encode_text(l));
    }
    if let Some(c) = &p.contact_info {
        sig.set("ContactInfo", encode_text(c));
    }
    let sig_id = doc.inner.add_object(sig);
    let field = doc
        .inner
        .get_dictionary_mut(p.field)
        .map_err(err("invalid signature field"))?;
    if field
        .get(b"V")
        .is_ok_and(|v| v.as_reference().is_ok() || v.as_dict().is_ok())
    {
        return Err("this signature field is already signed".into());
    }
    field.set("V", Object::Reference(sig_id));
    // SignaturesExist | AppendOnly
    let form_id = doc
        .inner
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(|o| o.as_reference().ok());
    if let Some(id) = form_id {
        doc.inner
            .get_dictionary_mut(id)
            .map_err(err("AcroForm"))?
            .set("SigFlags", Object::Integer(3));
    }

    let widgets = forms::fields(&doc.inner)
        .into_iter()
        .find(|f| f.id == p.field)
        .map(|f| f.widgets)
        .unwrap_or_default();
    for w in widgets {
        let Some(rect) = doc
            .inner
            .get_dictionary(w)
            .ok()
            .and_then(|d| d.get(b"Rect").ok())
            .and_then(|o| rect_of(&doc.inner, o))
        else {
            continue;
        };
        let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
        if width < 1.0 || height < 1.0 {
            continue; // invisible signature
        }
        let mut lines = vec![
            format!("Digitally signed by {}", p.key.common_name),
            format!("Date: {}", pretty_date(&now)),
        ];
        if let Some(r) = &p.reason {
            lines.push(format!("Reason: {r}"));
        }
        if let Some(l) = &p.location {
            lines.push(format!("Location: {l}"));
        }
        let font = doc.std_font(StdFont::Helvetica);
        let mut res = Resources::default();
        let fname = res.font(doc.fonts[font].obj);
        let widest = lines
            .iter()
            .map(|l| doc.fonts[font].width(l))
            .fold(1.0, f64::max)
            / 1000.0;
        let size = ((width - 8.0) / widest)
            .min((height - 6.0) / (lines.len() as f64 * 1.2))
            .clamp(3.0, 12.0);
        let mut ops = Ops::new();
        ops.q()
            .stroke(&[0.0, 0.0, 0.5])
            .line_width(1.0)
            .rect(0.5, 0.5, width - 1.0, height - 1.0)
            .op("S");
        ops.begin_text().font(&fname, size).fill(&[0.0, 0.0, 0.3]);
        let top = height - 3.0 - doc.fonts[font].ascender() * size / 1000.0;
        for (i, line) in lines.iter().enumerate() {
            let enc = doc.fonts[font].encode(line);
            ops.text_matrix([1.0, 0.0, 0.0, 1.0, 4.0, top - i as f64 * size * 1.2])
                .show(&enc);
        }
        ops.end_text().Q();
        let ap = crate::content::form_xobject(
            &mut doc.inner,
            [0.0, 0.0, width, height],
            None,
            res,
            ops.into_bytes(),
        );
        doc.inner
            .get_dictionary_mut(w)
            .map_err(err("invalid widget"))?
            .set(
                "AP",
                Object::Dictionary(Dictionary::from_iter(vec![("N", Object::Reference(ap))])),
            );
    }
    doc.touch();
    Ok(())
}

fn pretty_date(pdf: &str) -> String {
    let d = pdf.trim_start_matches("D:");
    if d.len() < 14 {
        return pdf.to_string();
    }
    format!(
        "{}-{}-{} {}:{}:{} UTC",
        &d[0..4],
        &d[4..6],
        &d[6..8],
        &d[8..10],
        &d[10..12],
        &d[12..14]
    )
}

/// Patches the byte range, signs the covered bytes and fills /Contents.
pub fn sign(mut pdf: Vec<u8>, p: &Pending) -> Res<Vec<u8>> {
    let br =
        Regex::new(r"/ByteRange\s*\[\s*0\s+9999999999\s+9999999999\s+9999999999\s*\]").unwrap();
    let m = br
        .find_iter(&pdf)
        .last()
        .ok_or("signature placeholder not found in the saved file")?;
    let (br_start, br_end) = (m.start(), m.end());
    let hex_len = CONTENTS_SIZE * 2;
    let needle: Vec<u8> = std::iter::once(b'<')
        .chain(std::iter::repeat_n(b'0', hex_len))
        .chain(std::iter::once(b'>'))
        .collect();
    let contents_start =
        find_last(&pdf, &needle).ok_or("signature /Contents placeholder not found")?;
    let contents_end = contents_start + needle.len();

    let range = [0, contents_start, contents_end, pdf.len() - contents_end];
    let text = format!(
        "/ByteRange [{} {} {} {}]",
        range[0], range[1], range[2], range[3]
    );
    let width = br_end - br_start;
    if text.len() > width {
        return Err("byte range does not fit its placeholder".into());
    }
    let padded = format!(
        "{}{}]",
        &text[..text.len() - 1],
        " ".repeat(width - text.len())
    );
    pdf[br_start..br_end].copy_from_slice(padded.as_bytes());

    let covered = |h: &mut dyn FnMut(&[u8])| {
        h(&pdf[..contents_start]);
        h(&pdf[contents_end..]);
    };
    let cms = match &p.key.key {
        SignKey::P384(_) => {
            let mut h = Sha384::new();
            covered(&mut |b| h.update(b));
            build_cms(&p.key, &h.finalize(), const_oid::db::rfc5912::ID_SHA_384)?
        }
        _ => {
            let mut h = Sha256::new();
            covered(&mut |b| h.update(b));
            build_cms(&p.key, &h.finalize(), const_oid::db::rfc5912::ID_SHA_256)?
        }
    };
    if cms.len() > CONTENTS_SIZE {
        return Err(format!(
            "signature too large ({} bytes, {} reserved)",
            cms.len(),
            CONTENTS_SIZE
        ));
    }
    let hex: String = cms.iter().map(|b| format!("{b:02x}")).collect();
    pdf[contents_start + 1..contents_start + 1 + hex.len()].copy_from_slice(hex.as_bytes());
    Ok(pdf)
}

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn build_cms(
    km: &KeyMaterial,
    digest: &[u8],
    digest_oid: der::oid::ObjectIdentifier,
) -> Res<Vec<u8>> {
    let cert = Certificate::from_der(&km.cert_der).map_err(err("invalid certificate"))?;
    let eci = EncapsulatedContentInfo {
        econtent_type: const_oid::db::rfc5911::ID_DATA,
        econtent: None,
    };
    let sid = SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
        issuer: cert.tbs_certificate.issuer.clone(),
        serial_number: cert.tbs_certificate.serial_number.clone(),
    });
    let digest_alg = AlgorithmIdentifierOwned {
        oid: digest_oid,
        parameters: None,
    };
    let mut builder = SignedDataBuilder::new(&eci);
    builder
        .add_digest_algorithm(digest_alg.clone())
        .map_err(err("CMS"))?;
    builder
        .add_certificate(CertificateChoices::Certificate(cert))
        .map_err(err("CMS"))?;
    for der in &km.chain {
        if let Ok(c) = Certificate::from_der(der) {
            builder
                .add_certificate(CertificateChoices::Certificate(c))
                .map_err(err("CMS"))?;
        }
    }
    let time = create_signing_time_attribute().map_err(err("CMS"))?;
    let signing_cert = signing_certificate_v2(&km.cert_der)?;
    match &km.key {
        SignKey::Rsa(k) => {
            let signer = rsa::pkcs1v15::SigningKey::<Sha256>::new(k.clone());
            let mut sib = SignerInfoBuilder::new(&signer, sid, digest_alg, &eci, Some(digest))
                .map_err(err("CMS"))?;
            sib.add_signed_attribute(time).map_err(err("CMS"))?;
            sib.add_signed_attribute(signing_cert.clone())
                .map_err(err("CMS"))?;
            builder
                .add_signer_info::<_, rsa::pkcs1v15::Signature>(sib)
                .map_err(err("CMS signing"))?;
        }
        SignKey::P256(k) => {
            let mut sib = SignerInfoBuilder::new(k, sid, digest_alg, &eci, Some(digest))
                .map_err(err("CMS"))?;
            sib.add_signed_attribute(time).map_err(err("CMS"))?;
            sib.add_signed_attribute(signing_cert.clone())
                .map_err(err("CMS"))?;
            builder
                .add_signer_info::<_, p256::ecdsa::DerSignature>(sib)
                .map_err(err("CMS signing"))?;
        }
        SignKey::P384(k) => {
            let mut sib = SignerInfoBuilder::new(k, sid, digest_alg, &eci, Some(digest))
                .map_err(err("CMS"))?;
            sib.add_signed_attribute(time).map_err(err("CMS"))?;
            sib.add_signed_attribute(signing_cert.clone())
                .map_err(err("CMS"))?;
            builder
                .add_signer_info::<_, p384::ecdsa::DerSignature>(sib)
                .map_err(err("CMS signing"))?;
        }
    }
    builder
        .build()
        .map_err(err("CMS"))?
        .to_der()
        .map_err(err("CMS encoding"))
}

/// ESS signing-certificate-v2 attribute (RFC 5035), required by PAdES / CAdES baseline:
/// SigningCertificateV2 ::= SEQUENCE { certs SEQUENCE OF ESSCertIDv2 }
/// ESSCertIDv2 ::= SEQUENCE { certHash OCTET STRING }   -- hashAlgorithm defaults to SHA-256
fn signing_certificate_v2(cert_der: &[u8]) -> Res<x509_cert::attr::Attribute> {
    use der::asn1::{Any, SetOfVec};
    let hash = Sha256::digest(cert_der);
    let mut cert_id = vec![0x30, 0x22, 0x04, 0x20];
    cert_id.extend_from_slice(&hash);
    let mut certs = vec![0x30, cert_id.len() as u8];
    certs.extend_from_slice(&cert_id);
    let mut value = vec![0x30, certs.len() as u8];
    value.extend_from_slice(&certs);
    let any = Any::from_der(&value).map_err(err("ESS attribute"))?;
    let mut values = SetOfVec::new();
    values.insert(any).map_err(err("ESS attribute"))?;
    Ok(x509_cert::attr::Attribute {
        oid: der::oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47"),
        values,
    })
}
