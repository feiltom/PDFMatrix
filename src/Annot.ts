import { PDFDate } from './Date.js';
import { call, PDFNetError } from './errors.js';
import { ColorPt, Point, QuadPoint, Rect } from './geom.js';
import type { Image } from './Image.js';
import type { ObjRef } from './native.js';
import { Page } from './Page.js';
import type { PDFDoc } from './PDFDoc.js';
import {
  asName,
  asNumbers,
  asRef,
  colorArray,
  getDict,
  name,
  ref,
  setKey,
  toColorPt,
  type PdfDict,
  type PdfValue,
} from './sdf.js';

enum Type {
  e_Text = 0,
  e_Link = 1,
  e_FreeText = 2,
  e_Line = 3,
  e_Square = 4,
  e_Circle = 5,
  e_Polygon = 6,
  e_Polyline = 7,
  e_Highlight = 8,
  e_Underline = 9,
  e_Squiggly = 10,
  e_StrikeOut = 11,
  e_Stamp = 12,
  e_Caret = 13,
  e_Ink = 14,
  e_Popup = 15,
  e_FileAttachment = 16,
  e_Sound = 17,
  e_Movie = 18,
  e_Widget = 19,
  e_Screen = 20,
  e_PrinterMark = 21,
  e_TrapNet = 22,
  e_Watermark = 23,
  e_3D = 24,
  e_Redact = 25,
  e_Projection = 26,
  e_RichMedia = 27,
  e_Unknown = 28,
}

const SUBTYPES: Record<number, string> = {
  [Type.e_Text]: 'Text', [Type.e_Link]: 'Link', [Type.e_FreeText]: 'FreeText', [Type.e_Line]: 'Line',
  [Type.e_Square]: 'Square', [Type.e_Circle]: 'Circle', [Type.e_Polygon]: 'Polygon', [Type.e_Polyline]: 'PolyLine',
  [Type.e_Highlight]: 'Highlight', [Type.e_Underline]: 'Underline', [Type.e_Squiggly]: 'Squiggly',
  [Type.e_StrikeOut]: 'StrikeOut', [Type.e_Stamp]: 'Stamp', [Type.e_Caret]: 'Caret', [Type.e_Ink]: 'Ink',
  [Type.e_Popup]: 'Popup', [Type.e_FileAttachment]: 'FileAttachment', [Type.e_Sound]: 'Sound', [Type.e_Movie]: 'Movie',
  [Type.e_Widget]: 'Widget', [Type.e_Screen]: 'Screen', [Type.e_PrinterMark]: 'PrinterMark', [Type.e_TrapNet]: 'TrapNet',
  [Type.e_Watermark]: 'Watermark', [Type.e_3D]: '3D', [Type.e_Redact]: 'Redact', [Type.e_Projection]: 'Projection',
  [Type.e_RichMedia]: 'RichMedia',
};

/** Bit positions of the annotation flags (PDFNet Annot.Flag). */
enum Flag {
  e_invisible = 0,
  e_hidden = 1,
  e_print = 2,
  e_no_zoom = 3,
  e_no_rotate = 4,
  e_no_view = 5,
  e_annot_read_only = 6,
  e_locked = 7,
  e_toggle_no_view = 8,
  e_locked_contents = 9,
}

enum BorderStyleType {
  e_solid = 0,
  e_dashed = 1,
  e_beveled = 2,
  e_inset = 3,
  e_underline = 4,
}

const BS_NAMES = ['S', 'D', 'B', 'I', 'U'];

/** PDFNet.AnnotBorderStyle */
export class AnnotBorderStyle {
  static readonly Style = BorderStyleType;

  constructor(
    public style: BorderStyleType = BorderStyleType.e_solid,
    public width = 1,
    public hr = 0,
    public vr = 0,
    public dash: number[] = [3],
  ) {}

  static async create(style: BorderStyleType, width: number, hr = 0, vr = 0): Promise<AnnotBorderStyle> {
    return new AnnotBorderStyle(style, width, hr, vr);
  }

  static async createWithDashPattern(style: BorderStyleType, width: number, hr: number, vr: number, dash: number[]): Promise<AnnotBorderStyle> {
    return new AnnotBorderStyle(style, width, hr, vr, dash);
  }

