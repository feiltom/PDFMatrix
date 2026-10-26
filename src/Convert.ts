import { readFile, writeFile } from 'node:fs/promises';
import { basename, extname } from 'node:path';

import { call, PDFNetError } from './errors.js';
import { native } from './native.js';
import { PDFDoc } from './PDFDoc.js';

const DEFAULT_GOTENBERG = 'http://localhost:3000';
const IMAGE_EXT = new Set(['.png', '.jpg', '.jpeg', '.gif', '.bmp', '.tif', '.tiff', '.webp', '.jp2', '.j2k', '.jpx']);
const HTML_EXT = new Set(['.html', '.htm']);

function gotenbergUrl(explicit?: string): string {
  return (explicit || process.env.GOTENBERG_URL || DEFAULT_GOTENBERG).replace(/\/+$/, '');
}

/** Options of Office → PDF conversion. Office conversion is delegated to a Gotenberg service. */
export class OfficeToPDFOptions {
  /** @internal */
  gotenberg?: string;
  /** @internal */
  fields: Record<string, string> = {};
  /** @internal */
  timeoutMs = 120_000;
  /** @internal */
  fileName?: string;

  /** Base URL of the Gotenberg service (default: $GOTENBERG_URL or http://localhost:3000). */
  setGotenbergUrl(url: string): this {
    this.gotenberg = url;
    return this;
  }

  setLandscape(landscape: boolean): this {
    this.fields.landscape = String(landscape);
    return this;
  }

  /** Pages to export, e.g. "1-3,5". */
  setPageRanges(ranges: string): this {
    this.fields.nativePageRanges = ranges;
    return this;
  }

  /** Password of a protected Office document. */
  setPassword(password: string): this {
    this.fields.password = password;
    return this;
  }

  /** Export form fields as PDF form fields (default true). */
  setExportFormFields(exportFields: boolean): this {
    this.fields.exportFormFields = String(exportFields);
    return this;
  }

  /** Name (with extension) to use for in-memory input, e.g. "report.xlsx" (detected otherwise). */
  setFileName(name: string): this {
    this.fileName = name;
    return this;
  }

  setTimeout(ms: number): this {
    this.timeoutMs = ms;
    return this;
  }

  // Accepted for API parity with PDFNet; LibreOffice decides these.
  setLocale(_locale: string): this {
    return this;
  }

  setApplyPageBreaksToSheet(_apply: boolean): this {
    return this;
  }

  setDisplayChangeTracking(_display: boolean): this {
    return this;
  }
}

type Input = string | Uint8Array;

/** Guesses a file name (extension) for in-memory documents, so LibreOffice can pick a filter. */
export function guessFileName(data: Uint8Array): string {
  const head = Buffer.from(data.subarray(0, 8)).toString('latin1');
  if (head.startsWith('%PDF')) return 'document.pdf';
  if (head.startsWith('{\\rtf')) return 'document.rtf';
  if (data[0] === 0xd0 && data[1] === 0xcf && data[2] === 0x11 && data[3] === 0xe0) return 'document.doc';
  if (head.startsWith('PK')) {
    // Office Open XML and OpenDocument packages are ZIP files; look at the entry names.
    const names = Buffer.from(data).toString('latin1');
    if (names.includes('word/')) return 'document.docx';
    if (names.includes('xl/')) return 'document.xlsx';
    if (names.includes('ppt/')) return 'document.pptx';
    const odf = /application\/vnd\.oasis\.opendocument\.(text|spreadsheet|presentation)/.exec(names);
    if (odf) return `document.${{ text: 'odt', spreadsheet: 'ods', presentation: 'odp' }[odf[1] as 'text']}`;
  }
  if (head.trimStart().startsWith('<')) return 'document.html';
  return 'document.txt';
}

