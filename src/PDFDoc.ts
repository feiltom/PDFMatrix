import { readFile, writeFile } from 'node:fs/promises';

import { PDFDate } from './Date.js';
import { DigitalSignatureField, DigitalSignatureFieldIterator, VerificationOptions } from './DigitalSignature.js';
import { FDFDoc } from './FDFDoc.js';
import { Field, FieldIterator, createField } from './Field.js';
import { call, PDFNetError } from './errors.js';
import { Rect } from './geom.js';
import { native, type NativeDoc } from './native.js';
import { Page } from './Page.js';
import { SecurityHandler } from './SecurityHandler.js';
import { setKey } from './sdf.js';

/** PDFNet.SDFDoc — only the save flags are meaningful here. */
export class SDFDoc {
  static readonly SaveOptions = {
    e_incremental: 0x01,
    e_remove_unused: 0x02,
    e_hex_strings: 0x04,
    e_omit_xref: 0x08,
    e_linearized: 0x10,
    e_compatibility: 0x20,
  } as const;
}

const LETTER = [0, 0, 612, 792];

export class PageIterator {
  /** @internal */
  constructor(private readonly doc: PDFDoc, private index: number) {}

  async hasNext(): Promise<boolean> {
    return this.index >= 0 && this.index < (await this.doc.getPageCount());
  }

  /** Advances synchronously, so `for (; await it.hasNext(); it.next())` works without awaiting next(). */
  async next(): Promise<void> {
    this.index++;
  }

  async current(): Promise<Page> {
    return this.doc.getPage(this.index + 1);
  }

  /** 1-based number of the current page. */
  async getPageNumber(): Promise<number> {
    return this.index + 1;
  }

  /** @internal */
  position(): number {
    return this.index;
  }
}

export class PDFDocInfo {
  /** @internal */
  constructor(private readonly doc: PDFDoc) {}

  private get(key: string): string {
    return call('PDFDocInfo.get', () => this.doc.native().getInfo(key)) ?? '';
  }

  private set(key: string, value: string | null): void {
    call('PDFDocInfo.set', () => this.doc.native().setInfo(key, value === '' ? null : value));
  }

  async getTitle(): Promise<string> { return this.get('Title'); }
  async setTitle(v: string): Promise<void> { this.set('Title', v); }
  async getAuthor(): Promise<string> { return this.get('Author'); }
  async setAuthor(v: string): Promise<void> { this.set('Author', v); }
  async getSubject(): Promise<string> { return this.get('Subject'); }
  async setSubject(v: string): Promise<void> { this.set('Subject', v); }
  async getKeywords(): Promise<string> { return this.get('Keywords'); }
  async setKeywords(v: string): Promise<void> { this.set('Keywords', v); }
  async getCreator(): Promise<string> { return this.get('Creator'); }
  async setCreator(v: string): Promise<void> { this.set('Creator', v); }
  async getProducer(): Promise<string> { return this.get('Producer'); }
  async setProducer(v: string): Promise<void> { this.set('Producer', v); }

  async getCreationDate(): Promise<PDFDate> {
    return PDFDate.parse(this.get('CreationDate')) ?? new PDFDate();
  }

  async setCreationDate(d: PDFDate): Promise<void> {
    this.set('CreationDate', d.toPDFString());
  }

  async getModDate(): Promise<PDFDate> {
    return PDFDate.parse(this.get('ModDate')) ?? new PDFDate();
  }

  async setModDate(d: PDFDate): Promise<void> {
    this.set('ModDate', d.toPDFString());
  }
}

/**
 * PDFNet.PDFDoc — a PDF document.
 *
 * Encrypted documents that need a password open in a "locked" state, exactly like PDFNet:
 * call initStdSecurityHandler(password) before accessing pages.
 */
export class PDFDoc {
  static readonly InsertFlag = { e_none: 0, e_insert_bookmark: 1 } as const;

  #native: NativeDoc | null;
  #lockedBytes: Buffer | null = null;
  #modified = false;

  private constructor(doc: NativeDoc | null, lockedBytes: Buffer | null = null) {
    this.#native = doc;
    this.#lockedBytes = lockedBytes;
  }

  // ---------------------------------------------------------- creation

  static async create(): Promise<PDFDoc> {
    return new PDFDoc(native.NativeDoc.create());
  }

  static async createFromFilePath(path: string): Promise<PDFDoc> {
    let bytes: Buffer;
    try {
      bytes = await readFile(path);
    } catch (e) {
      throw new PDFNetError(`cannot read file ${path}`, 'PDFDoc.createFromFilePath', { cause: e });
    }
    return PDFDoc.fromBytes(bytes, 'PDFDoc.createFromFilePath');
  }

  static async createFromUFilePath(path: string): Promise<PDFDoc> {
    return PDFDoc.createFromFilePath(path);
  }

