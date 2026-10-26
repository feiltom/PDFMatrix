import { readFile } from 'node:fs/promises';

import { PDFDate } from './Date.js';
import { call, PDFNetError } from './errors.js';
import { Field } from './Field.js';
import type { ObjRef } from './native.js';
import type { PDFDoc } from './PDFDoc.js';
import { asName, asRef, getDict } from './sdf.js';

enum SubFilterType {
  e_adbe_x509_rsa_sha1 = 0,
  e_adbe_pkcs7_detached = 1,
  e_adbe_pkcs7_sha1 = 2,
  e_ETSI_CAdES_detached = 3,
  e_ETSI_RFC3161 = 4,
  e_unknown = 5,
  e_absent = 6,
}

const SUBFILTERS: Record<string, SubFilterType> = {
  'adbe.x509.rsa_sha1': SubFilterType.e_adbe_x509_rsa_sha1,
  'adbe.pkcs7.detached': SubFilterType.e_adbe_pkcs7_detached,
  'adbe.pkcs7.sha1': SubFilterType.e_adbe_pkcs7_sha1,
  'ETSI.CAdES.detached': SubFilterType.e_ETSI_CAdES_detached,
  'ETSI.RFC3161': SubFilterType.e_ETSI_RFC3161,
};

enum DocumentStatus {
  e_no_error = 0,
  e_corrupt_file = 1,
  e_unsigned = 2,
  e_bad_byteranges = 3,
  e_corrupt_cryptographic_contents = 4,
}

enum DigestStatus {
  e_digest_invalid = 0,
  e_digest_verified = 1,
  e_digest_verification_disabled = 2,
  e_weak_digest_algorithm_but_digest_verifiable = 3,
  e_no_digest_status = 4,
  e_unsupported_encoding = 5,
}

enum TrustStatus {
  e_trust_verified = 0,
  e_untrusted = 1,
  e_trust_verification_disabled = 2,
  e_no_trust_status = 3,
}

enum ModificationPermissionsStatus {
  e_invalidated_by_disallowed_changes = 0,
  e_has_allowed_changes = 1,
  e_unmodified = 2,
  e_permissions_verification_disabled = 3,
  e_no_permissions_status = 4,
}

type Report = ReturnType<ReturnType<PDFDoc['native']>['verifySignatures']>[number];

/** Decodes DER, PEM (one or several certificates) or base64 into DER buffers. */
function toDer(data: Uint8Array | string): Buffer[] {
  const buf = typeof data === 'string' ? Buffer.from(data, 'utf8') : Buffer.from(data);
  const text = buf.toString('latin1');
  if (text.includes('-----BEGIN CERTIFICATE-----')) {
    return [...text.matchAll(/-----BEGIN CERTIFICATE-----([\s\S]*?)-----END CERTIFICATE-----/g)].map((m) =>
      Buffer.from(m[1].replace(/\s+/g, ''), 'base64'),
    );
  }
  return [buf];
}

/** PDFNet.VerificationOptions */
export class VerificationOptions {
  static readonly SecurityLevel = { e_compatibility_and_archiving: 0, e_maximum: 1 } as const;
  static readonly CertificateTrustFlag = { e_signing_trust: 1, e_certification_trust: 2, e_default_trust: 1 } as const;

  /** @internal */
  readonly trusted: Buffer[] = [];
  /** @internal */
  checkTrust = true;

  static async create(_level = 0): Promise<VerificationOptions> {
    return new VerificationOptions();
  }

  /** Trusts a certificate (DER or PEM bytes). */
  async addTrustedCertificate(cert: Uint8Array | string, _trustFlags = 1): Promise<void> {
    this.trusted.push(...toDer(cert));
  }

  /** Trusts every certificate of a DER/PEM file. */
  async addTrustedCertificateUString(path: string, _trustFlags = 1): Promise<void> {
    try {
      this.trusted.push(...toDer(await readFile(path)));
    } catch (e) {
      throw new PDFNetError(`cannot read certificate ${path}`, 'VerificationOptions.addTrustedCertificate', { cause: e });
    }
  }

  async addTrustedCertificates(certs: Uint8Array): Promise<void> {
    this.trusted.push(...toDer(certs));
  }

  /** Disables (false) or enables trust verification. */
  async enableTrustVerification(enable: boolean): Promise<void> {
    this.checkTrust = enable;
  }

  /** @internal */
  toNative() {
    return { trustedCertificates: this.trusted, checkTrust: this.checkTrust };
  }
}

/** PDFNet.VerificationResult */
export class VerificationResult {
  static readonly DocumentStatus = DocumentStatus;
  static readonly DigestStatus = DigestStatus;
  static readonly TrustStatus = TrustStatus;
  static readonly ModificationPermissionsStatus = ModificationPermissionsStatus;

  /** @internal */
  constructor(private readonly r: Report) {}

  async getVerificationStatus(): Promise<boolean> {
    return this.r.verified;
  }

  async getDocumentStatus(): Promise<DocumentStatus> {
    return this.r.documentStatus;
  }

  async getDigestStatus(): Promise<DigestStatus> {
    return this.r.digestStatus;
  }

  async getTrustStatus(): Promise<TrustStatus> {
    return this.r.trustStatus;
  }

  async getPermissionsStatus(): Promise<ModificationPermissionsStatus> {
    return this.r.permissionsStatus;
  }

  async getDigestAlgorithm(): Promise<string> {
    return this.r.digestAlgorithm;
  }

  async getDocumentStatusAsString(): Promise<string> {
    return DocumentStatus[this.r.documentStatus];
  }

