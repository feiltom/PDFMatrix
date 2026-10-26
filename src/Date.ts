/**
 * PDFNet.Date — a PDF date (ISO 32000-1 §7.9.4): D:YYYYMMDDHHmmSSOHH'mm'
 */
export class PDFDate {
  year = 0;
  month = 0;
  day = 0;
  hour = 0;
  minute = 0;
  second = 0;
  /** 'Z', '+', '-' or 0 when unknown. */
  UT: string | 0 = 0;
  UT_hour = 0;
  UT_minutes = 0;

  static async create(): Promise<PDFDate> {
    return new PDFDate();
  }

  static async init(year: number, month: number, day: number, hour = 0, minute = 0, second = 0): Promise<PDFDate> {
    const d = new PDFDate();
    Object.assign(d, { year, month, day, hour, minute, second, UT: 'Z' });
    return d;
  }

  static fromJSDate(date: globalThis.Date): PDFDate {
    const d = new PDFDate();
    d.year = date.getUTCFullYear();
    d.month = date.getUTCMonth() + 1;
    d.day = date.getUTCDate();
    d.hour = date.getUTCHours();
    d.minute = date.getUTCMinutes();
    d.second = date.getUTCSeconds();
    d.UT = 'Z';
    return d;
  }

  static parse(s: string | null | undefined): PDFDate | null {
    if (!s) return null;
    const m = /^(?:D:)?(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?(\d{2})?([Zz+-])?(\d{2})?'?(\d{2})?'?/.exec(s.trim());
    if (!m) return null;
    const d = new PDFDate();
    d.year = Number(m[1]);
    d.month = Number(m[2] ?? 1);
    d.day = Number(m[3] ?? 1);
    d.hour = Number(m[4] ?? 0);
    d.minute = Number(m[5] ?? 0);
    d.second = Number(m[6] ?? 0);
    d.UT = m[7] ? m[7].toUpperCase() : 0;
    d.UT_hour = Number(m[8] ?? 0);
    d.UT_minutes = Number(m[9] ?? 0);
    return d;
  }

  async isValid(): Promise<boolean> {
    return this.year > 0 && this.month >= 1 && this.month <= 12 && this.day >= 1 && this.day <= 31;
  }

  async setCurrentTime(): Promise<void> {
    Object.assign(this, PDFDate.fromJSDate(new globalThis.Date()));
  }

  async getYear(): Promise<number> { return this.year; }
  async getMonth(): Promise<number> { return this.month; }
  async getDay(): Promise<number> { return this.day; }
  async getHour(): Promise<number> { return this.hour; }
  async getMinute(): Promise<number> { return this.minute; }
  async getSecond(): Promise<number> { return this.second; }
  async getUT(): Promise<string | 0> { return this.UT; }
  async getUTHour(): Promise<number> { return this.UT_hour; }
  async getUTMin(): Promise<number> { return this.UT_minutes; }

  toJSDate(): globalThis.Date {
    const sign = this.UT === '-' ? -1 : 1;
    const offsetMin = this.UT === '+' || this.UT === '-' ? sign * (this.UT_hour * 60 + this.UT_minutes) : 0;
    const utc = globalThis.Date.UTC(this.year, this.month - 1, this.day, this.hour, this.minute, this.second);
    return new globalThis.Date(utc - offsetMin * 60_000);
  }

  toPDFString(): string {
    const p = (n: number, w = 2) => String(n).padStart(w, '0');
    const base = `D:${p(this.year, 4)}${p(this.month)}${p(this.day)}${p(this.hour)}${p(this.minute)}${p(this.second)}`;
    if (this.UT === '+' || this.UT === '-') return `${base}${this.UT}${p(this.UT_hour)}'${p(this.UT_minutes)}'`;
    return this.UT === 'Z' ? `${base}Z` : base;
  }
}
