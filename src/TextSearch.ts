import { call, PDFNetError } from './errors.js';
import { QuadPoint } from './geom.js';
import type { PDFDoc } from './PDFDoc.js';

enum Mode {
  e_reg_expression = 0x0001,
  e_case_sensitive = 0x0002,
  e_whole_word = 0x0004,
  e_search_up = 0x0008,
  e_page_stop = 0x0010,
  e_highlight = 0x0020,
  e_ambient_string = 0x0040,
}

enum ResultCode {
  e_done = 0,
  e_page = 1,
  e_found = 2,
}

/** Highlight quads of a search result (subset of PDFNet.Highlights). */
export class Highlights {
  #i = 0;

  /** @internal */
  constructor(private readonly pageNum: number, private readonly quads: number[][]) {}

  async begin(_doc?: PDFDoc): Promise<void> {
    this.#i = 0;
  }

  async hasNext(): Promise<boolean> {
    return this.#i < this.quads.length;
  }

  async next(): Promise<void> {
    this.#i++;
  }

  async getCurrentPageNumber(): Promise<number> {
    return this.pageNum;
  }

  /** Quads of the current highlight, as a flat array [x1 y1 … x4 y4] per quad. */
  async getCurrentQuads(): Promise<QuadPoint[]> {
    const q = this.quads[this.#i];
    return q ? [QuadPoint.fromArray(q)] : [];
  }

  /** All quads of the result. */
  async getAllQuads(): Promise<QuadPoint[]> {
    return this.quads.map((q) => QuadPoint.fromArray(q));
  }
}

export interface TextSearchResult {
  code: ResultCode;
  out_str: string;
  ambient_str: string;
  page_num: number;
  highlights: Highlights | null;
}

/** PDFNet.TextSearch — iterative search over a page range. */
export class TextSearch {
  static readonly Mode = Mode;
  static readonly ResultCode = ResultCode;

  #doc: PDFDoc | null = null;
  #pattern = '';
  #mode = 0;
  #start = 1;
  #end = 1;
  #page = 0;
  #pending: { text: string; ambient: string; quads: number[][] }[] = [];
  #pageAnnounced = false;

  static async create(): Promise<TextSearch> {
    return new TextSearch();
  }

  /** startPage/endPage are 1-based and inclusive; -1 means first/last page. */
  async begin(doc: PDFDoc, pattern: string, mode: number, startPage = -1, endPage = -1): Promise<boolean> {
    const count = call('TextSearch.begin', () => doc.native().pageCount());
    if (count === 0) return false;
    this.#doc = doc;
    this.#pattern = pattern;
    this.#mode = mode;
    this.#start = startPage < 1 ? 1 : Math.min(startPage, count);
    this.#end = endPage < 1 ? count : Math.min(endPage, count);
    if (this.#start > this.#end) [this.#start, this.#end] = [this.#end, this.#start];
    this.#page = mode & Mode.e_search_up ? this.#end : this.#start;
    this.#pending = [];
    this.#pageAnnounced = false;
    this.loadPage();
    return true;
  }

  private loadPage(): void {
    const doc = this.#doc!;
    const hits = call('TextSearch.run', () =>
      doc.native().search(this.#page - 1, this.#pattern, {
        regex: (this.#mode & Mode.e_reg_expression) !== 0,
        caseSensitive: (this.#mode & Mode.e_case_sensitive) !== 0,
        wholeWord: (this.#mode & Mode.e_whole_word) !== 0,
      }),
    );
    this.#pending = this.#mode & Mode.e_search_up ? hits.reverse() : hits;
  }

  async run(): Promise<TextSearchResult> {
    if (!this.#doc) throw new PDFNetError('call begin() first', 'TextSearch.run');
    const up = (this.#mode & Mode.e_search_up) !== 0;
    for (;;) {
      const hit = this.#pending.shift();
      if (hit) {
        return {
          code: ResultCode.e_found,
          out_str: hit.text,
          ambient_str: this.#mode & Mode.e_ambient_string ? hit.ambient : '',
          page_num: this.#page,
          highlights: this.#mode & Mode.e_highlight ? new Highlights(this.#page, hit.quads) : null,
        };
      }
      const nextPage = up ? this.#page - 1 : this.#page + 1;
      if (nextPage < this.#start || nextPage > this.#end) {
        return { code: ResultCode.e_done, out_str: '', ambient_str: '', page_num: this.#page, highlights: null };
      }
      if (this.#mode & Mode.e_page_stop && !this.#pageAnnounced) {
        this.#pageAnnounced = true;
        return { code: ResultCode.e_page, out_str: '', ambient_str: '', page_num: this.#page, highlights: null };
      }
      this.#pageAnnounced = false;
      this.#page = nextPage;
      this.loadPage();
    }
  }

  async setPattern(pattern: string): Promise<boolean> {
    this.#pattern = pattern;
    if (this.#doc) this.loadPage();
    return true;
  }

  async getMode(): Promise<number> {
    return this.#mode;
  }

  async setMode(mode: number): Promise<void> {
    this.#mode = mode;
  }

  async getCurrentPage(): Promise<number> {
    return this.#page;
  }

  async setRightToLeftLanguage(_rtl: boolean): Promise<void> {}
}
