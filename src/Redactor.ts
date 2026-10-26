import { call } from './errors.js';
import { ColorPt, Rect } from './geom.js';
import type { PDFDoc } from './PDFDoc.js';

/** One area to redact (PDFNet.Redactor.Redaction). */
export class Redaction {
  constructor(
    /** 1-based page number */
    public page_num: number,
    public bbox: Rect,
    /** When true, everything on the page EXCEPT bbox is redacted. */
    public negative = false,
    /** Overlay text drawn in the area (when appearance.use_overlay_text). */
    public text = '',
  ) {}
}

/** Appearance of redacted areas (same field names as PDFNet's Redactor.Appearance). */
export interface RedactorAppearance {
  redaction_overlay?: boolean;
  positive_overlay_color?: ColorPt;
  negative_overlay_color?: ColorPt;
  border?: boolean;
  use_overlay_text?: boolean;
  min_font_size?: number;
  max_font_size?: number;
  text_color?: ColorPt;
  /** -1 left, 0 center, 1 right */
  horiz_text_alignment?: number;
  /** Accepted for API parity. */
  vert_text_alignment?: number;
  show_redacted_content_regions?: boolean;
  redacted_content_color?: ColorPt;
}

export interface RedactionResult {
  glyphs: number;
  images: number;
  paths: number;
  annotations: number;
}

const rgb = (c: ColorPt | undefined, fallback: number[]) => (c ? c.c.slice(0, 3) : fallback);

/**
 * PDFNet.Redactor — removes text, image pixels, vector graphics and annotations under the given
 * areas (the content is deleted from the file, not just covered).
 */
export class Redactor {
  static readonly Redaction = Redaction;

  static async redactionCreate(pageNum: number, bbox: Rect, negative = false, text = ''): Promise<Redaction> {
    return new Redaction(pageNum, bbox, negative, text);
  }

  /**
   * Applies the redactions. Coordinates are in PDF user space; `extNegMode` and `pageCoordSys`
   * are accepted for API parity.
   */
  static async redact(
    doc: PDFDoc,
    redactions: Redaction[],
    appearance: RedactorAppearance = {},
    _extNegMode = false,
    _pageCoordSys = true,
  ): Promise<RedactionResult> {
    const total: RedactionResult = { glyphs: 0, images: 0, paths: 0, annotations: 0 };
    // Group by overlay text and polarity: each group gets one overlay style.
    const groups = new Map<string, { negative: boolean; text: string; regions: { page: number; rect: number[] }[] }>();
    for (const r of redactions) {
      const page = await doc.getPage(r.page_num);
      const rects = r.negative ? complement((await page.getMediaBox()).toArray(), r.bbox.toArray()) : [r.bbox.toArray()];
      const key = `${r.negative}|${r.text}`;
      const g = groups.get(key) ?? { negative: r.negative, text: r.text, regions: [] };
      g.regions.push(...rects.map((rect) => ({ page: r.page_num - 1, rect })));
      groups.set(key, g);
    }
    for (const g of groups.values()) {
      const color = g.negative
        ? rgb(appearance.negative_overlay_color, [1, 1, 1])
        : rgb(appearance.positive_overlay_color, [0, 0, 0]);
      const stats = call('Redactor.redact', () =>
        doc.native().redact(g.regions, {
          enabled: appearance.redaction_overlay ?? true,
          color,
          border: appearance.border ?? false,
          borderColor: rgb(appearance.redacted_content_color, [1, 0, 0]),
          text: (appearance.use_overlay_text ?? true) && !g.negative ? g.text : undefined,
          textColor: rgb(appearance.text_color, [1, 1, 1]),
          minFontSize: appearance.min_font_size ?? 2,
          maxFontSize: appearance.max_font_size ?? 24,
          textAlign: appearance.horiz_text_alignment ?? 0,
        }),
      );
      total.glyphs += stats.glyphs;
      total.images += stats.images;
      total.paths += stats.paths;
      total.annotations += stats.annotations;
    }
    return total;
  }
}

/** The four bands of `page` around `keep` (for negative redaction). */
function complement(page: number[], keep: number[]): number[][] {
  const [px1, py1, px2, py2] = page;
  const [x1, y1, x2, y2] = keep;
  return [
    [px1, py1, px2, y1],
    [px1, y2, px2, py2],
    [px1, y1, x1, y2],
    [x2, y1, px2, y2],
  ].filter((r) => r[2] - r[0] > 0 && r[3] - r[1] > 0);
}
