import { Annot } from './Annot.js';
import { call, PDFNetError } from './errors.js';
import { ColorPt, Rect } from './geom.js';
import type { ObjRef } from './native.js';
import type { Page } from './Page.js';
import type { PDFDoc } from './PDFDoc.js';
import { asName, asRef, colorArray, getDict, sameRef, setKey, type PdfDict, type PdfValue } from './sdf.js';

type FieldInfo = ReturnType<ReturnType<PDFDoc['native']>['fields']>[number];

enum Type {
  e_button = 0,
  e_check = 1,
  e_radio = 2,
  e_text = 3,
  e_choice = 4,
  e_signature = 5,
  e_null = 6,
}

/** PDFNet Field.Flag → bit mask in /Ff. */
enum Flag {
  e_read_only = 0,
  e_required = 1,
  e_no_export = 2,
  e_pushbutton_flag = 3,
  e_radio_flag = 4,
  e_toggle_to_off = 5,
  e_radios_in_unison = 6,
  e_multiline = 7,
  e_password = 8,
  e_file_select = 9,
  e_no_spellcheck = 10,
  e_no_scroll = 11,
  e_comb = 12,
  e_rich_text = 13,
  e_combo = 14,
  e_edit = 15,
  e_sort = 16,
  e_multiselect = 17,
  e_commit_on_sel_change = 18,
}

const FLAG_BITS: Record<Flag, number> = {
  [Flag.e_read_only]: 1 << 0,
  [Flag.e_required]: 1 << 1,
  [Flag.e_no_export]: 1 << 2,
  [Flag.e_pushbutton_flag]: 1 << 16,
  [Flag.e_radio_flag]: 1 << 15,
  [Flag.e_toggle_to_off]: 1 << 14,
  [Flag.e_radios_in_unison]: 1 << 25,
  [Flag.e_multiline]: 1 << 12,
  [Flag.e_password]: 1 << 13,
  [Flag.e_file_select]: 1 << 20,
  [Flag.e_no_spellcheck]: 1 << 22,
  [Flag.e_no_scroll]: 1 << 23,
  [Flag.e_comb]: 1 << 24,
  [Flag.e_rich_text]: 1 << 25,
  [Flag.e_combo]: 1 << 17,
  [Flag.e_edit]: 1 << 18,
  [Flag.e_sort]: 1 << 19,
  [Flag.e_multiselect]: 1 << 21,
  [Flag.e_commit_on_sel_change]: 1 << 26,
};

const KIND_TO_TYPE: Record<string, Type> = {
  button: Type.e_button,
  check: Type.e_check,
  radio: Type.e_radio,
  text: Type.e_text,
  combo: Type.e_choice,
  list: Type.e_choice,
  signature: Type.e_signature,
};

const TYPE_TO_FT: Record<number, { ft: string; flags: number }> = {
  [Type.e_button]: { ft: 'Btn', flags: 1 << 16 },
  [Type.e_check]: { ft: 'Btn', flags: 0 },
  [Type.e_radio]: { ft: 'Btn', flags: (1 << 15) | (1 << 14) },
  [Type.e_text]: { ft: 'Tx', flags: 0 },
  [Type.e_choice]: { ft: 'Ch', flags: 0 },
  [Type.e_signature]: { ft: 'Sig', flags: 0 },
};

/** PDFNet.Field — an interactive form field (AcroForm). */
export class Field {
  static readonly Type = Type;
  static readonly Flag = Flag;
  static readonly TextJustification = { e_left_justified: 0, e_centered: 1, e_right_justified: 2 } as const;

  /** @internal */
  constructor(readonly doc: PDFDoc, readonly ref: ObjRef | null) {}

  /** @internal */
  info(): FieldInfo {
    if (!this.ref) throw new PDFNetError('invalid field', 'Field');
    const r = this.ref;
    const f = call('Field', () => this.doc.native().fields()).find((x) => sameRef(x.obj, r));
    if (!f) throw new PDFNetError('field no longer exists', 'Field');
    return f;
  }

  async isValid(): Promise<boolean> {
    if (!this.ref) return false;
    try {
      this.info();
      return true;
    } catch {
      return false;
    }
  }

  async getName(): Promise<string> {
    return this.info().name;
  }

  async getPartialName(): Promise<string> {
    return this.info().name.split('.').pop() ?? '';
  }

  async getType(): Promise<Type> {
    return KIND_TO_TYPE[this.info().kind] ?? Type.e_null;
  }

  async getValueAsString(): Promise<string> {
    return this.info().value;
  }

  async getValueAsBool(): Promise<boolean> {
    const v = this.info().value;
    return v !== '' && v !== 'Off';
  }

  /** Sets the value and regenerates the widget appearances. */
  async setValueAsString(value: string): Promise<void> {
    const f = this.info();
    call('Field.setValue', () => this.doc.native().fieldSetValue(f.obj, value));
  }

