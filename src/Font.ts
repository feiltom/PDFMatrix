import { readFile } from 'node:fs/promises';

import { call, PDFNetError } from './errors.js';
import type { PDFDoc } from './PDFDoc.js';

enum StandardType1Font {
  e_times_roman = 0,
  e_times_bold = 1,
  e_times_italic = 2,
  e_times_bold_italic = 3,
  e_helvetica = 4,
  e_helvetica_bold = 5,
  e_helvetica_oblique = 6,
  e_helvetica_bold_oblique = 7,
  e_courier = 8,
  e_courier_bold = 9,
  e_courier_oblique = 10,
  e_courier_bold_oblique = 11,
  e_symbol = 12,
  e_zapf_dingbats = 13,
}

/**
 * PDFNet.Font — a font usable to write text (Stamper, appearances).
 * Standard fonts use WinAnsiEncoding; TrueType fonts are embedded for full Unicode.
 */
export class Font {
  static readonly StandardType1Font = StandardType1Font;

  /** @internal */
  constructor(
    readonly doc: PDFDoc,
    readonly index: number,
    private readonly info: { name: string; ascender: number; descender: number },
  ) {}

  static async create(doc: PDFDoc, type: StandardType1Font = StandardType1Font.e_helvetica): Promise<Font> {
    const info = call('Font.create', () => doc.native().fontStandard(type));
    return new Font(doc, info.index, info);
  }

  /** Embeds a TrueType/OpenType font file (always embedded; Unicode via Identity-H). */
  static async createTrueTypeFont(doc: PDFDoc, path: string, _embed = true, _subset = true): Promise<Font> {
    let data: Buffer;
    try {
      data = await readFile(path);
    } catch (e) {
      throw new PDFNetError(`cannot read font ${path}`, 'Font.createTrueTypeFont', { cause: e });
    }
    return Font.createFromBuffer(doc, data);
  }

  static async createCIDTrueTypeFont(doc: PDFDoc, path: string, embed = true, subset = true): Promise<Font> {
    return Font.createTrueTypeFont(doc, path, embed, subset);
  }

  static async createFromBuffer(doc: PDFDoc, data: Uint8Array): Promise<Font> {
    const info = call('Font.createFromBuffer', () => doc.native().fontTruetype(Buffer.from(data)));
    return new Font(doc, info.index, info);
  }

  async getName(): Promise<string> {
    return this.info.name;
  }

  async getFamilyName(): Promise<string> {
    return this.info.name.replace(/^[A-Z]{6}\+/, '').split('-')[0];
  }

  /** Ascender, in 1/1000 em. */
  async getAscent(): Promise<number> {
    return this.info.ascender;
  }

  /** Descender (negative), in 1/1000 em. */
  async getDescent(): Promise<number> {
    return this.info.descender;
  }

  /** Width of `text` in 1/1000 em (multiply by size / 1000 for points). */
  async getTextWidth(text: string): Promise<number> {
    return call('Font.getTextWidth', () => this.doc.native().fontWidth(this.index, text));
  }
}