  async getStyle(): Promise<BorderStyleType> { return this.style; }
  async setStyle(s: BorderStyleType): Promise<void> { this.style = s; }
  async getWidth(): Promise<number> { return this.width; }
  async setWidth(w: number): Promise<void> { this.width = w; }
  async getDash(): Promise<number[]> { return this.dash; }
  async setDash(d: number[]): Promise<void> { this.dash = d; }
}

/** PDFNet.Destination (Fit and XYZ views). */
export class Destination {
  /** @internal */
  constructor(readonly page: Page, readonly array: PdfValue[]) {}

  static async createFit(page: Page): Promise<Destination> {
    return new Destination(page, [ref(page.ref), name('Fit')]);
  }

  static async createXYZ(page: Page, left: number, top: number, zoom: number): Promise<Destination> {
    return new Destination(page, [ref(page.ref), name('XYZ'), left, top, zoom]);
  }

  static async createFitH(page: Page, top: number): Promise<Destination> {
    return new Destination(page, [ref(page.ref), name('FitH'), top]);
  }

  async getPage(): Promise<Page> {
    return this.page;
  }
}

/** PDFNet.Action (URI and GoTo). */
export class Action {
  /** @internal */
  constructor(readonly dict: PdfDict) {}

  static async createURI(_doc: PDFDoc, uri: string): Promise<Action> {
    return new Action({ Type: name('Action'), S: name('URI'), URI: uri });
  }

  static async createGoto(dest: Destination): Promise<Action> {
    return new Action({ Type: name('Action'), S: name('GoTo'), D: dest.array });
  }

  async getType(): Promise<string> {
    return asName(this.dict.S) ?? '';
  }

  async getURI(): Promise<string> {
    return typeof this.dict.URI === 'string' ? this.dict.URI : '';
  }
}

/** PDFNet.Annot — base class of all annotations. */
export class Annot {
  static readonly Type = Type;
  static readonly Flag = Flag;

  /** @internal */
  constructor(readonly doc: PDFDoc, readonly ref: ObjRef) {}