async function readInput(input: Input, location: string, fileName?: string): Promise<{ data: Uint8Array; name: string }> {
  if (typeof input !== 'string') return { data: input, name: fileName ?? guessFileName(input) };
  try {
    return { data: await readFile(input), name: basename(input) };
  } catch (e) {
    throw new PDFNetError(`cannot read ${input}`, location, { cause: e });
  }
}

/** POSTs files to a Gotenberg route and returns the produced PDF. */
async function gotenberg(
  route: string,
  files: { name: string; data: Uint8Array }[],
  fields: Record<string, string>,
  base: string | undefined,
  timeoutMs: number,
  location: string,
): Promise<Uint8Array> {
  const url = `${gotenbergUrl(base)}${route}`;
  const form = new FormData();
  for (const f of files) form.append('files', new Blob([Buffer.from(f.data)]), f.name);
  for (const [k, v] of Object.entries(fields)) form.append(k, v);
  let res: Response;
  try {
    res = await fetch(url, { method: 'POST', body: form, signal: AbortSignal.timeout(timeoutMs) });
  } catch (e) {
    throw new PDFNetError(
      `conversion service unreachable at ${gotenbergUrl(base)} (start it with \`make gotenberg\` or set GOTENBERG_URL)`,
      location,
      { cause: e },
    );
  }
  if (!res.ok) {
    const detail = (await res.text().catch(() => '')).slice(0, 300);
    throw new PDFNetError(`conversion failed (HTTP ${res.status}): ${detail}`, location);
  }
  return new Uint8Array(await res.arrayBuffer());
}

/** Appends every page of a PDF (bytes) to `doc`. */
async function appendPdf(doc: PDFDoc, pdf: Uint8Array): Promise<void> {
  const src = await PDFDoc.createFromBuffer(pdf);
  await doc.insertPages((await doc.getPageCount()) + 1, src, 1, await src.getPageCount());
}

/**
 * PDFNet.Convert — conversion to PDF.
 * PDFs and images are converted locally (Rust core); Office documents, text and HTML go through
 * a Gotenberg service (LibreOffice / Chromium) running in its own container.
 */
export class Convert {
  static readonly OfficeToPDFOptions = OfficeToPDFOptions;

  /** Converts an Office document (docx, xlsx, pptx, odt, rtf, txt, csv…) and appends it to `doc`. */
  static async officeToPdf(doc: PDFDoc, input: Input, options?: OfficeToPDFOptions | null): Promise<void> {
    const o = options ?? new OfficeToPDFOptions();
    const { data, name } = await readInput(input, 'Convert.officeToPdf', o.fileName);
    const pdf = await gotenberg('/forms/libreoffice/convert', [{ name, data }], o.fields, o.gotenberg, o.timeoutMs, 'Convert.officeToPdf');
    await appendPdf(doc, pdf);
  }

  /** Same as officeToPdf, returning a new document. */
  static async officeToPdfDoc(input: Input, options?: OfficeToPDFOptions | null): Promise<PDFDoc> {
    const doc = await PDFDoc.create();
    await Convert.officeToPdf(doc, input, options);
    return doc;
  }

  /**
   * Converts any supported file and appends it to `doc`: PDF (pages copied), images (one page
   * per image, at 96 dpi), HTML (Chromium), everything else (LibreOffice).
   */
  static async toPdf(doc: PDFDoc, inputPath: string, options?: OfficeToPDFOptions | null): Promise<void> {
    const ext = extname(inputPath).toLowerCase();
    if (ext === '.pdf') {
      const { data } = await readInput(inputPath, 'Convert.toPdf');
      return appendPdf(doc, data);
    }
    if (IMAGE_EXT.has(ext)) {
      const { data } = await readInput(inputPath, 'Convert.toPdf');
      call('Convert.toPdf', () => doc.native().addImagePage(Buffer.from(data), 96));
      return;
    }
    if (HTML_EXT.has(ext)) {
      const h = await HTML2PDF.create();
      if (options?.gotenberg) h.setGotenbergUrl(options.gotenberg);
      await h.insertFromHtmlString(new TextDecoder().decode((await readInput(inputPath, 'Convert.toPdf')).data));
      await h.convert(doc);
      return;
    }
    return Convert.officeToPdf(doc, inputPath, options);
  }