  static async createFromBuffer(buf: ArrayBuffer | ArrayBufferView): Promise<PDFDoc> {
    const bytes = ArrayBuffer.isView(buf)
      ? Buffer.from(buf.buffer, buf.byteOffset, buf.byteLength)
      : Buffer.from(buf);
    return PDFDoc.fromBytes(bytes, 'PDFDoc.createFromBuffer');
  }

  private static fromBytes(bytes: Buffer, location: string): PDFDoc {
    try {
      return new PDFDoc(native.NativeDoc.open(bytes));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (/encrypted/i.test(msg)) return new PDFDoc(null, bytes);
      throw new PDFNetError(msg, location, { cause: e });
    }
  }

  // ---------------------------------------------------------- internals

  /** @internal */
  native(): NativeDoc {
    if (this.#native) return this.#native;
    if (this.#lockedBytes) {
      throw new PDFNetError('document is encrypted: call initStdSecurityHandler(password) first', 'PDFDoc');
    }
    throw new PDFNetError('document is closed', 'PDFDoc');
  }

  /** @internal */
  isOpen(): boolean {
    return this.#native !== null;
  }

  private mutate<T>(location: string, fn: (d: NativeDoc) => T): T {
    const out = call(location, () => fn(this.native()));
    this.#modified = true;
    return out;
  }

  // ---------------------------------------------------------- security (decryption)

  /** Opens the document with the default (empty) user password. */
  async initSecurityHandler(): Promise<boolean> {
    if (this.#native) return true;
    return this.tryUnlock('');
  }

  async initStdSecurityHandler(password: string): Promise<boolean> {
    if (this.#native) return true;
    return this.tryUnlock(password);
  }

  async initStdSecurityHandlerUString(password: string): Promise<boolean> {
    return this.initStdSecurityHandler(password);
  }

  async initStdSecurityHandlerBuffer(password: ArrayBufferView): Promise<boolean> {
    return this.initStdSecurityHandler(Buffer.from(password.buffer, password.byteOffset, password.byteLength).toString('latin1'));
  }

  private tryUnlock(password: string): boolean {
    if (!this.#lockedBytes) return false;
    try {
      this.#native = native.NativeDoc.open(this.#lockedBytes, password);
      this.#lockedBytes = null;
      return true;
    } catch {
      return false;
    }
  }

  /** True when the source file was encrypted (security is currently kept in memory only). */
  async isEncrypted(): Promise<boolean> {
    return this.#lockedBytes !== null || (this.#native?.wasEncrypted() ?? false);
  }

  // ---------------------------------------------------------- pages

  async getPageCount(): Promise<number> {
    return call('PDFDoc.getPageCount', () => this.native().pageCount());
  }

  /** 1-based page access. */
  async getPage(pageNumber: number): Promise<Page> {
    const ref = call('PDFDoc.getPage', () => this.native().pageRef(pageNumber - 1));
    return new Page(this, ref);
  }

  async getPageIterator(pageNumber = 1): Promise<PageIterator> {
    return new PageIterator(this, pageNumber - 1);
  }

  /** Creates a new, empty page (not yet in the page sequence). Defaults to US Letter. */
  async pageCreate(mediaBox?: Rect): Promise<Page> {
    const box = mediaBox ? mediaBox.toArray() : LETTER;
    return new Page(this, this.mutate('PDFDoc.pageCreate', (d) => d.newPage(box)));
  }

  async pagePushBack(page: Page): Promise<void> {
    this.placePage(page, call('PDFDoc.pagePushBack', () => this.native().pageCount()), 'PDFDoc.pagePushBack');
  }

  async pagePushFront(page: Page): Promise<void> {
    this.placePage(page, 0, 'PDFDoc.pagePushFront');
  }

  /** Inserts `page` before the page the iterator points to. */
  async pageInsert(where: PageIterator, page: Page): Promise<void> {
    this.placePage(page, where.position(), 'PDFDoc.pageInsert');
  }

  async pageRemove(where: PageIterator): Promise<void> {
    this.mutate('PDFDoc.pageRemove', (d) => d.removePage(where.position()));
  }

  /**
   * Puts `page` at `index`. A detached page of this document is inserted as is; a page that is
   * already in this document, or a page of another document, is copied (PDFNet semantics).
   */
  private placePage(page: Page, index: number, location: string): void {
    if (page.doc === this) {
      if (page.index0() < 0) {
        this.mutate(location, (d) => d.insertPage(page.ref, index));
      } else {
        this.mutate(location, (d) => d.duplicatePages([page.ref], index));
      }
      return;
    }
    const src = page.doc.native();
    this.mutate(location, (d) => d.importPages(src, [page.ref], index));
  }

  /**
   * Copies pages [startPage, endPage] (1-based, inclusive) of `srcDoc` before page
   * `insertBeforePageNumber` of this document (≤ 1: at the front, > count: appended).
   */
  async insertPages(
    insertBeforePageNumber: number,
    srcDoc: PDFDoc,
    startPage: number,
    endPage: number,
    _flag: number = PDFDoc.InsertFlag.e_none,
  ): Promise<void> {
    const refs = srcDoc.pageRange(startPage, endPage, 'PDFDoc.insertPages');
    const at = Math.max(0, insertBeforePageNumber - 1);
    if (srcDoc === this) {
      this.mutate('PDFDoc.insertPages', (d) => d.duplicatePages(refs, at));
    } else {
      const src = srcDoc.native();
      this.mutate('PDFDoc.insertPages', (d) => d.importPages(src, refs, at));
    }
  }

  /** Same as insertPages, then removes the pages from the source document. */
  async movePages(
    moveBeforePageNumber: number,
    srcDoc: PDFDoc,
    startPage: number,
    endPage: number,
    _flag: number = PDFDoc.InsertFlag.e_none,
  ): Promise<void> {
    if (srcDoc === this) {
      const refs = this.pageRange(startPage, endPage, 'PDFDoc.movePages');
      const target = Math.max(0, moveBeforePageNumber - 1);
      // Pages removed before the target shift it to the left.
      const shift = refs.filter((_, i) => startPage - 1 + i < target).length;
      for (let p = endPage; p >= startPage; p--) {
        this.mutate('PDFDoc.movePages', (d) => d.removePage(p - 1));
      }
      refs.forEach((ref, i) => this.mutate('PDFDoc.movePages', (d) => d.insertPage(ref, target - shift + i)));
      return;
    }
    await this.insertPages(moveBeforePageNumber, srcDoc, startPage, endPage);
    for (let p = endPage; p >= startPage; p--) {
      srcDoc.mutate('PDFDoc.movePages', (d) => d.removePage(p - 1));
    }
  }

  /** @internal */
  pageRange(start: number, end: number, location: string) {
    const count = call(location, () => this.native().pageCount());
    if (start < 1 || end > count || start > end) {
      throw new PDFNetError(`invalid page range ${start}-${end} (document has ${count} pages)`, location);
    }
    const all = call(location, () => this.native().pageRefs());
    return all.slice(start - 1, end);
  }

  // ---------------------------------------------------------- metadata

  async getDocInfo(): Promise<PDFDocInfo> {
    this.native();
    return new PDFDocInfo(this);
  }

  /** PDF header version, e.g. "1.7". */
  async getVersion(): Promise<string> {
    return this.native().version();
  }

  async isModified(): Promise<boolean> {
    return this.#modified;
  }

  // ---------------------------------------------------------- saving

  async saveMemoryBuffer(flags: number = 0): Promise<Uint8Array> {
    const O = SDFDoc.SaveOptions;
    const buf = call('PDFDoc.save', () =>
      this.native().save({
        removeUnused: (flags & O.e_remove_unused) !== 0,
        objectStreams: (flags & O.e_compatibility) === 0 && (flags & O.e_linearized) === 0,
        incremental: (flags & O.e_incremental) !== 0,
        linearize: (flags & O.e_linearized) !== 0,
      }),
    );
    this.#modified = false;
    return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  }

  async save(path: string, flags: number = 0): Promise<void> {
    const bytes = await this.saveMemoryBuffer(flags);
    try {
      await writeFile(path, bytes);
    } catch (e) {
      throw new PDFNetError(`cannot write file ${path}`, 'PDFDoc.save', { cause: e });
    }
  }

  // ---------------------------------------------------------- forms & annotations

  /** Iterates over terminal fields; with `fieldName`, starts at that field. */
  async getFieldIterator(fieldName?: string): Promise<FieldIterator> {
    const all = call('PDFDoc.getFieldIterator', () => this.native().fields());
    const start = fieldName ? Math.max(0, all.findIndex((f) => f.name === fieldName)) : 0;
    const refs = fieldName && !all.some((f) => f.name === fieldName) ? [] : all.slice(start).map((f) => f.obj);
    return new FieldIterator(this, refs);
  }

  /** The field with this fully qualified name; check isValid() when it may not exist. */
  async getField(fieldName: string): Promise<Field> {
    const f = call('PDFDoc.getField', () => this.native().fields()).find((x) => x.name === fieldName);
    return new Field(this, f?.obj ?? null);
  }

  /** Creates a field (add widgets with WidgetAnnot.create + page.annotPushBack). */
  async fieldCreate(fieldName: string, type: number, fieldValue?: string): Promise<Field> {
    const field = createField(this, fieldName, type);
    this.#modified = true;
    if (fieldValue !== undefined && fieldValue !== '') {
      const info = field.info();
      setKey(this, info.obj, 'V', type === Field.Type.e_check || type === Field.Type.e_radio ? { $name: fieldValue } : fieldValue);
    }
    return field;
  }

  /** Regenerates the appearance of every form field. */
  async refreshFieldAppearances(): Promise<void> {
    this.mutate('PDFDoc.refreshFieldAppearances', (d) => d.refreshFieldAppearances());
  }

  /** Burns annotations into page content: only form fields when `formsOnly`. */
  async flattenAnnotations(formsOnly = false): Promise<void> {
    this.mutate('PDFDoc.flattenAnnotations', (d) => d.flattenAnnotations(formsOnly));
  }

  /** Applies (burns in) every RedactionAnnot of the document, then removes them. */
  async applyRedactions(): Promise<{ glyphs: number; images: number; paths: number; annotations: number }> {
    return this.mutate('PDFDoc.applyRedactions', (d) => d.applyRedactAnnotations());
  }

  /**
   * Imports annotations (and form field values, command XFDF) from an XFDF string, as exported
   * by WebViewer or Acrobat. Annotations with the same name are replaced; appearances are
   * generated so that they display in any viewer.
   */
  async mergeXFDFString(xfdf: string, _opts?: unknown): Promise<void> {
    this.mutate('PDFDoc.mergeXFDFString', (d) => d.mergeXfdf(xfdf));
  }

  async mergeXFDF(fdf: FDFDoc | string, opts?: unknown): Promise<void> {
    return this.mergeXFDFString(typeof fdf === 'string' ? fdf : fdf.xfdf, opts);
  }

  async fdfMerge(fdf: FDFDoc): Promise<void> {
    return this.mergeXFDFString(fdf.xfdf);
  }

  async hasSignatures(): Promise<boolean> {
    return call('PDFDoc.hasSignatures', () => this.native().fields()).some((f) => f.kind === 'signature');
  }

  // ---------------------------------------------------------- security & signatures

  static readonly SignaturesVerificationStatus = {
    e_unsigned: 0,
    e_failure: 1,
    e_untrusted: 2,
    e_unsupported: 3,
    e_verified: 4,
  } as const;

  /** Security of the opened document, or null when it is not encrypted. */
  async getSecurityHandler(): Promise<SecurityHandler | null> {
    const i = call('PDFDoc.getSecurityHandler', () => this.native().securityInfo());
    return i.encrypted ? new SecurityHandler(i.algorithm, i.keyLength, i.permissions, true) : null;
  }

  /** Encrypts the document with `handler` on the next full save. */
  async setSecurityHandler(handler: SecurityHandler): Promise<void> {
    this.mutate('PDFDoc.setSecurityHandler', (d) => d.setEncryption(handler.toNative()));
  }

  /** Saves the document without encryption from now on. */
  async removeSecurity(): Promise<void> {
    this.mutate('PDFDoc.removeSecurity', (d) => d.removeSecurity());
  }

  async createDigitalSignatureField(name = ''): Promise<DigitalSignatureField> {
    const count = call('PDFDoc.createDigitalSignatureField', () => this.native().fields()).length;
    const field = createField(this, name || `Signature${count + 1}`, Field.Type.e_signature);
    this.#modified = true;
    return new DigitalSignatureField(this, field.info().obj);
  }

  async getDigitalSignatureFieldIterator(): Promise<DigitalSignatureFieldIterator> {
    const refs = call('PDFDoc.getDigitalSignatureFieldIterator', () => this.native().fields())
      .filter((f) => f.kind === 'signature')
      .map((f) => f.obj);
    return new DigitalSignatureFieldIterator(this, refs);
  }

  /** Verifies every signed signature field (against the last loaded or saved bytes). */
  async verifySignedDigitalSignatures(opts?: VerificationOptions): Promise<number> {
    const o = opts ?? (await VerificationOptions.create());
    const S = PDFDoc.SignaturesVerificationStatus;
    const signed = call('PDFDoc.verifySignedDigitalSignatures', () => this.native().verifySignatures(o.toNative())).filter((r) => r.signed);
    if (signed.length === 0) return S.e_unsigned;
    if (signed.some((r) => r.documentStatus !== 0 || r.digestStatus === 0)) return S.e_failure;
    if (signed.some((r) => r.digestStatus === 5)) return S.e_unsupported;
    if (signed.some((r) => !r.verified)) return S.e_untrusted;
    return S.e_verified;
  }

  // ---------------------------------------------------------- lifecycle (no-ops kept for API parity)

  async lock(): Promise<void> {}
  async unlock(): Promise<void> {}
  async lockRead(): Promise<void> {}
  async unlockRead(): Promise<void> {}
  async tryLock(): Promise<boolean> { return true; }
  async tryLockRead(): Promise<boolean> { return true; }

  async close(): Promise<void> {
    this.#native = null;
    this.#lockedBytes = null;
  }

  async destroy(): Promise<void> {
    return this.close();
  }
}