  async setValueAsBool(value: boolean): Promise<void> {
    await this.setValueAsString(value ? 'Yes' : 'Off');
  }

  async setValue(value: string | boolean): Promise<void> {
    if (typeof value === 'boolean') return this.setValueAsBool(value);
    return this.setValueAsString(value);
  }

  async getOptCount(): Promise<number> {
    return this.info().options.length;
  }

  async getOpt(index: number): Promise<string> {
    return this.info().options[index]?.display ?? '';
  }

  /** Export value of option `index`. */
  async getOptExportValue(index: number): Promise<string> {
    return this.info().options[index]?.exportValue ?? '';
  }

  async getFlag(flag: Flag): Promise<boolean> {
    return (this.info().flags & FLAG_BITS[flag]) !== 0;
  }

  async setFlag(flag: Flag, value: boolean): Promise<void> {
    const f = this.info();
    const bits = value ? f.flags | FLAG_BITS[flag] : f.flags & ~FLAG_BITS[flag];
    setKey(this.doc, f.obj, 'Ff', bits);
    call('Field.setFlag', () => this.doc.native().fieldRefresh(f.obj));
  }

  async getMaxLen(): Promise<number> {
    const m = getDict(this.doc, this.info().obj).MaxLen;
    return typeof m === 'number' ? m : -1;
  }

  async setMaxLen(maxLen: number): Promise<void> {
    const f = this.info();
    setKey(this.doc, f.obj, 'MaxLen', maxLen < 0 ? null : maxLen);
    call('Field.setMaxLen', () => this.doc.native().fieldRefresh(f.obj));
  }

  async getJustification(): Promise<number> {
    const q = getDict(this.doc, this.info().obj).Q;
    return typeof q === 'number' ? q : 0;
  }

  async setJustification(j: number): Promise<void> {
    const f = this.info();
    setKey(this.doc, f.obj, 'Q', j);
    call('Field.setJustification', () => this.doc.native().fieldRefresh(f.obj));
  }

  /** Choice fields: replaces the options (export value = display text). */
  async setOptions(options: string[]): Promise<void> {
    const f = this.info();
    setKey(this.doc, f.obj, 'Opt', options);
    call('Field.setOptions', () => this.doc.native().fieldRefresh(f.obj));
  }

  async refreshAppearance(): Promise<void> {
    const f = this.info();
    call('Field.refreshAppearance', () => this.doc.native().fieldRefresh(f.obj));
  }

  async getNumWidgets(): Promise<number> {
    return this.info().widgets.length;
  }

  async getWidget(index: number): Promise<WidgetAnnot> {
    const w = this.info().widgets[index];
    if (!w) throw new PDFNetError(`widget index ${index} out of range`, 'Field.getWidget');
    return new WidgetAnnot(this.doc, w);
  }

  /** Burns every widget of the field into its page. */
  async flatten(_page?: Page): Promise<void> {
    for (const w of this.info().widgets) {
      const p = asRef(getDict(this.doc, w).P);
      if (p) call('Field.flatten', () => this.doc.native().annotFlatten(p, w));
    }
  }

  async isLockedByDigitalSignature(): Promise<boolean> {
    return false;
  }
}

export class FieldIterator {
  #i = 0;

  /** @internal */
  constructor(private readonly doc: PDFDoc, private readonly refs: ObjRef[]) {}

  async hasNext(): Promise<boolean> {
    return this.#i < this.refs.length;
  }

  async next(): Promise<void> {
    this.#i++;
  }