  /** Loads a .docx template (Apryse DocGen syntax: {{values}}, loops, conditions, images). */
  static async createOfficeTemplateWithPath(path: string, options?: OfficeToPDFOptions | null): Promise<TemplateDocument> {
    const { data } = await readInput(path, 'Convert.createOfficeTemplateWithPath');
    return createTemplate(data, options, 'Convert.createOfficeTemplateWithPath');
  }

  static async createOfficeTemplateWithBuffer(buf: Uint8Array, options?: OfficeToPDFOptions | null): Promise<TemplateDocument> {
    return createTemplate(buf, options, 'Convert.createOfficeTemplateWithBuffer');
  }

  /** Appends one page per image (PNG, JPEG, GIF, BMP, TIFF, WebP). */
  static async fromImage(doc: PDFDoc, image: Input, dpi = 96): Promise<void> {
    const { data } = await readInput(image, 'Convert.fromImage');
    call('Convert.fromImage', () => doc.native().addImagePage(Buffer.from(data), dpi));
  }
}

/** PDFNet.HTML2PDF — HTML/URL to PDF through Gotenberg's Chromium module. */
export class HTML2PDF {
  #items: ({ kind: 'url'; url: string } | { kind: 'html'; html: string })[] = [];
  #gotenberg?: string;
  #fields: Record<string, string> = {};
  #timeoutMs = 120_000;

  static async create(): Promise<HTML2PDF> {
    return new HTML2PDF();
  }

  setGotenbergUrl(url: string): this {
    this.#gotenberg = url;
    return this;
  }

  /** Paper size in inches (Gotenberg/Chromium units). */
  async setPaperSize(widthInches: number, heightInches: number): Promise<void> {
    this.#fields.paperWidth = String(widthInches);
    this.#fields.paperHeight = String(heightInches);
  }

  async setLandscape(landscape: boolean): Promise<void> {
    this.#fields.landscape = String(landscape);
  }

