import { PDFNetError } from './errors.js';

enum Filter {
  e_all = 0,
  e_even = 1,
  e_odd = 2,
}

/** PDFNet.PageSet — an ordered set of 1-based page numbers. */
export class PageSet {
  static readonly Filter = Filter;

  readonly #pages: number[] = [];

  static async create(): Promise<PageSet> {
    return new PageSet();
  }

  static async createSinglePage(pageNum: number): Promise<PageSet> {
    const s = new PageSet();
    await s.addPage(pageNum);
    return s;
  }

  static async createRange(rangeStart: number, rangeEnd: number): Promise<PageSet> {
    return PageSet.createFilteredRange(rangeStart, rangeEnd, Filter.e_all);
  }

  static async createFilteredRange(rangeStart: number, rangeEnd: number, filter: Filter = Filter.e_all): Promise<PageSet> {
    const s = new PageSet();
    await s.addRange(rangeStart, rangeEnd, filter);
    return s;
  }

  async addPage(pageNum: number): Promise<void> {
    this.#pages.push(pageNum);
  }

  async addRange(rangeStart: number, rangeEnd: number, filter: Filter = Filter.e_all): Promise<void> {
    for (let p = rangeStart; p <= rangeEnd; p++) {
      if (filter === Filter.e_even && p % 2 !== 0) continue;
      if (filter === Filter.e_odd && p % 2 === 0) continue;
      this.#pages.push(p);
    }
  }

  /** @internal 0-based page indices, validated against the page count. */
  indices(pageCount: number, location: string): number[] {
    for (const p of this.#pages) {
      if (p < 1 || p > pageCount) throw new PDFNetError(`page ${p} out of range (1-${pageCount})`, location);
    }
    return [...new Set(this.#pages)].map((p) => p - 1);
  }
}
