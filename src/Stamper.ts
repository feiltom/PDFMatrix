import { call } from './errors.js';
import type { Font } from './Font.js';
import { ColorPt } from './geom.js';
import type { Image } from './Image.js';
import type { Page } from './Page.js';
import type { PageSet } from './PageSet.js';
import type { PDFDoc } from './PDFDoc.js';

enum SizeType {
  e_relative_scale = 1,
  e_absolute_size = 2,
  e_font_size = 3,
}

enum HorizontalAlignment {
  e_horizontal_left = -1,
  e_horizontal_center = 0,
  e_horizontal_right = 1,
}

enum VerticalAlignment {
  e_vertical_bottom = -1,
  e_vertical_center = 0,
  e_vertical_top = 1,
}

enum TextAlignment {
  e_align_left = -1,
  e_align_center = 0,
  e_align_right = 1,
}

/**
 * PDFNet.Stamper — stamps text, images or pages (watermarks, headers, page numbers…).
 * Stamps are added to the page content (or as Stamp annotations with setAsAnnotation) and can
 * be removed with Stamper.deleteStamps.
 */
export class Stamper {
  static readonly SizeType = SizeType;
  static readonly HorizontalAlignment = HorizontalAlignment;
  static readonly VerticalAlignment = VerticalAlignment;
  static readonly TextAlignment = TextAlignment;

  #size: { type: SizeType; a: number; b: number };
  #h = HorizontalAlignment.e_horizontal_center;
  #v = VerticalAlignment.e_vertical_center;
  #pos = { x: 0, y: 0, percentage: false };
  #rotation = 0;
  #opacity = 1;
  #background = false;
  #annotation = false;
  #textAlign = TextAlignment.e_align_center;
  #font: Font | null = null;
  #color: number[] = [1, 0, 0];
  #underline = false;

  private constructor(type: SizeType, a: number, b: number) {
    this.#size = { type, a, b };
  }

  static async create(sizeType: SizeType, a: number, b: number): Promise<Stamper> {
    return new Stamper(sizeType, a, b);
  }

  async setSize(sizeType: SizeType, a: number, b: number): Promise<void> {
    this.#size = { type: sizeType, a, b };
  }

  async setAlignment(h: HorizontalAlignment, v: VerticalAlignment): Promise<void> {
    this.#h = h;
    this.#v = v;
  }

  /** Offset from the alignment reference, in points or as a fraction of the page size. */
  async setPosition(x: number, y: number, usePercentage = false): Promise<void> {
    this.#pos = { x, y, percentage: usePercentage };
  }

  /** Counter-clockwise rotation in degrees. */
  async setRotation(degrees: number): Promise<void> {
    this.#rotation = degrees;
  }

  async setOpacity(opacity: number): Promise<void> {
    this.#opacity = opacity;
  }

  async setAsBackground(background: boolean): Promise<void> {
    this.#background = background;
  }

  async setAsAnnotation(annotation: boolean): Promise<void> {
    this.#annotation = annotation;
  }

  async setTextAlignment(align: TextAlignment): Promise<void> {
    this.#textAlign = align;
  }

  async setFont(font: Font): Promise<void> {
    this.#font = font;
  }

  /** Text color (RGB components of the ColorPt). */
  async setFontColor(color: ColorPt): Promise<void> {
    this.#color = color.c.slice(0, 3);
  }

  async destroy(): Promise<void> {}

  async setUnderline(underline: boolean): Promise<void> {
    this.#underline = underline;
  }

  // Visibility through optional content is not implemented; stamps always show and print.
  async showsOnScreen(_on: boolean): Promise<void> {}
  async showsOnPrint(_on: boolean): Promise<void> {}

  private options(doc: PDFDoc) {
    return {
      sizeType: this.#size.type,
      a: this.#size.a,
      b: this.#size.b,
      hAlign: this.#h,
      vAlign: this.#v,
      x: this.#pos.x,
      y: this.#pos.y,
      percentage: this.#pos.percentage,
      rotation: this.#rotation,
      opacity: this.#opacity,
      background: this.#background,
      asAnnotation: this.#annotation,
      textAlign: this.#textAlign,
      font: this.#font && this.#font.doc === doc ? this.#font.index : undefined,
      color: this.#color,
      underline: this.#underline,
    };
  }

  private static pages(doc: PDFDoc, set: PageSet, location: string): number[] {
    return set.indices(call(location, () => doc.native().pageCount()), location);
  }

  async stampText(doc: PDFDoc, text: string, pageSet: PageSet): Promise<void> {
    const pages = Stamper.pages(doc, pageSet, 'Stamper.stampText');
    call('Stamper.stampText', () => doc.native().stampText(text, pages, this.options(doc)));
  }

  async stampImage(doc: PDFDoc, image: Image, pageSet: PageSet): Promise<void> {
    const pages = Stamper.pages(doc, pageSet, 'Stamper.stampImage');
    call('Stamper.stampImage', () => doc.native().stampImage(image.nativeInfo(), pages, this.options(doc)));
  }

  /** Stamps `srcPage` (from this or another document) onto the selected pages. */
  async stampPage(doc: PDFDoc, srcPage: Page, pageSet: PageSet): Promise<void> {
    const pages = Stamper.pages(doc, pageSet, 'Stamper.stampPage');
    const opts = this.options(doc);
    if (srcPage.doc === doc) {
      call('Stamper.stampPage', () => doc.native().stampPage(srcPage.ref, pages, opts));
    } else {
      const src = srcPage.doc.native();
      call('Stamper.stampPage', () => doc.native().stampPageFrom(src, srcPage.ref, pages, opts));
    }
  }

  static async deleteStamps(doc: PDFDoc, pageSet: PageSet): Promise<void> {
    const pages = Stamper.pages(doc, pageSet, 'Stamper.deleteStamps');
    call('Stamper.deleteStamps', () => doc.native().deleteStamps(pages));
  }

  static async hasStamps(doc: PDFDoc, pageSet: PageSet): Promise<boolean> {
    const pages = Stamper.pages(doc, pageSet, 'Stamper.hasStamps');
    return call('Stamper.hasStamps', () => doc.native().hasStamps(pages));
  }
}
