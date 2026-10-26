/**
 * Internal helpers over the native JSON view of PDF objects (see native/src/sdf.rs).
 */
import { call } from './errors.js';
import { ColorPt } from './geom.js';
import type { ObjRef } from './native.js';
import type { PDFDoc } from './PDFDoc.js';

export type PdfValue =
  | null
  | boolean
  | number
  | string
  | { $name: string }
  | { $ref: [number, number] }
  | { $bytes: string }
  | { $stream: Record<string, PdfValue> }
  | PdfValue[]
  | { [key: string]: PdfValue };

export type PdfDict = Record<string, PdfValue>;

export const name = (n: string): PdfValue => ({ $name: n });
export const ref = (r: ObjRef): PdfValue => ({ $ref: [r.num, r.generation] });

export function asRef(v: PdfValue | undefined): ObjRef | null {
  if (!v || typeof v !== 'object' || !('$ref' in v)) return null;
  const [num, generation] = (v as { $ref: [number, number] }).$ref;
  return { num, generation };
}

export function asName(v: PdfValue | undefined): string | null {
  return v && typeof v === 'object' && '$name' in v ? (v as { $name: string }).$name : null;
}

export function asNumbers(v: PdfValue | undefined): number[] {
  return Array.isArray(v) ? v.filter((x): x is number => typeof x === 'number') : [];
}

export function sameRef(a: ObjRef, b: ObjRef): boolean {
  return a.num === b.num && a.generation === b.generation;
}

/** Dictionary (or stream dictionary) of an indirect object. */
export function getDict(doc: PDFDoc, r: ObjRef): PdfDict {
  const v = call('SDF.get', () => doc.native().objGet(r)) as PdfValue;
  if (v && typeof v === 'object' && !Array.isArray(v)) {
    if ('$stream' in v) return v.$stream as PdfDict;
    if (!('$ref' in v) && !('$name' in v) && !('$bytes' in v)) return v as PdfDict;
  }
  return {};
}

/** Sets a key; `null` removes it. */
export function setKey(doc: PDFDoc, r: ObjRef, key: string, value: PdfValue): void {
  call('SDF.set', () => doc.native().objSet(r, key, value));
}

export function createObj(doc: PDFDoc, value: PdfValue): ObjRef {
  return call('SDF.create', () => doc.native().objCreate(value));
}

/** ColorPt → PDF color array with `compNum` components (0 = transparent / none). */
export function colorArray(c: ColorPt, compNum = 3): number[] {
  return c.c.slice(0, compNum);
}

/** PDF color array → ColorPt (+ component count). */
export function toColorPt(v: PdfValue | undefined): { color: ColorPt; compNum: number } {
  const n = asNumbers(v);
  return { color: new ColorPt(n[0] ?? 0, n[1] ?? 0, n[2] ?? 0, n[3] ?? 0), compNum: n.length };
}