  /** Creates a detached annotation; place it with page.annotPushBack(annot). */
  // Subclasses define their own create(doc, pos, …); the base signature stays permissive so that
  // the PDFNet API shape type-checks.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  static async create(doc: PDFDoc, ...args: any[]): Promise<Annot> {
    const [type, pos] = args as [Type, Rect];
    return new Annot(doc, Annot.createRaw(doc, type, pos));
  }

  /** @internal */
  static createRaw(doc: PDFDoc, type: Type, pos: Rect): ObjRef {
    const subtype = SUBTYPES[type];
    if (!subtype) throw new PDFNetError(`unknown annotation type ${type}`, 'Annot.create');
    return call('Annot.create', () => doc.native().annotCreate(subtype, pos.toArray()));
  }

  /** Re-types an annotation, e.g. `await PDFNet.LinkAnnot.createFromAnnot(annot)`. */
  static async createFromAnnot<T extends Annot>(this: new (doc: PDFDoc, ref: ObjRef) => T, annot: Annot): Promise<T> {
    return new this(annot.doc, annot.ref);
  }

  /** @internal */
  dict(): PdfDict {
    return getDict(this.doc, this.ref);
  }

  /** @internal */
  set(key: string, value: PdfValue): void {
    setKey(this.doc, this.ref, key, value);
  }

  async isValid(): Promise<boolean> {
    return this.doc.isOpen() && Object.keys(this.dict()).length > 0;
  }

  async getType(): Promise<Type> {
    const st = asName(this.dict().Subtype);
    const found = Object.entries(SUBTYPES).find(([, v]) => v === st);
    return found ? (Number(found[0]) as Type) : Type.e_Unknown;
  }

  async isMarkup(): Promise<boolean> {
    const t = await this.getType();
    return ![Type.e_Link, Type.e_Popup, Type.e_Widget, Type.e_Screen, Type.e_PrinterMark, Type.e_TrapNet, Type.e_Watermark, Type.e_3D, Type.e_Unknown].includes(t);
  }

  async getRect(): Promise<Rect> {
    return Rect.fromArray(asNumbers(this.dict().Rect).concat([0, 0, 0, 0]));
  }

  async setRect(r: Rect): Promise<void> {
    this.set('Rect', r.toArray());
  }

  async resize(r: Rect): Promise<void> {
    await this.setRect(r);
    await this.refreshAppearance();
  }

  async getContents(): Promise<string> {
    const c = this.dict().Contents;
    return typeof c === 'string' ? c : '';
  }

  async setContents(contents: string): Promise<void> {
    this.set('Contents', contents);
  }

  async getUniqueID(): Promise<string> {
    const nm = this.dict().NM;
    return typeof nm === 'string' ? nm : '';
  }

  async setUniqueID(id: string): Promise<void> {
    this.set('NM', id);
  }

  async getDate(): Promise<PDFDate> {
    const m = this.dict().M;
    return PDFDate.parse(typeof m === 'string' ? m : null) ?? new PDFDate();
  }

  async setDate(d: PDFDate): Promise<void> {
    this.set('M', d.toPDFString());
  }

  async getFlags(): Promise<number> {
    const f = this.dict().F;
    return typeof f === 'number' ? f : 0;
  }

  async getFlag(flag: Flag): Promise<boolean> {
    return ((await this.getFlags()) & (1 << flag)) !== 0;
  }

  async setFlag(flag: Flag, value: boolean): Promise<void> {
    const f = await this.getFlags();
    this.set('F', value ? f | (1 << flag) : f & ~(1 << flag));
  }

  async getColorAsRGB(): Promise<ColorPt> {
    return toColorPt(this.dict().C).color;
  }

  async getColorCompNum(): Promise<number> {
    return asNumbers(this.dict().C).length;
  }

  /** compNum: 0 (transparent), 1 (gray), 3 (RGB) or 4 (CMYK). */
  async setColor(color: ColorPt, compNum = 3): Promise<void> {
    this.set('C', colorArray(color, compNum));
  }

  async getBorderStyle(): Promise<AnnotBorderStyle> {
    const d = this.dict();
    const bs = d.BS && typeof d.BS === 'object' && !Array.isArray(d.BS) ? (d.BS as PdfDict) : null;
    const style = Math.max(0, BS_NAMES.indexOf(asName(bs?.S) ?? 'S')) as BorderStyleType;
    const width = typeof bs?.W === 'number' ? bs.W : (asNumbers(d.Border)[2] ?? 1);
    return new AnnotBorderStyle(style, width, 0, 0, asNumbers(bs?.D).length ? asNumbers(bs?.D) : [3]);
  }

  async setBorderStyle(bs: AnnotBorderStyle, _oldStyleOnly = false): Promise<void> {
    const v: PdfDict = { W: bs.width, S: name(BS_NAMES[bs.style] ?? 'S') };
    if (bs.style === BorderStyleType.e_dashed) v.D = bs.dash;
    this.set('BS', v);
  }

  /** The page the annotation is placed on (via /P). */
  async getPage(): Promise<Page> {
    const p = asRef(this.dict().P);
    if (!p) throw new PDFNetError('annotation is not placed on a page', 'Annot.getPage');
    return new Page(this.doc, p);
  }

  /** Regenerates the appearance stream from the annotation's properties. */
  async refreshAppearance(): Promise<void> {
    call('Annot.refreshAppearance', () => this.doc.native().annotRefresh(this.ref));
  }

  /** Burns the annotation into the page content and removes it. */
  async flatten(page: Page): Promise<void> {
    call('Annot.flatten', () => this.doc.native().annotFlatten(page.ref, this.ref));
  }
}

/** Markup annotations (all user annotations except links, popups and widgets). */
export class Markup extends Annot {
  async getTitle(): Promise<string> {
    const t = this.dict().T;
    return typeof t === 'string' ? t : '';
  }

  async setTitle(title: string): Promise<void> {
    this.set('T', title);
  }

  async getSubject(): Promise<string> {
    const s = this.dict().Subj;
    return typeof s === 'string' ? s : '';
  }

  async setSubject(subject: string): Promise<void> {
    this.set('Subj', subject);
  }

  async getOpacity(): Promise<number> {
    const ca = this.dict().CA;
    return typeof ca === 'number' ? ca : 1;
  }

  async setOpacity(opacity: number): Promise<void> {
    this.set('CA', opacity);
  }

  async getInteriorColor(): Promise<ColorPt> {
    return toColorPt(this.dict().IC).color;
  }

  async getInteriorColorCompNum(): Promise<number> {
    return asNumbers(this.dict().IC).length;
  }

  async setInteriorColor(color: ColorPt, compNum = 3): Promise<void> {
    this.set('IC', compNum === 0 ? null : colorArray(color, compNum));
  }

