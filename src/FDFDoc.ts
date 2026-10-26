import { readFile } from 'node:fs/promises';

import { call, PDFNetError } from './errors.js';
import { native } from './native.js';

/**
 * PDFNet.FDFDoc — annotation/form data exchanged as XFDF. Created from XFDF (validated on
 * creation) and merged into a document with doc.fdfMerge(fdf) or doc.mergeXFDF(fdf).
 */
export class FDFDoc {
  /** @internal */
  constructor(readonly xfdf: string) {}

  /** Parses XFDF; throws a PDFNetError when it is not well-formed XFDF. */
  static async createFromXFDF(xfdf: string): Promise<FDFDoc> {
    call('FDFDoc.createFromXFDF', () => native.xfdfValidate(xfdf));
    return new FDFDoc(xfdf);
  }

  static async createFromXFDFFile(path: string): Promise<FDFDoc> {
    let text: string;
    try {
      text = await readFile(path, 'utf8');
    } catch (e) {
      throw new PDFNetError(`cannot read ${path}`, 'FDFDoc.createFromXFDF', { cause: e });
    }
    return FDFDoc.createFromXFDF(text);
  }

  async saveAsXFDFString(): Promise<string> {
    return this.xfdf;
  }

  async destroy(): Promise<void> {}
}
