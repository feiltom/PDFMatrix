import { readFile } from 'node:fs/promises';

import { call, PDFNetError } from './errors.js';
import type { ObjRef } from './native.js';
import type { PDFDoc } from './PDFDoc.js';

/** PDFNet.Image — an image embedded in a document (PNG, JPEG, GIF, BMP, TIFF, WebP). */
export class Image {
  /** @internal */
  constructor(
    readonly doc: PDFDoc,
    readonly ref: ObjRef,
    private readonly width: number,
    private readonly height: number,
  ) {}

  static async createFromFile(doc: PDFDoc, path: string, _encoderHints?: unknown): Promise<Image> {
    let data: Buffer;
    try {
      data = await readFile(path);
    } catch (e) {
      throw new PDFNetError(`cannot read image ${path}`, 'Image.createFromFile', { cause: e });
    }
    return Image.createFromMemory2(doc, data);
  }

  /** Embeds an encoded image (file bytes). */
  static async createFromMemory2(doc: PDFDoc, buf: Uint8Array, _encoderHints?: unknown): Promise<Image> {
    const info = call('Image.createFromMemory2', () => doc.native().imageEmbed(Buffer.from(buf)));
    return new Image(doc, info.obj, info.width, info.height);
  }

  async getImageWidth(): Promise<number> {
    return this.width;
  }

  async getImageHeight(): Promise<number> {
    return this.height;
  }

  /** @internal */
  nativeInfo() {
    return { obj: this.ref, width: this.width, height: this.height };
  }
}