  async getCreationDates(): Promise<PDFDate> {
    const d = this.dict().CreationDate;
    return PDFDate.parse(typeof d === 'string' ? d : null) ?? new PDFDate();
  }

  async setCreationDates(d: PDFDate): Promise<void> {
    this.set('CreationDate', d.toPDFString());
  }

  async getPopup(): Promise<PopupAnnot | null> {
    const p = asRef(this.dict().Popup);
    return p ? new PopupAnnot(this.doc, p) : null;
  }

  async setPopup(popup: PopupAnnot): Promise<void> {
    this.set('Popup', ref(popup.ref));
    popup.set('Parent', ref(this.ref));
  }
}

const TEXT_ICONS = ['Comment', 'Key', 'Help', 'NewParagraph', 'Paragraph', 'Insert', 'Note'];

/** Sticky note. */
export class TextAnnot extends Markup {
  static readonly Icon = { e_Comment: 0, e_Key: 1, e_Help: 2, e_NewParagraph: 3, e_Paragraph: 4, e_Insert: 5, e_Note: 6, e_Unknown: 7 } as const;

  static async create(doc: PDFDoc, pos: Rect | Point, contents = ''): Promise<TextAnnot> {
    const r = pos instanceof Point ? new Rect(pos.x, pos.y - 20, pos.x + 20, pos.y) : pos;
    const a = new TextAnnot(doc, Annot.createRaw(doc, Type.e_Text, r));
    a.set('C', [1, 0.85, 0.2]);
    a.set('Name', name('Note'));
    if (contents) a.set('Contents', contents);
    return a;
  }

  async getIcon(): Promise<number> {
    const i = TEXT_ICONS.indexOf(asName(this.dict().Name) ?? 'Note');
    return i < 0 ? 7 : i;
  }

  async getIconName(): Promise<string> {
    return asName(this.dict().Name) ?? 'Note';
  }

  async setIcon(icon: number | string): Promise<void> {
    this.set('Name', name(typeof icon === 'string' ? icon : (TEXT_ICONS[icon] ?? 'Note')));
  }

  async isOpen(): Promise<boolean> {
    return this.dict().Open === true;
  }

  async setOpen(open: boolean): Promise<void> {
    this.set('Open', open);
  }
}

/** Text box drawn directly on the page. */
export class FreeTextAnnot extends Markup {
  static readonly IntentName = { e_FreeText: 0, e_FreeTextCallout: 1, e_FreeTextTypeWriter: 2 } as const;

  static async create(doc: PDFDoc, pos: Rect): Promise<FreeTextAnnot> {
    const a = new FreeTextAnnot(doc, Annot.createRaw(doc, Type.e_FreeText, pos));
    a.set('DA', '/Helv 12 Tf 0 g');
    a.set('BS', { W: 0, S: name('S') });
    return a;
  }

  private da(): { font: string; size: number; color: number[] } {
    const da = typeof this.dict().DA === 'string' ? (this.dict().DA as string) : '/Helv 12 Tf 0 g';
    const tf = /\/(\S+)\s+([\d.]+)\s+Tf/.exec(da);
    const rg = /([\d.]+)\s+([\d.]+)\s+([\d.]+)\s+rg/.exec(da);
    const g = /([\d.]+)\s+g(?:\s|$)/.exec(da);
    return {
      font: tf?.[1] ?? 'Helv',
      size: Number(tf?.[2] ?? 12),
      color: rg ? [Number(rg[1]), Number(rg[2]), Number(rg[3])] : g ? [Number(g[1])] : [0],
    };
  }

  private writeDa(v: { font: string; size: number; color: number[] }): void {
    const op = v.color.length === 1 ? 'g' : v.color.length === 4 ? 'k' : 'rg';
    this.set('DA', `/${v.font} ${v.size} Tf ${v.color.join(' ')} ${op}`);
  }

  async getFontSize(): Promise<number> {
    return this.da().size;
  }

  async setFontSize(size: number): Promise<void> {
    this.writeDa({ ...this.da(), size });
  }

