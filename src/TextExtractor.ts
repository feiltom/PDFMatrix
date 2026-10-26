import { call, PDFNetError } from './errors.js';
import { QuadPoint, Rect } from './geom.js';
import type { PageTextResult } from './native.js';
import type { Page } from './Page.js';

type NativeLine = PageTextResult['lines'][number];
type NativeWord = NativeLine['words'][number];

export class Word {
  /** @internal */
  constructor(private readonly line: Line | null, private readonly index: number) {}

  private get data(): NativeWord | undefined {
    return this.line?.data?.words[this.index];
  }

  async isValid(): Promise<boolean> {
    return this.data !== undefined;
  }

  async getString(): Promise<string> {
    return this.data?.text ?? '';
  }

  async getStringLen(): Promise<number> {
    return [...(this.data?.text ?? '')].length;
  }

  async getBBox(): Promise<Rect> {
    return Rect.fromArray(this.data?.bbox ?? [0, 0, 0, 0]);
  }

  async getQuad(): Promise<QuadPoint> {
    return QuadPoint.fromArray(this.data?.quad ?? [0, 0, 0, 0, 0, 0, 0, 0]);
  }

  async getNumGlyphs(): Promise<number> {
    return this.data?.chars.length ?? 0;
  }

  async getGlyphQuad(glyphIdx: number): Promise<QuadPoint> {
    const b = this.data?.chars[glyphIdx]?.bbox ?? [0, 0, 0, 0];
    return QuadPoint.fromArray([b[0], b[1], b[2], b[1], b[2], b[3], b[0], b[3]]);
  }

  async getFontSize(): Promise<number> {
    return this.data?.chars[0]?.fontSize ?? 0;
  }

  async getCurrentNum(): Promise<number> {
    return this.index;
  }

  async getNextWord(): Promise<Word> {
    return new Word(this.line, this.index + 1);
  }
}

export class Line {
  /** @internal */
  constructor(private readonly page: PageTextResult | null, private readonly index: number) {}

  /** @internal */
  get data(): NativeLine | undefined {
    return this.page?.lines[this.index];
  }

  async isValid(): Promise<boolean> {
    return this.data !== undefined;
  }

  async getNumWords(): Promise<number> {
    return this.data?.words.length ?? 0;
  }

  async getFirstWord(): Promise<Word> {
    return new Word(this, 0);
  }

  async getWord(wordIdx: number): Promise<Word> {
    return new Word(this, wordIdx);
  }

  async getBBox(): Promise<Rect> {
    return Rect.fromArray(this.data?.bbox ?? [0, 0, 0, 0]);
  }

  async getCurrentNum(): Promise<number> {
    return this.index;
  }

  async getNextLine(): Promise<Line> {
    return new Line(this.page, this.index + 1);
  }

  async isSimpleLine(): Promise<boolean> {
    return true;
  }
}

/** PDFNet.TextExtractor — words and lines with their positions (PDF user space). */
export class TextExtractor {
  static readonly ProcessingFlags = {
    e_no_ligature_exp: 1,
    e_no_dup_remove: 2,
    e_punct_break: 4,
    e_remove_hidden_text: 8,
    e_no_invisible_text: 16,
    e_no_watermarks: 128,
    e_extract_using_zorder: 256,
  } as const;

  static readonly XMLOutputFlags = {
    e_words_as_elements: 1,
    e_output_bbox: 2,
    e_output_style_info: 4,
  } as const;

  #result: PageTextResult | null = null;

  static async create(): Promise<TextExtractor> {
    return new TextExtractor();
  }

  /**
   * Extracts the text of `page`. When `clip` is given, only words whose bounding box center
   * lies inside it are kept.
   */
  async begin(page: Page, clip?: Rect | null, _flags = 0): Promise<void> {
    const index = page.requireAttached('TextExtractor.begin');
    const result = call('TextExtractor.begin', () => page.doc.native().extractText(index));
    if (clip) {
      const c = clip.normalized();
      const inside = (b: number[]) => {
        const cx = (b[0] + b[2]) / 2;
        const cy = (b[1] + b[3]) / 2;
        return cx >= c.x1 && cx <= c.x2 && cy >= c.y1 && cy <= c.y2;
      };
      result.lines = result.lines
        .map((l) => ({ ...l, words: l.words.filter((w) => inside(w.bbox)) }))
        .filter((l) => l.words.length > 0);
      result.text = result.lines.map((l) => l.words.map((w) => w.text).join(' ')).join('\n');
    }
    this.#result = result;
  }

  private get result(): PageTextResult {
    if (!this.#result) throw new PDFNetError('call begin(page) first', 'TextExtractor');
    return this.#result;
  }

  /** Page text; with `dehyphen`, words broken by a trailing hyphen are joined across lines. */
  async getAsText(dehyphen = false): Promise<string> {
    const text = this.result.text;
    return dehyphen ? text.replace(/(\p{L})-\n(\p{L})/gu, '$1$2') : text;
  }

  async getWordCount(): Promise<number> {
    return this.result.lines.reduce((n, l) => n + l.words.length, 0);
  }

  async getNumLines(): Promise<number> {
    return this.result.lines.length;
  }

  async getFirstLine(): Promise<Line> {
    return new Line(this.result, 0);
  }

  async getRightToLeftLanguage(): Promise<boolean> {
    return false;
  }

  async setRightToLeftLanguage(_rtl: boolean): Promise<void> {}

  /** Text of the words whose center lies inside `rect`. */
  async getTextUnderAnnot(annot: { getRect(): Promise<Rect> }): Promise<string> {
    const r = (await annot.getRect()).normalized();
    return this.result.lines
      .map((l) =>
        l.words
          .filter((w) => {
            const cx = (w.bbox[0] + w.bbox[2]) / 2;
            const cy = (w.bbox[1] + w.bbox[3]) / 2;
            return cx >= r.x1 && cx <= r.x2 && cy >= r.y1 && cy <= r.y2;
          })
          .map((w) => w.text)
          .join(' '),
      )
      .filter((s) => s.length > 0)
      .join('\n');
  }

  /** XML similar to PDFNet's output (Page/Flow/Para/Line/Word). */
  async destroy(): Promise<void> {}

  async getAsXML(flags = 0): Promise<string> {
    const F = TextExtractor.XMLOutputFlags;
    const esc = (s: string) => s.replace(/[<>&"]/g, (c) => `&${{ '<': 'lt', '>': 'gt', '&': 'amp', '"': 'quot' }[c]};`);
    const box = (b: number[]) => (flags & F.e_output_bbox ? ` box="${b.map((v) => v.toFixed(2)).join(', ')}"` : '');
    const lines = this.result.lines.map((l) => {
      const body =
        flags & F.e_words_as_elements
          ? l.words.map((w) => `<Word${box(w.bbox)}>${esc(w.text)}</Word>`).join('')
          : esc(l.words.map((w) => w.text).join(' '));
      return `<Line${box(l.bbox)}>${body}</Line>`;
    });
    return `<Page><Flow><Para>${lines.join('')}</Para></Flow></Page>`;
  }
}
