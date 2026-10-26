import { writeFile } from 'node:fs/promises';

import { call, PDFNetError } from './errors.js';
import type { RenderOptions } from './native.js';
import type { Page } from './Page.js';

enum PixelFormat {
  e_rgba = 0,
  e_bgra = 1,
  e_rgb = 2,
  e_bgr = 3,
  e_gray = 4,
  e_gray_alpha = 5,
  e_cmyk = 6,
}

export interface Bitmap {
  width: number;
  height: number;
  stride: number;
  dpi: number;
  buf: Uint8Array;
}

/** PDFNet.PDFDraw — rasterizes pages (hayro, pure Rust). Supported formats: PNG, JPEG/JPG. */
export class PDFDraw {
  static readonly PixelFormat = PixelFormat;

  #dpi: number;
  #size: { width: number; height: number; preserve: boolean } | null = null;
  #transparent = false;
  #annotations = true;
  #jpegQuality = 80;

  private constructor(dpi: number) {
    this.#dpi = dpi;
  }

  static async create(dpi = 92): Promise<PDFDraw> {
    return new PDFDraw(dpi);
  }

  async setDPI(dpi: number): Promise<void> {
    this.#dpi = dpi;
    this.#size = null;
  }

  /** Renders to a fixed pixel size (fit inside width×height when preserveAspectRatio). */
  async setImageSize(width: number, height: number, preserveAspectRatio = true): Promise<void> {
    this.#size = { width, height, preserve: preserveAspectRatio };
  }

  async setPageTransparent(transparent: boolean): Promise<void> {
    this.#transparent = transparent;
  }

  async setDrawAnnotations(draw: boolean): Promise<void> {
    this.#annotations = draw;
  }

  /** JPEG quality (1-100) used by export/exportBuffer. */
  /** Releases resources (no-op: memory is garbage collected). */
  async destroy(): Promise<void> {}

  async setJPEGQuality(quality: number): Promise<void> {
    this.#jpegQuality = quality;
  }

  // Accepted for API parity; the rasterizer always anti-aliases.
  async setAntiAliasing(_on: boolean): Promise<void> {}
  async setImageSmoothing(_on = true, _hq = false): Promise<void> {}
  async setPrintMode(_on: boolean): Promise<void> {}
  async setThinLineAdjustment(_pixelGrid: boolean, _strokeAdjust: boolean): Promise<void> {}
  async setGamma(_exp: number): Promise<void> {}
  async setOCGContext(_ctx: unknown): Promise<void> {}

  private render(page: Page, format: string, location: string) {
    const index = page.requireAttached(location);
    const opts: RenderOptions = {
      dpi: this.#dpi,
      transparent: this.#transparent,
      annotations: this.#annotations,
      format,
      jpegQuality: this.#jpegQuality,
    };
    if (this.#size) {
      opts.width = this.#size.width;
      opts.height = this.#size.height;
      opts.preserveAspectRatio = this.#size.preserve;
    }
    return call(location, () => page.doc.native().render(index, opts));
  }

  private static normalizeFormat(format: string): string {
    const f = format.toUpperCase();
    if (f === 'PNG' || f === 'JPEG' || f === 'JPG') return f === 'JPG' ? 'JPEG' : f;
    throw new PDFNetError(`unsupported export format ${format} (PNG or JPEG)`, 'PDFDraw.export');
  }

  /** Renders `page` and writes it to `filename`. */
  async export(page: Page, filename: string, format = 'PNG'): Promise<void> {
    const data = await this.exportBuffer(page, format);
    await writeFile(filename, data);
  }

  /** Renders `page` and returns the encoded image (PNG or JPEG). */
  async exportBuffer(page: Page, format = 'PNG'): Promise<Uint8Array> {
    const r = this.render(page, PDFDraw.normalizeFormat(format), 'PDFDraw.export');
    return new Uint8Array(r.data.buffer, r.data.byteOffset, r.data.byteLength);
  }

  async exportStream(page: Page, format = 'PNG'): Promise<Uint8Array> {
    return this.exportBuffer(page, format);
  }

  /** Raw pixels. Default pixel format is BGRA like PDFNet. */
  async getBitmap(page: Page, pixelFormat: PixelFormat = PixelFormat.e_bgra, demult = false): Promise<Bitmap> {
    const r = this.render(page, 'RGBA', 'PDFDraw.getBitmap');
    const { width, height } = r;
    const src = r.data;
    const n = width * height;
    const channels = { [PixelFormat.e_rgba]: 4, [PixelFormat.e_bgra]: 4, [PixelFormat.e_rgb]: 3, [PixelFormat.e_bgr]: 3, [PixelFormat.e_gray]: 1, [PixelFormat.e_gray_alpha]: 2 }[pixelFormat as number];
    if (!channels) throw new PDFNetError(`unsupported pixel format ${pixelFormat}`, 'PDFDraw.getBitmap');
    const out = new Uint8Array(n * channels);
    for (let i = 0; i < n; i++) {
      let r8 = src[i * 4], g8 = src[i * 4 + 1], b8 = src[i * 4 + 2];
      const a8 = src[i * 4 + 3];
      if (!demult && channels === 4) {
        // PDFNet returns premultiplied alpha unless demult is requested.
        r8 = Math.round((r8 * a8) / 255);
        g8 = Math.round((g8 * a8) / 255);
        b8 = Math.round((b8 * a8) / 255);
      }
      const o = i * channels;
      switch (pixelFormat) {
        case PixelFormat.e_rgba: out.set([r8, g8, b8, a8], o); break;
        case PixelFormat.e_bgra: out.set([b8, g8, r8, a8], o); break;
        case PixelFormat.e_rgb: out.set([r8, g8, b8], o); break;
        case PixelFormat.e_bgr: out.set([b8, g8, r8], o); break;
        case PixelFormat.e_gray: out[o] = Math.round(0.299 * r8 + 0.587 * g8 + 0.114 * b8); break;
        case PixelFormat.e_gray_alpha: out[o] = Math.round(0.299 * r8 + 0.587 * g8 + 0.114 * b8); out[o + 1] = a8; break;
      }
    }
    const dpi = this.#size ? (width / (await page.getPageWidth())) * 72 : this.#dpi;
    return { width, height, stride: width * channels, dpi, buf: out };
  }
}