  /** Font by standard name, e.g. 'Helv', 'TiRo', 'Cour', 'Helvetica-Bold'. */
  async setFontName(font: string): Promise<void> {
    this.writeDa({ ...this.da(), font: font.replace(/^\//, '') });
  }

  async getTextColor(): Promise<ColorPt> {
    const c = this.da().color;
    return new ColorPt(c[0] ?? 0, c[1] ?? 0, c[2] ?? 0, c[3] ?? 0);
  }

  async setTextColor(color: ColorPt, compNum = 3): Promise<void> {
    this.writeDa({ ...this.da(), color: colorArray(color, compNum) });
  }

  /** Border color (uses /C; background is the interior color). */
  async setLineColor(color: ColorPt, compNum = 3): Promise<void> {
    await this.setColor(color, compNum);
  }

  async getQuaddingFormat(): Promise<number> {
    const q = this.dict().Q;
    return typeof q === 'number' ? q : 0;
  }

  async setQuaddingFormat(q: number): Promise<void> {
    this.set('Q', q);
  }
}

export class SquareAnnot extends Markup {
  static async create(doc: PDFDoc, pos: Rect): Promise<SquareAnnot> {
    const a = new SquareAnnot(doc, Annot.createRaw(doc, Type.e_Square, pos));
    a.set('C', [0, 0, 0]);
    return a;
  }
}

export class CircleAnnot extends Markup {
  static async create(doc: PDFDoc, pos: Rect): Promise<CircleAnnot> {
    const a = new CircleAnnot(doc, Annot.createRaw(doc, Type.e_Circle, pos));
    a.set('C', [0, 0, 0]);
    return a;
  }
}

const ENDINGS = ['Square', 'Circle', 'Diamond', 'OpenArrow', 'ClosedArrow', 'Butt', 'ROpenArrow', 'RClosedArrow', 'Slash', 'None'];

export class LineAnnot extends Markup {
  static readonly EndingStyle = {
    e_Square: 0, e_Circle: 1, e_Diamond: 2, e_OpenArrow: 3, e_ClosedArrow: 4, e_Butt: 5,
    e_ROpenArrow: 6, e_RClosedArrow: 7, e_Slash: 8, e_None: 9, e_Unknown: 10,
  } as const;

  static async create(doc: PDFDoc, pos: Rect): Promise<LineAnnot> {
    const a = new LineAnnot(doc, Annot.createRaw(doc, Type.e_Line, pos));
    const n = pos.normalized();
    a.set('L', [n.x1, n.y1, n.x2, n.y2]);
    a.set('C', [0, 0, 0]);
    return a;
  }

  private line(): number[] {
    const l = asNumbers(this.dict().L);
    return l.length === 4 ? l : [0, 0, 0, 0];
  }

  async getStartPoint(): Promise<Point> {
    const l = this.line();
    return new Point(l[0], l[1]);
  }

  async setStartPoint(p: Point): Promise<void> {
    const l = this.line();
    this.set('L', [p.x, p.y, l[2], l[3]]);
  }

  async getEndPoint(): Promise<Point> {
    const l = this.line();
    return new Point(l[2], l[3]);
  }

  async setEndPoint(p: Point): Promise<void> {
    const l = this.line();
    this.set('L', [l[0], l[1], p.x, p.y]);
  }

  private endings(): string[] {
    const le = this.dict().LE;
    return Array.isArray(le) ? le.map((v) => asName(v) ?? 'None') : ['None', 'None'];
  }

  async getStartStyle(): Promise<number> {
    return Math.max(0, ENDINGS.indexOf(this.endings()[0]));
  }

  async setStartStyle(style: number): Promise<void> {
    this.set('LE', [name(ENDINGS[style] ?? 'None'), name(this.endings()[1] ?? 'None')]);
  }

  async getEndStyle(): Promise<number> {
    return Math.max(0, ENDINGS.indexOf(this.endings()[1]));
  }

  async setEndStyle(style: number): Promise<void> {
    this.set('LE', [name(this.endings()[0] ?? 'None'), name(ENDINGS[style] ?? 'None')]);
  }
}

export class PolyLineAnnot extends Markup {
  static async create(doc: PDFDoc, pos: Rect): Promise<PolyLineAnnot> {
    const a = new this(doc, Annot.createRaw(doc, Type.e_Polyline, pos));
    a.set('C', [0, 0, 0]);
    return a;
  }

  async getVertexCount(): Promise<number> {
    return asNumbers(this.dict().Vertices).length / 2;
  }

  async getVertex(idx: number): Promise<Point> {
    const v = asNumbers(this.dict().Vertices);
    return new Point(v[idx * 2] ?? 0, v[idx * 2 + 1] ?? 0);
  }

  async setVertex(idx: number, p: Point): Promise<void> {
    const v = asNumbers(this.dict().Vertices);
    while (v.length < idx * 2 + 2) v.push(p.x, p.y);
    v[idx * 2] = p.x;
    v[idx * 2 + 1] = p.y;
    this.set('Vertices', v);
  }
}

export class PolygonAnnot extends PolyLineAnnot {
  static async create(doc: PDFDoc, pos: Rect): Promise<PolygonAnnot> {
    const a = new PolygonAnnot(doc, Annot.createRaw(doc, Type.e_Polygon, pos));
    a.set('C', [0, 0, 0]);
    return a;
  }
}

export class InkAnnot extends Markup {
  static async create(doc: PDFDoc, pos: Rect): Promise<InkAnnot> {
    const a = new InkAnnot(doc, Annot.createRaw(doc, Type.e_Ink, pos));
    a.set('C', [0, 0, 0]);
    return a;
  }

  private paths(): number[][] {
    const ink = this.dict().InkList;
    return Array.isArray(ink) ? ink.map((p) => asNumbers(p)) : [];
  }

  async getPathCount(): Promise<number> {
    return this.paths().length;
  }

  async getPointCount(pathIdx: number): Promise<number> {
    return (this.paths()[pathIdx]?.length ?? 0) / 2;
  }

  async getPoint(pathIdx: number, pointIdx: number): Promise<Point> {
    const p = this.paths()[pathIdx] ?? [];
    return new Point(p[pointIdx * 2] ?? 0, p[pointIdx * 2 + 1] ?? 0);
  }

  async setPoint(pathIdx: number, pointIdx: number, pt: Point): Promise<void> {
    const paths = this.paths();
    while (paths.length <= pathIdx) paths.push([]);
    const p = paths[pathIdx];
    while (p.length < pointIdx * 2 + 2) p.push(pt.x, pt.y);
    p[pointIdx * 2] = pt.x;
    p[pointIdx * 2 + 1] = pt.y;
    this.set('InkList', paths);
  }
}

/** Text markup (highlight, underline, strike-out, squiggly). Quads are stored in Acrobat order. */
export class TextMarkupAnnot extends Markup {
  async getQuadPointCount(): Promise<number> {
    return asNumbers(this.dict().QuadPoints).length / 8;
  }

  async getQuadPoint(idx: number): Promise<QuadPoint> {
    const q = asNumbers(this.dict().QuadPoints).slice(idx * 8, idx * 8 + 8);
    // stored as UL, UR, LL, LR → p1..p4 = LL, LR, UR, UL
    return new QuadPoint(q[4], q[5], q[6], q[7], q[2], q[3], q[0], q[1]);
  }

  async setQuadPoint(idx: number, qp: QuadPoint): Promise<void> {
    const q = asNumbers(this.dict().QuadPoints);
    const stored = [qp.p4x, qp.p4y, qp.p3x, qp.p3y, qp.p1x, qp.p1y, qp.p2x, qp.p2y];
    while (q.length < idx * 8 + 8) q.push(...stored);
    q.splice(idx * 8, 8, ...stored);
    this.set('QuadPoints', q);
  }

  /** @internal */
  static createMarkup<T extends TextMarkupAnnot>(
    cls: new (doc: PDFDoc, ref: ObjRef) => T, doc: PDFDoc, type: Type, pos: Rect, color: number[],
  ): T {
    const a = new cls(doc, Annot.createRaw(doc, type, pos));
    a.set('C', color);
    const n = pos.normalized();
    a.set('QuadPoints', [n.x1, n.y2, n.x2, n.y2, n.x1, n.y1, n.x2, n.y1]);
    return a;
  }
}

export class HighlightAnnot extends TextMarkupAnnot {
  static async create(doc: PDFDoc, pos: Rect): Promise<HighlightAnnot> {
    return TextMarkupAnnot.createMarkup(HighlightAnnot, doc, Type.e_Highlight, pos, [1, 1, 0]);
  }
}

export class UnderlineAnnot extends TextMarkupAnnot {
  static async create(doc: PDFDoc, pos: Rect): Promise<UnderlineAnnot> {
    return TextMarkupAnnot.createMarkup(UnderlineAnnot, doc, Type.e_Underline, pos, [0, 0, 1]);
  }
}

export class StrikeOutAnnot extends TextMarkupAnnot {
  static async create(doc: PDFDoc, pos: Rect): Promise<StrikeOutAnnot> {
    return TextMarkupAnnot.createMarkup(StrikeOutAnnot, doc, Type.e_StrikeOut, pos, [1, 0, 0]);
  }
}

export class SquigglyAnnot extends TextMarkupAnnot {
  static async create(doc: PDFDoc, pos: Rect): Promise<SquigglyAnnot> {
    return TextMarkupAnnot.createMarkup(SquigglyAnnot, doc, Type.e_Squiggly, pos, [1, 0, 0]);
  }
}

export class LinkAnnot extends Annot {
  static async create(doc: PDFDoc, pos: Rect, action?: Action): Promise<LinkAnnot> {
    const a = new LinkAnnot(doc, Annot.createRaw(doc, Type.e_Link, pos));
    a.set('Border', [0, 0, 0]);
    if (action) await a.setAction(action);
    return a;
  }

  async setAction(action: Action): Promise<void> {
    this.set('A', action.dict);
  }

  async getAction(): Promise<Action | null> {
    const a = this.dict().A;
    return a && typeof a === 'object' && !Array.isArray(a) ? new Action(a as PdfDict) : null;
  }
}

const STAMP_ICONS = [
  'Approved', 'Experimental', 'NotApproved', 'AsIs', 'Expired', 'NotForPublicRelease', 'Confidential',
  'Final', 'Sold', 'Departmental', 'ForComment', 'TopSecret', 'ForPublicRelease', 'Draft',
];

export class RubberStampAnnot extends Markup {
  static readonly Icon = {
    e_Approved: 0, e_Experimental: 1, e_NotApproved: 2, e_AsIs: 3, e_Expired: 4, e_NotForPublicRelease: 5,
    e_Confidential: 6, e_Final: 7, e_Sold: 8, e_Departmental: 9, e_ForComment: 10, e_TopSecret: 11,
    e_ForPublicRelease: 12, e_Draft: 13, e_Unknown: 14,
  } as const;

  static async create(doc: PDFDoc, pos: Rect, icon: number | string = 13): Promise<RubberStampAnnot> {
    const a = new RubberStampAnnot(doc, Annot.createRaw(doc, Type.e_Stamp, pos));
    await a.setIcon(icon);
    return a;
  }

  async getIconName(): Promise<string> {
    return asName(this.dict().Name) ?? 'Draft';
  }

  async setIcon(icon: number | string): Promise<void> {
    this.set('Name', name(typeof icon === 'string' ? icon : (STAMP_ICONS[icon] ?? 'Draft')));
  }

  /** Uses an image (stretched to the annotation rect) as the stamp appearance. */
  async setImageAppearance(image: Image): Promise<void> {
    call('RubberStampAnnot.setImageAppearance', () => this.doc.native().annotImageAppearance(this.ref, image.ref));
  }
}

export class PopupAnnot extends Annot {
  static async create(doc: PDFDoc, pos: Rect, parent?: Markup): Promise<PopupAnnot> {
    const p = new PopupAnnot(doc, Annot.createRaw(doc, Type.e_Popup, pos));
    if (parent) await parent.setPopup(p);
    return p;
  }

  async isOpen(): Promise<boolean> {
    return this.dict().Open === true;
  }

  async setOpen(open: boolean): Promise<void> {
    this.set('Open', open);
  }

  async getParent(): Promise<Markup | null> {
    const p = asRef(this.dict().Parent);
    return p ? new Markup(this.doc, p) : null;
  }
}

/** Redaction mark (applied with PDFNet.Redactor in a later phase). */
export class RedactionAnnot extends Markup {
  static async create(doc: PDFDoc, pos: Rect): Promise<RedactionAnnot> {
    const a = new RedactionAnnot(doc, Annot.createRaw(doc, Type.e_Redact, pos));
    a.set('C', [1, 0, 0]);
    return a;
  }

  async getOverlayText(): Promise<string> {
    const t = this.dict().OverlayText;
    return typeof t === 'string' ? t : '';
  }

  async setOverlayText(text: string): Promise<void> {
    this.set('OverlayText', text);
  }
}

export { Type as AnnotType, Flag as AnnotFlag };