  /** Margins in inches. */
  async setMargins(top: number, bottom: number, left: number, right: number): Promise<void> {
    Object.assign(this.#fields, { marginTop: String(top), marginBottom: String(bottom), marginLeft: String(left), marginRight: String(right) });
  }

  async setPrintBackground(print: boolean): Promise<void> {
    this.#fields.printBackground = String(print);
  }

  async insertFromUrl(url: string): Promise<void> {
    this.#items.push({ kind: 'url', url });
  }

  async insertFromHtmlString(html: string): Promise<void> {
    this.#items.push({ kind: 'html', html });
  }

  /** Converts every inserted item, in order, and appends the pages to `doc`. */
  async convert(doc: PDFDoc): Promise<boolean> {
    for (const item of this.#items) {
      const pdf =
        item.kind === 'url'
          ? await gotenberg('/forms/chromium/convert/url', [], { ...this.#fields, url: item.url }, this.#gotenberg, this.#timeoutMs, 'HTML2PDF.convert')
          : await gotenberg(
              '/forms/chromium/convert/html',
              [{ name: 'index.html', data: new TextEncoder().encode(item.html) }],
              this.#fields,
              this.#gotenberg,
              this.#timeoutMs,
              'HTML2PDF.convert',
            );
      await appendPdf(doc, pdf);
    }
    return true;
  }
}

// ============================================================================ DocGen

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

/** Replaces every { image_url } value by the same object carrying the image bytes (base64). */
async function resolveImages(v: Json, location: string): Promise<Json> {
  if (Array.isArray(v)) return Promise.all(v.map((x) => resolveImages(x, location)));
  if (!v || typeof v !== 'object') return v;
  const out: { [k: string]: Json } = {};
  for (const [k, x] of Object.entries(v)) out[k] = await resolveImages(x, location);
  const url = v.image_url;
  if (typeof url === 'string' && url) {
    let bytes: Uint8Array;
    if (url.startsWith('data:')) {
      bytes = Buffer.from(url.slice(url.indexOf(',') + 1), 'base64');
    } else if (/^https?:\/\//i.test(url)) {
      const res = await fetch(url).catch((e) => {
        throw new PDFNetError(`cannot download image ${url}`, location, { cause: e });
      });
      if (!res.ok) throw new PDFNetError(`cannot download image ${url} (HTTP ${res.status})`, location);
      bytes = new Uint8Array(await res.arrayBuffer());
    } else {
      bytes = (await readInput(url, location)).data;
    }
    out.__image_b64 = Buffer.from(bytes).toString('base64');
  }
  return out;
}

/**
 * PDFNet.TemplateDocument — a .docx template (Apryse DocGen syntax):
 * {{value}}, {{a.b.c}}, {{loop items}}…{{endloop}}, {{if cond}}…{{elif cond}}…{{else}}…{{endif}},
 * images from { "image_url": file | URL | data URI, "width": pt, "height": pt }.
 */
export class TemplateDocument {
  #warnings: string[] = [];

  /** @internal */
  constructor(
    private readonly template: Uint8Array,
    private readonly options: OfficeToPDFOptions | null,
  ) {}

  /** JSON object describing the keys the template expects (arrays for loops). */
  async getTemplateKeysJson(): Promise<string> {
    const keys = call('TemplateDocument.getTemplateKeysJson', () => native.docgenTemplateKeys(Buffer.from(this.template)));
    return JSON.stringify(keys, null, 2);
  }

  /** Fills the template and returns the .docx bytes. */
  async fillTemplateJsonToBuffer(json: string | object): Promise<Uint8Array> {
    let data: Json;
    try {
      data = (typeof json === 'string' ? JSON.parse(json) : json) as Json;
    } catch (e) {
      throw new PDFNetError('invalid JSON data', 'TemplateDocument.fillTemplateJson', { cause: e });
    }
    const resolved = await resolveImages(data, 'TemplateDocument.fillTemplateJson');
    const r = call('TemplateDocument.fillTemplateJson', () => native.docgenFill(Buffer.from(this.template), resolved));
    this.#warnings = r.warnings;
    return new Uint8Array(r.docx.buffer, r.docx.byteOffset, r.docx.byteLength);
  }

  /** Fills the template and writes the resulting .docx. */
  async fillTemplateJsonToOffice(json: string | object, outputFilename: string): Promise<void> {
    const docx = await this.fillTemplateJsonToBuffer(json);
    try {
      await writeFile(outputFilename, docx);
    } catch (e) {
      throw new PDFNetError(`cannot write ${outputFilename}`, 'TemplateDocument.fillTemplateJsonToOffice', { cause: e });
    }
  }

  /** Fills the template and converts it to PDF (through Gotenberg). */
  async fillTemplateJson(json: string | object): Promise<PDFDoc> {
    const docx = await this.fillTemplateJsonToBuffer(json);
    const opts = this.options ?? new OfficeToPDFOptions();
    const prev = opts.fileName;
    opts.fileName = 'document.docx';
    try {
      return await Convert.officeToPdfDoc(docx, opts);
    } finally {
      opts.fileName = prev;
    }
  }

  /** Warnings of the last fill (missing values…), one per line; '' when none. */
  async destroy(): Promise<void> {}

  async getErrorString(): Promise<string> {
    return this.#warnings.join('\n');
  }
}

/** @internal Shared by Convert.createOfficeTemplate*. */
function createTemplate(template: Uint8Array, options: OfficeToPDFOptions | null | undefined, location: string): TemplateDocument {
  // Validate early: unbalanced loops/conditions or a non-docx file fail here, like PDFNet.
  call(location, () => native.docgenTemplateKeys(Buffer.from(template)));
  return new TemplateDocument(template, options ?? null);
}