  async current(): Promise<Field> {
    return new Field(this.doc, this.refs[this.#i] ?? null);
  }
}

/** @internal Creates a form field of a PDFNet type. */
export function createField(doc: PDFDoc, fieldName: string, type: Type): Field {
  const t = TYPE_TO_FT[type];
  if (!t) throw new PDFNetError(`cannot create a field of type ${type}`, 'PDFDoc.fieldCreate');
  return new Field(doc, call('PDFDoc.fieldCreate', () => doc.native().fieldCreate(fieldName, t.ft, t.flags)));
}

interface WidgetLook {
  borderColor?: number[];
  backgroundColor?: number[];
  borderWidth?: number;
  onState?: string;
  caption?: string;
}

/** PDFNet.WidgetAnnot — the visual part of a form field on a page. */
export class WidgetAnnot extends Annot {
  /** Creates a widget for `field` (a Field or the name of an existing field). */
  static async create(doc: PDFDoc, pos: Rect, field: Field | string = ''): Promise<WidgetAnnot> {
    const f = typeof field === 'string' ? await doc.getField(field) : field;
    return WidgetAnnot.make(doc, pos, f, {});
  }

  /** @internal */
  static make(doc: PDFDoc, pos: Rect, field: Field, look: WidgetLook): WidgetAnnot {
    const f = field.info();
    const r = call('WidgetAnnot.create', () => doc.native().widgetCreate(f.obj, pos.toArray(), look));
    return new this(doc, r);
  }

  async getField(): Promise<Field> {
    const r = this.ref;
    const f = call('WidgetAnnot.getField', () => this.doc.native().fields()).find((x) => x.widgets.some((w) => sameRef(w, r)));
    return new Field(this.doc, f?.obj ?? null);
  }

  private mk(): PdfDict {
    const mk = this.dict().MK;
    return mk && typeof mk === 'object' && !Array.isArray(mk) ? { ...(mk as PdfDict) } : {};
  }

  private setMk(key: string, value: PdfValue): void {
    const mk = this.mk();
    if (value === null) delete mk[key];
    else mk[key] = value;
    this.set('MK', mk);
  }

  async setBackgroundColor(color: ColorPt, compNum = 3): Promise<void> {
    this.setMk('BG', compNum === 0 ? null : colorArray(color, compNum));
    await this.refreshAppearance();
  }

  async setBorderColor(color: ColorPt, compNum = 3): Promise<void> {
    this.setMk('BC', compNum === 0 ? null : colorArray(color, compNum));
    await this.refreshAppearance();
  }

  /** Font size used for the field text (0 = auto). */
  async setFontSize(size: number): Promise<void> {
    const da = typeof this.dict().DA === 'string' ? (this.dict().DA as string) : '/Helv 0 Tf 0 g';
    this.set('DA', /Tf/.test(da) ? da.replace(/(\/\S+)\s+[\d.]+\s+Tf/, `$1 ${size} Tf`) : `/Helv ${size} Tf ${da}`);
    await this.refreshAppearance();
  }

  async setTextColor(color: ColorPt, compNum = 3): Promise<void> {
    const da = typeof this.dict().DA === 'string' ? (this.dict().DA as string) : '/Helv 0 Tf 0 g';
    const font = /(\/\S+\s+[\d.]+\s+Tf)/.exec(da)?.[1] ?? '/Helv 0 Tf';
    const c = colorArray(color, compNum);
    const op = c.length === 1 ? 'g' : c.length === 4 ? 'k' : 'rg';
    this.set('DA', `${font} ${c.join(' ')} ${op}`);
    await this.refreshAppearance();
  }

  async getOnStateName(): Promise<string> {
    const ap = this.dict().AP as PdfDict | undefined;
    const n = ap && (ap.N as PdfDict | undefined);
    return n && typeof n === 'object' ? (Object.keys(n).find((k) => k !== 'Off') ?? '') : '';
  }

  async getAppearanceState(): Promise<string> {
    return asName(this.dict().AS) ?? '';
  }
}

export class TextWidget extends WidgetAnnot {
  static async create(doc: PDFDoc, pos: Rect, fieldName = ''): Promise<TextWidget> {
    const name = fieldName || `TextField${Date.now()}`;
    return TextWidget.make(doc, pos, createField(doc, name, Type.e_text), {}) as TextWidget;
  }

  async getText(): Promise<string> {
    return (await this.getField()).getValueAsString();
  }

  async setText(text: string): Promise<void> {
    await (await this.getField()).setValueAsString(text);
  }
}

export class CheckBoxWidget extends WidgetAnnot {
  static async create(doc: PDFDoc, pos: Rect, fieldName = ''): Promise<CheckBoxWidget> {
    const name = fieldName || `CheckBox${Date.now()}`;
    return CheckBoxWidget.make(doc, pos, createField(doc, name, Type.e_check), { borderColor: [0], borderWidth: 1 }) as CheckBoxWidget;
  }

  async isChecked(): Promise<boolean> {
    const s = await this.getAppearanceState();
    return s !== '' && s !== 'Off';
  }

  async setChecked(checked: boolean): Promise<void> {
    await (await this.getField()).setValueAsBool(checked);
  }
}

export class RadioButtonWidget extends WidgetAnnot {
  async isEnabled(): Promise<boolean> {
    const s = await this.getAppearanceState();
    return s !== '' && s !== 'Off';
  }

  async enableButton(): Promise<void> {
    await (await this.getField()).setValueAsString(await this.getOnStateName());
  }

  async getGroup(): Promise<RadioButtonGroup> {
    return new RadioButtonGroup(this.doc, await this.getField());
  }
}

export class RadioButtonGroup {
  /** @internal */
  constructor(readonly doc: PDFDoc, readonly field: Field) {}

  static async create(doc: PDFDoc, groupName = ''): Promise<RadioButtonGroup> {
    const name = groupName || `RadioGroup${Date.now()}`;
    return new RadioButtonGroup(doc, createField(doc, name, Type.e_radio));
  }

  static async createFromField(field: Field): Promise<RadioButtonGroup> {
    return new RadioButtonGroup(field.doc, field);
  }

  /** Adds a button; `onState` is its export value (defaults to "1", "2", …). */
  async add(pos: Rect, onState = ''): Promise<RadioButtonWidget> {
    const state = onState || String((await this.getNumButtons()) + 1);
    return RadioButtonWidget.make(this.doc, pos, this.field, { onState: state, borderColor: [0], borderWidth: 1 }) as RadioButtonWidget;
  }

  async getNumButtons(): Promise<number> {
    return this.field.info().widgets.length;
  }

  async getButton(index: number): Promise<RadioButtonWidget> {
    const w = this.field.info().widgets[index];
    if (!w) throw new PDFNetError(`button index ${index} out of range`, 'RadioButtonGroup.getButton');
    return new RadioButtonWidget(this.doc, w);
  }

  async getSelectedButton(): Promise<RadioButtonWidget | null> {
    const f = this.field.info();
    const i = f.onStates.findIndex((s) => s.includes(f.value));
    return i >= 0 && f.value !== 'Off' ? new RadioButtonWidget(this.doc, f.widgets[i]) : null;
  }

  async getField(): Promise<Field> {
    return this.field;
  }

  /** Places every button of the group on `page`. */
  async addGroupButtonsToPage(page: Page): Promise<void> {
    for (const w of this.field.info().widgets) await page.annotPushBack(new Annot(this.doc, w));
  }
}

class ChoiceWidget extends WidgetAnnot {
  async addOption(option: string): Promise<void> {
    const f = await this.getField();
    const opts = f.info().options.map((o) => o.display);
    await f.setOptions([...opts, option]);
  }

  async addOptions(options: string[]): Promise<void> {
    const f = await this.getField();
    await f.setOptions([...f.info().options.map((o) => o.display), ...options]);
  }

  async getNumOptions(): Promise<number> {
    return (await this.getField()).getOptCount();
  }

  async getOption(index: number): Promise<string> {
    return (await this.getField()).getOpt(index);
  }

  async getSelectedOption(): Promise<string> {
    return (await this.getField()).getValueAsString();
  }

  async setSelectedOption(value: string): Promise<void> {
    await (await this.getField()).setValueAsString(value);
  }
}

export class ComboBoxWidget extends ChoiceWidget {
  static async create(doc: PDFDoc, pos: Rect, fieldName = ''): Promise<ComboBoxWidget> {
    const name = fieldName || `ComboBox${Date.now()}`;
    const field = createField(doc, name, Type.e_choice);
    setKey(doc, field.info().obj, 'Ff', 1 << 17);
    return ComboBoxWidget.make(doc, pos, field, { borderColor: [0], borderWidth: 1 }) as ComboBoxWidget;
  }
}

export class ListBoxWidget extends ChoiceWidget {
  static async create(doc: PDFDoc, pos: Rect, fieldName = ''): Promise<ListBoxWidget> {
    const name = fieldName || `ListBox${Date.now()}`;
    return ListBoxWidget.make(doc, pos, createField(doc, name, Type.e_choice), { borderColor: [0], borderWidth: 1 }) as ListBoxWidget;
  }
}

export class PushButtonWidget extends WidgetAnnot {
  static async create(doc: PDFDoc, pos: Rect, fieldName = ''): Promise<PushButtonWidget> {
    const name = fieldName || `PushButton${Date.now()}`;
    return PushButtonWidget.make(doc, pos, createField(doc, name, Type.e_button), {
      borderColor: [0], backgroundColor: [0.85], borderWidth: 1,
    }) as PushButtonWidget;
  }

  async setStaticCaptionText(caption: string): Promise<void> {
    const mk = this.dict().MK as PdfDict | undefined;
    this.set('MK', { ...(mk ?? {}), CA: caption });
    await this.refreshAppearance();
  }

  async getStaticCaptionText(): Promise<string> {
    const mk = this.dict().MK as PdfDict | undefined;
    return typeof mk?.CA === 'string' ? mk.CA : '';
  }
}

/** Signature field widget (signing itself comes with the security phase). */
export class SignatureWidget extends WidgetAnnot {
  /** Widget for a field created with doc.createDigitalSignatureField(). */
  static async createWithDigitalSignatureField(doc: PDFDoc, pos: Rect, field: { ref: ObjRef }): Promise<SignatureWidget> {
    return SignatureWidget.make(doc, pos, new Field(doc, field.ref), {}) as SignatureWidget;
  }

  static async create(doc: PDFDoc, pos: Rect, fieldName = ''): Promise<SignatureWidget> {
    const name = fieldName || `Signature${Date.now()}`;
    return SignatureWidget.make(doc, pos, createField(doc, name, Type.e_signature), {}) as SignatureWidget;
  }
}

export { Type as FieldType, Flag as FieldFlag };
