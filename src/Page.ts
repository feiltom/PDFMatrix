import { Annot } from './Annot.js';
import { call, PDFNetError } from './errors.js';
import { Rect } from './geom.js';
import type { ObjRef } from './native.js';
import type { PDFDoc } from './PDFDoc.js';
import { getDict } from './sdf.js';

enum Rotate {
  e_0 = 0,
  e_90 = 1,
  e_180 = 2,
  e_270 = 3,
}

enum Box {
  e_media = 0,
  e_crop = 1,
  e_bleed = 2,
  e_trim = 3,
  e_art = 4,
  e_user_crop = 5,
}

/** A page of a PDFDoc. Identified by its object reference, so it stays valid when pages move. */
export class Page {
  static readonly Rotate = Rotate;
  static readonly Box = Box;

  /** @internal */
  constructor(readonly doc: PDFDoc, readonly ref: ObjRef) {}

  // ---------------------------------------------------------- static rotation helpers

  static async rotationToDegree(r: Rotate): Promise<number> {
    return r * 90;
  }

  static async degreeToRotation(degree: number): Promise<Rotate> {
    const d = ((Math.round(degree / 90) * 90) % 360 + 360) % 360;
    return (d / 90) as Rotate;
  }

  static async addRotations(r0: Rotate, r1: Rotate): Promise<Rotate> {
    return ((r0 + r1) % 4) as Rotate;
  }

  static async subtractRotations(r0: Rotate, r1: Rotate): Promise<Rotate> {
    return ((((r0 - r1) % 4) + 4) % 4) as Rotate;
  }

  // ---------------------------------------------------------- identity

  async isValid(): Promise<boolean> {
    return this.doc.isOpen();
  }

  /** 1-based page number, or 0 when the page is not (yet) part of the document. */
  async getIndex(): Promise<number> {
    return this.index0() + 1;
  }

  /** @internal 0-based index, -1 when detached. */
  index0(): number {
    return call('Page.getIndex', () => this.doc.native().pageIndex(this.ref));
  }

  /** @internal */
  requireAttached(location: string): number {
    const i = this.index0();
    if (i < 0) throw new PDFNetError('the page must be added to the document first', location);
    return i;
  }

  // ---------------------------------------------------------- boxes & rotation

  async getBox(type: Box = Box.e_crop): Promise<Rect> {
    if (type === Box.e_user_crop) type = Box.e_crop;
    return Rect.fromArray(call('Page.getBox', () => this.doc.native().pageBox(this.ref, type)));
  }

  async setBox(type: Box, box: Rect): Promise<void> {
    if (type === Box.e_user_crop) type = Box.e_crop;
    call('Page.setBox', () => this.doc.native().setPageBox(this.ref, type, box.toArray()));
  }

  async getMediaBox(): Promise<Rect> {
    return this.getBox(Box.e_media);
  }

  async setMediaBox(box: Rect): Promise<void> {
    return this.setBox(Box.e_media, box);
  }

  async getCropBox(): Promise<Rect> {
    return this.getBox(Box.e_crop);
  }

  async setCropBox(box: Rect): Promise<void> {
    return this.setBox(Box.e_crop, box);
  }

  /** Visible area of the page (crop box intersected with media box). */
  async getVisibleContentBox(): Promise<Rect> {
    const crop = await this.getCropBox();
    const media = await this.getMediaBox();
    const r = new Rect();
    return (await r.intersectRect(crop, media)) ? r : crop;
  }

  async getRotation(): Promise<Rotate> {
    return (call('Page.getRotation', () => this.doc.native().rotation(this.ref)) / 90) as Rotate;
  }

  async setRotation(angle: Rotate): Promise<void> {
    call('Page.setRotation', () => this.doc.native().setRotation(this.ref, angle * 90));
  }

  /** Width of the given box, taking the page rotation into account. */
  async getPageWidth(box: Box = Box.e_crop): Promise<number> {
    const r = await this.getBox(box);
    const rot = await this.getRotation();
    return rot % 2 === 1 ? await r.height() : await r.width();
  }

  /** Height of the given box, taking the page rotation into account. */
  async getPageHeight(box: Box = Box.e_crop): Promise<number> {
    const r = await this.getBox(box);
    const rot = await this.getRotation();
    return rot % 2 === 1 ? await r.width() : await r.height();
  }

  // ---------------------------------------------------------- annotations

  private annotRefs(): ObjRef[] {
    return call('Page.annots', () => this.doc.native().pageAnnots(this.ref));
  }

  async getNumAnnots(): Promise<number> {
    return this.annotRefs().length;
  }

  /** Annotation at `index` (0-based), as a base Annot (use XxxAnnot.createFromAnnot to re-type). */
  async getAnnot(index: number): Promise<Annot> {
    const r = this.annotRefs()[index];
    if (!r) throw new PDFNetError(`annotation index ${index} out of range`, 'Page.getAnnot');
    return new Annot(this.doc, r);
  }

  async annotPushBack(annot: Annot): Promise<void> {
    this.placeAnnot(annot, undefined);
  }

  async annotPushFront(annot: Annot): Promise<void> {
    this.placeAnnot(annot, 0);
  }

  async annotInsert(pos: number, annot: Annot): Promise<void> {
    this.placeAnnot(annot, pos);
  }

  /** Places the annotation and generates its appearance if it has none yet. */
  private placeAnnot(annot: Annot, index: number | undefined): void {
    if (annot.doc !== this.doc) throw new PDFNetError('annotation belongs to another document', 'Page.annotPushBack');
    call('Page.annotPushBack', () => this.doc.native().annotAdd(this.ref, annot.ref, index));
    if (!getDict(this.doc, annot.ref).AP) {
      call('Page.annotPushBack', () => this.doc.native().annotRefresh(annot.ref));
    }
  }

  async annotRemove(annot: Annot | number): Promise<void> {
    const target = typeof annot === 'number' ? this.annotRefs()[annot] : annot.ref;
    if (!target) throw new PDFNetError('annotation not found', 'Page.annotRemove');
    call('Page.annotRemove', () => this.doc.native().annotRemove(this.ref, target));
  }
}

export { Rotate as PageRotate, Box as PageBox };