  async getDigestStatusAsString(): Promise<string> {
    return DigestStatus[this.r.digestStatus];
  }

  async getTrustStatusAsString(): Promise<string> {
    return TrustStatus[this.r.trustStatus];
  }

  async getPermissionsStatusAsString(): Promise<string> {
    return ModificationPermissionsStatus[this.r.permissionsStatus];
  }

  /** Human-readable explanation of a failure ('' when verified). */
  async getMessage(): Promise<string> {
    return this.r.message;
  }

  /** DER of the signer certificate. */
  async getSignerCertificate(): Promise<Uint8Array | null> {
    return this.r.certificates[0] ?? null;
  }

  /** DER of every certificate embedded in the signature (signer first). */
  async getCertificates(): Promise<Uint8Array[]> {
    return this.r.certificates;
  }
}

/** PDFNet.DigitalSignatureField — a signature form field. */
export class DigitalSignatureField {
  static readonly SubFilterType = SubFilterType;

  #reason?: string;
  #location?: string;
  #contactInfo?: string;
  #subfilter = 'adbe.pkcs7.detached';

  /** @internal */
  constructor(readonly doc: PDFDoc, readonly ref: ObjRef) {}

  static async createFromField(field: Field): Promise<DigitalSignatureField> {
    if ((await field.getType()) !== Field.Type.e_signature) {
      throw new PDFNetError('not a signature field', 'DigitalSignatureField.createFromField');
    }
    return new DigitalSignatureField(field.doc, field.info().obj);
  }

  async getField(): Promise<Field> {
    return new Field(this.doc, this.ref);
  }

  private sig(): Record<string, unknown> | null {
    const v = asRef(getDict(this.doc, this.ref).V);
    return v ? getDict(this.doc, v) : null;
  }

  private str(key: string): string {
    const v = this.sig()?.[key];
    return typeof v === 'string' ? v : '';
  }

  async hasCryptographicSignature(): Promise<boolean> {
    return this.sig() !== null;
  }

  async getSignatureName(): Promise<string> {
    return this.str('Name');
  }

  async getReason(): Promise<string> {
    return this.str('Reason');
  }

  async getLocation(): Promise<string> {
    return this.str('Location');
  }

  async getContactInfo(): Promise<string> {
    return this.str('ContactInfo');
  }

  async getSigningTime(): Promise<PDFDate> {
    return PDFDate.parse(this.str('M')) ?? new PDFDate();
  }

  async getSubFilter(): Promise<SubFilterType> {
    const s = this.sig();
    if (!s) return SubFilterType.e_absent;
    return SUBFILTERS[asName(s.SubFilter as never) ?? ''] ?? SubFilterType.e_unknown;
  }

  async getByteRanges(): Promise<{ offset: number; length: number }[]> {
    const br = (this.sig()?.ByteRange as number[] | undefined) ?? [];
    const out = [];
    for (let i = 0; i + 1 < br.length; i += 2) out.push({ offset: br[i], length: br[i + 1] });
    return out;
  }

  async setReason(reason: string): Promise<void> {
    this.#reason = reason;
  }

  async setLocation(location: string): Promise<void> {
    this.#location = location;
  }

  async setContactInfo(info: string): Promise<void> {
    this.#contactInfo = info;
  }

  /** e_adbe_pkcs7_detached (default) or e_ETSI_CAdES_detached. */
  async setSubFilter(type: SubFilterType): Promise<void> {
    this.#subfilter = type === SubFilterType.e_ETSI_CAdES_detached ? 'ETSI.CAdES.detached' : 'adbe.pkcs7.detached';
  }

  /** Signs with a PKCS#12 file when the document is next saved (use e_incremental). */
  async signOnNextSave(pkcs12Path: string, password: string): Promise<void> {
    let data: Buffer;
    try {
      data = await readFile(pkcs12Path);
    } catch (e) {
      throw new PDFNetError(`cannot read ${pkcs12Path}`, 'DigitalSignatureField.signOnNextSave', { cause: e });
    }
    await this.signOnNextSaveFromBuffer(data, password);
  }

  async signOnNextSaveFromBuffer(pkcs12: Uint8Array, password: string): Promise<void> {
    call('DigitalSignatureField.signOnNextSave', () =>
      this.doc.native().signOnNextSave(this.ref, Buffer.from(pkcs12), password, {
        reason: this.#reason,
        location: this.#location,
        contactInfo: this.#contactInfo,
        subfilter: this.#subfilter,
      }),
    );
  }

  /** Verifies this signature against the last loaded or saved bytes. */
  async verify(opts?: VerificationOptions): Promise<VerificationResult> {
    const o = opts ?? (await VerificationOptions.create());
    const reports = call('DigitalSignatureField.verify', () => this.doc.native().verifySignatures(o.toNative()));
    const r = reports.find((x) => x.field && x.field.num === this.ref.num && x.field.generation === this.ref.generation);
    if (!r) throw new PDFNetError('signature field not found in the saved document', 'DigitalSignatureField.verify');
    return new VerificationResult(r);
  }

  async isLockedByDigitalSignature(): Promise<boolean> {
    return false;
  }
}

export class DigitalSignatureFieldIterator {
  #i = 0;

  /** @internal */
  constructor(private readonly doc: PDFDoc, private readonly refs: ObjRef[]) {}

  async hasNext(): Promise<boolean> {
    return this.#i < this.refs.length;
  }

  async next(): Promise<void> {
    this.#i++;
  }

  async current(): Promise<DigitalSignatureField> {
    return new DigitalSignatureField(this.doc, this.refs[this.#i]);
  }
}
