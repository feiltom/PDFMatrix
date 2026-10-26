import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { PDFNet } from '../dist/index.js';

const fixture = (name) => new URL(`./fixtures/${name}`, import.meta.url).pathname;
const open = (name) => PDFNet.PDFDoc.createFromFilePath(fixture(name));
const reload = async (doc) => PDFNet.PDFDoc.createFromBuffer(await doc.saveMemoryBuffer());

async function text(doc, pageNum = 1) {
  const txt = await PDFNet.TextExtractor.create();
  await txt.begin(await doc.getPage(pageNum));
  return txt.getAsText();
}

/** RGB sampler over a 72-dpi render: at(x, y) in PDF user space of an unrotated page. */
async function sampler(doc, pageNum = 1) {
  const page = await doc.getPage(pageNum);
  const draw = await PDFNet.PDFDraw.create(72);
  const bmp = await draw.getBitmap(page, PDFNet.PDFDraw.PixelFormat.e_rgb);
  const at = (x, y) => {
    const o = (bmp.height - Math.round(y)) * bmp.stride + Math.round(x) * 3;
    return [bmp.buf[o], bmp.buf[o + 1], bmp.buf[o + 2]];
  };
  /** Counts pixels in a box matching a predicate. */
  const count = (x1, y1, x2, y2, pred) => {
    let n = 0;
    for (let x = x1; x < x2; x++) for (let y = y1; y < y2; y++) if (pred(at(x, y))) n++;
    return n;
  };
  return { at, count, bmp };
}

const isRed = ([r, g, b]) => r > 180 && g < 90 && b < 90;
const isBlue = ([r, g, b]) => b > 150 && r < 100 && g < 100;
const isDark = ([r, g, b]) => r + g + b < 300;

describe('Forms: reading', () => {
  it('lists fields with names, types, values and options', async () => {
    const doc = await open('form.pdf');
    const fields = [];
    for (const it = await doc.getFieldIterator(); await it.hasNext(); it.next()) {
      const f = await it.current();
      fields.push([await f.getName(), await f.getType(), await f.getValueAsString()]);
    }
    const T = PDFNet.Field.Type;
    assert.deepEqual(fields, [
      ['name', T.e_text, 'Jean Dupont'],
      ['agree', T.e_check, 'Off'],
      ['size', T.e_radio, 'M'],
      ['country', T.e_choice, 'FR'],
      ['lang', T.e_choice, 'fr'],
    ]);
    const country = await doc.getField('country');
    assert.equal(await country.getOptCount(), 3);
    assert.equal(await country.getOpt(1), 'Belgique');
    assert.equal(await country.getOptExportValue(1), 'BE');
    assert.equal(await (await doc.getField('missing')).isValid(), false);
  });
});

describe('Forms: filling', () => {
  it('fills a text field and draws it with the DA color', async () => {
    const doc = await open('form.pdf');
    await (await doc.getField('name')).setValueAsString('Marie Curie');
    const re = await reload(doc);
    assert.equal(await (await re.getField('name')).getValueAsString(), 'Marie Curie');
    const { count } = await sampler(re);
    assert.ok(count(100, 700, 300, 720, isBlue) > 20, 'blue text drawn inside the field');
  });

  it('checks a checkbox using its own on-state name and appearance', async () => {
    const doc = await open('form.pdf');
    const agree = await doc.getField('agree');
    await agree.setValueAsBool(true);
    assert.equal(await agree.getValueAsString(), 'Oui');
    assert.equal(await agree.getValueAsBool(), true);
    const { at } = await sampler(await reload(doc));
    assert.ok(isBlue(at(110, 670)), `center of the checkbox: ${at(110, 670)}`);
    await agree.setValueAsBool(false);
    assert.equal(await agree.getValueAsString(), 'Off');
  });

  it('selects a radio button and rejects unknown states', async () => {
    const doc = await open('form.pdf');
    const size = await doc.getField('size');
    await size.setValueAsString('S');
    const group = await PDFNet.RadioButtonGroup.createFromField(size);
    assert.equal(await (await group.getButton(0)).isEnabled(), true);
    assert.equal(await (await group.getButton(1)).isEnabled(), false);
    await assert.rejects(size.setValueAsString('XL'), /no state 'XL'/);
  });

  it('fills choice fields, then flattens: values become page text', async () => {
    const doc = await open('form.pdf');
    await (await doc.getField('country')).setValueAsString('BE');
    await (await doc.getField('lang')).setValueAsString('de');
    await doc.flattenAnnotations(true);
    const re = await reload(doc);
    assert.equal(await (await re.getPage(1)).getNumAnnots(), 0);
    assert.equal(await (await re.getFieldIterator()).hasNext(), false);
    const t = await text(re);
    assert.match(t, /Jean Dupont/, 'text field without appearance was generated (NeedAppearances)');
    assert.match(t, /Belgique/);
    assert.match(t, /en\s+fr\s+de/);
  });

  it('changes flags and justification', async () => {
    const doc = await open('form.pdf');
    const f = await doc.getField('name');
    assert.equal(await f.getFlag(PDFNet.Field.Flag.e_read_only), false);
    await f.setFlag(PDFNet.Field.Flag.e_read_only, true);
    await f.setJustification(PDFNet.Field.TextJustification.e_right_justified);
    const re = await reload(doc);
    const g = await re.getField('name');
    assert.equal(await g.getFlag(PDFNet.Field.Flag.e_read_only), true);
    assert.equal(await g.getJustification(), 2);
  });
});

describe('Forms: creating', () => {
  it('creates every widget kind, fills and flattens them', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const page = await doc.pageCreate();
    await doc.pagePushBack(page);

    const name = await PDFNet.TextWidget.create(doc, new PDFNet.Rect(50, 700, 300, 720), 'person.name');
    await name.setBorderColor(await PDFNet.ColorPt.init(0, 0, 0));
    await page.annotPushBack(name);
    const check = await PDFNet.CheckBoxWidget.create(doc, new PDFNet.Rect(50, 660, 70, 680), 'person.agree');
    await page.annotPushBack(check);
    const group = await PDFNet.RadioButtonGroup.create(doc, 'size');
    await group.add(new PDFNet.Rect(50, 620, 70, 640), 'S');
    await group.add(new PDFNet.Rect(80, 620, 100, 640), 'M');
    await group.addGroupButtonsToPage(page);
    const combo = await PDFNet.ComboBoxWidget.create(doc, new PDFNet.Rect(50, 580, 200, 600), 'city');
    await combo.addOptions(['Paris', 'Lyon']);
    await page.annotPushBack(combo);
    const list = await PDFNet.ListBoxWidget.create(doc, new PDFNet.Rect(50, 500, 150, 560), 'colors');
    await list.addOptions(['Rouge', 'Vert', 'Bleu']);
    await page.annotPushBack(list);
    const button = await PDFNet.PushButtonWidget.create(doc, new PDFNet.Rect(300, 500, 400, 530), 'go');
    await button.setStaticCaptionText('Envoyer');
    await page.annotPushBack(button);

    let re = await reload(doc);
    const names = [];
    for (const it = await re.getFieldIterator(); await it.hasNext(); it.next()) names.push(await (await it.current()).getName());
    assert.deepEqual(names, ['person.name', 'person.agree', 'size', 'city', 'colors', 'go']);
    assert.equal(await (await re.getPage(1)).getNumAnnots(), 7);

    await (await re.getField('person.name')).setValueAsString('Ada Lovelace');
    await (await re.getField('person.agree')).setValueAsBool(true);
    await (await re.getField('size')).setValueAsString('M');
    await (await re.getField('city')).setValueAsString('Lyon');
    await (await re.getField('colors')).setValueAsString('Vert');
    assert.equal(await (await re.getField('size')).getValueAsString(), 'M');

    const { at } = await sampler(re);
    assert.ok(isDark(at(60, 668)), 'check mark drawn');
    assert.ok(isDark(at(90, 630)), 'selected radio dot drawn');
    assert.ok(!isDark(at(60, 630)), 'other radio empty');

    await re.flattenAnnotations();
    re = await reload(re);
    const t = await text(re);
    for (const s of ['Ada Lovelace', 'Lyon', 'Rouge', 'Vert', 'Envoyer']) assert.match(t, new RegExp(s));
  });

  it('creates a field with fieldCreate + WidgetAnnot.create (PDFNet sample style)', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const page = await doc.pageCreate();
    doc.pagePushBack(page);
    const field = await doc.fieldCreate('address', PDFNet.Field.Type.e_text, '1 rue de la Paix');
    const widget = await PDFNet.WidgetAnnot.create(doc, new PDFNet.Rect(50, 700, 400, 720), field);
    await page.annotPushBack(widget);
    await doc.refreshFieldAppearances();
    await doc.flattenAnnotations(true);
    assert.match(await text(doc), /1 rue de la Paix/);
  });
});

describe('Annotations', () => {
  it('creates, reads back and removes markup annotations', async () => {
    const doc = await open('text.pdf');
    const page = await doc.getPage(1);
    const sq = await PDFNet.SquareAnnot.create(doc, new PDFNet.Rect(300, 300, 400, 400));
    await sq.setColor(await PDFNet.ColorPt.init(1, 0, 0));
    await sq.setBorderStyle(await PDFNet.AnnotBorderStyle.create(PDFNet.AnnotBorderStyle.Style.e_solid, 4));
    await sq.setContents('A red square');
    await sq.setTitle('Reviewer');
    await page.annotPushBack(sq);

    const line = await PDFNet.LineAnnot.create(doc, new PDFNet.Rect(0, 0, 1, 1));
    await line.setStartPoint(new PDFNet.Point(100, 100));
    await line.setEndPoint(new PDFNet.Point(200, 150));
    await line.setEndStyle(PDFNet.LineAnnot.EndingStyle.e_ClosedArrow);
    await page.annotPushBack(line);

    const note = await PDFNet.TextAnnot.create(doc, new PDFNet.Point(500, 750), 'Sticky note');
    await page.annotPushBack(note);
    const link = await PDFNet.LinkAnnot.create(doc, new PDFNet.Rect(72, 700, 200, 720), await PDFNet.Action.createURI(doc, 'https://example.org'));
    await page.annotPushBack(link);

    const re = await reload(doc);
    const p = await re.getPage(1);
    assert.equal(await p.getNumAnnots(), 4);
    const a0 = await PDFNet.Markup.createFromAnnot(await p.getAnnot(0));
    assert.equal(await a0.getType(), PDFNet.Annot.Type.e_Square);
    assert.equal(await a0.getContents(), 'A red square');
    assert.equal(await a0.getTitle(), 'Reviewer');
    assert.equal((await a0.getBorderStyle()).width, 4);
    const l = await PDFNet.LineAnnot.createFromAnnot(await p.getAnnot(1));
    assert.equal((await l.getEndPoint()).x, 200);
    const r = await l.getRect();
    assert.ok(r.x1 < 100 && r.x2 > 200 && r.y2 > 150, 'line rect recomputed to include the arrow');
    const lk = await PDFNet.LinkAnnot.createFromAnnot(await p.getAnnot(3));
    assert.equal(await (await lk.getAction()).getURI(), 'https://example.org');
    assert.ok((await (await p.getAnnot(0)).getUniqueID()).length > 0);

    const { at } = await sampler(re);
    assert.ok(isRed(at(301, 350)), `square border is red: ${at(301, 350)}`);
    assert.ok(!isRed(at(350, 350)), 'no interior fill');

    await p.annotRemove(0);
    assert.equal(await p.getNumAnnots(), 3);
  });

  it('highlights text with a multiply blend and opacity', async () => {
    const doc = await open('text.pdf');
    const page = await doc.getPage(1);
    const hl = await PDFNet.HighlightAnnot.create(doc, new PDFNet.Rect(70, 690, 220, 725));
    await hl.setOpacity(0.5);
    await page.annotPushBack(hl);
    const { at } = await sampler(doc);
    const [r, g, b] = at(75, 692);
    assert.ok(r > 200 && g > 200 && b < 230 && b > 60, `semi-transparent yellow: ${[r, g, b]}`);
  });

  it('generates FreeText and rubber-stamp appearances that survive flattening', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const page = await doc.pageCreate();
    doc.pagePushBack(page);
    const ft = await PDFNet.FreeTextAnnot.create(doc, new PDFNet.Rect(50, 600, 300, 700));
    await ft.setContents('Commentaire libre sur plusieurs mots qui doit passer à la ligne');
    await ft.setFontSize(14);
    await ft.setTextColor(await PDFNet.ColorPt.init(0, 0, 1));
    await page.annotPushBack(ft);
    const stamp = await PDFNet.RubberStampAnnot.create(doc, new PDFNet.Rect(350, 600, 550, 660), PDFNet.RubberStampAnnot.Icon.e_Approved);
    await page.annotPushBack(stamp);
    const ink = await PDFNet.InkAnnot.create(doc, new PDFNet.Rect(0, 0, 1, 1));
    await ink.setPoint(0, 0, new PDFNet.Point(100, 100));
    await ink.setPoint(0, 1, new PDFNet.Point(150, 180));
    await ink.setPoint(1, 0, new PDFNet.Point(200, 100));
    await page.annotPushBack(ink);

    await doc.flattenAnnotations();
    const re = await reload(doc);
    assert.equal(await (await re.getPage(1)).getNumAnnots(), 0);
    const t = await text(re);
    assert.match(t, /Commentaire libre/);
    assert.match(t, /\nla ligne|passer à/, 'wrapped');
    assert.match(t, /APPROVED/);
    const { count } = await sampler(re);
    assert.ok(count(95, 95, 155, 185, isDark) > 30, 'ink stroke burned into the page');
  });
});

describe('Stamper', () => {
  it('stamps a centered, rotated, semi-transparent watermark on all pages', async () => {
    const doc = await open('text.pdf');
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_relative_scale, 0.6, 0.6);
    await s.setRotation(45);
    await s.setOpacity(0.5);
    await s.setFontColor(await PDFNet.ColorPt.init(1, 0, 0));
    await s.stampText(doc, 'CONFIDENTIAL', await PDFNet.PageSet.createRange(1, 3));
    assert.equal(await PDFNet.Stamper.hasStamps(doc, await PDFNet.PageSet.createSinglePage(2)), true);
    const re = await reload(doc);
    for (let p = 1; p <= 3; p++) assert.match(await text(re, p), /CONFIDENTIAL/);
    const { count } = await sampler(re);
    const reddish = ([r, g, b]) => r > 200 && g < 200 && g > 60 && Math.abs(g - b) < 30;
    assert.ok(count(256, 346, 356, 446, reddish) > 50, 'pink (50% red) pixels around the page center');
  });

  it('aligns stamps in display space on rotated pages', async () => {
    const doc = await open('text.pdf');
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_font_size, 30, 30);
    await s.setAlignment(PDFNet.Stamper.HorizontalAlignment.e_horizontal_center, PDFNet.Stamper.VerticalAlignment.e_vertical_top);
    await s.setPosition(0, 10);
    await s.stampText(doc, 'HEADER', await PDFNet.PageSet.createSinglePage(3));
    const draw = await PDFNet.PDFDraw.create(36);
    const bmp = await draw.getBitmap(await doc.getPage(3), PDFNet.PDFDraw.PixelFormat.e_rgb);
    let firstRed = -1;
    for (let y = 0; y < bmp.height && firstRed < 0; y++) {
      for (let x = 0; x < bmp.width; x++) {
        if (isRed([...bmp.buf.subarray(y * bmp.stride + x * 3, y * bmp.stride + x * 3 + 3)])) {
          firstRed = y;
          break;
        }
      }
    }
    assert.ok(firstRed >= 0 && firstRed < bmp.height * 0.1, `stamp near the top of the displayed page (row ${firstRed}/${bmp.height})`);
  });

  it('deletes stamps (content and annotation stamps)', async () => {
    const doc = await open('text.pdf');
    const all = await PDFNet.PageSet.createRange(1, 3);
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_font_size, 20, 20);
    await s.stampText(doc, 'DRAFT', all);
    await s.setAsAnnotation(true);
    await s.stampText(doc, 'ANNOTATED', all);
    assert.equal(await (await doc.getPage(1)).getNumAnnots(), 1);
    await PDFNet.Stamper.deleteStamps(doc, all);
    assert.equal(await PDFNet.Stamper.hasStamps(doc, all), false);
    assert.equal(await (await doc.getPage(1)).getNumAnnots(), 0);
    assert.doesNotMatch(await text(doc), /DRAFT/);
  });

  it('stamps an image with alpha and a page of another document', async () => {
    const doc = await open('text.pdf');
    const img = await PDFNet.Image.createFromFile(doc, fixture('red-alpha.png'));
    assert.equal(await img.getImageWidth(), 4);
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_absolute_size, 100, 100);
    await s.setAlignment(PDFNet.Stamper.HorizontalAlignment.e_horizontal_left, PDFNet.Stamper.VerticalAlignment.e_vertical_bottom);
    await s.stampImage(doc, img, await PDFNet.PageSet.createSinglePage(1));
    const { at } = await sampler(doc);
    const [r, g, b] = at(50, 50);
    assert.ok(r > 240 && g > 100 && g < 160 && Math.abs(g - b) < 10, `half-transparent red over white: ${[r, g, b]}`);

    const other = await open('other.pdf');
    const ps = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_relative_scale, 0.3, 0.3);
    await ps.setAlignment(PDFNet.Stamper.HorizontalAlignment.e_horizontal_right, PDFNet.Stamper.VerticalAlignment.e_vertical_bottom);
    await ps.stampPage(doc, await other.getPage(2), await PDFNet.PageSet.createSinglePage(1));
    assert.match(await text(await reload(doc)), /Other B/);
  });

  it('writes Unicode text with an embedded TrueType font', async () => {
    const doc = await open('text.pdf');
    const font = await PDFNet.Font.createTrueTypeFont(doc, fixture('DejaVuSans.ttf'));
    assert.match(await font.getName(), /DejaVuSans/);
    assert.ok((await font.getAscent()) > 700);
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_font_size, 18, 18);
    await s.setFont(font);
    await s.setAlignment(PDFNet.Stamper.HorizontalAlignment.e_horizontal_left, PDFNet.Stamper.VerticalAlignment.e_vertical_bottom);
    await s.setPosition(72, 72);
    await s.stampText(doc, 'Привет — Ελληνικά ✓ €', await PDFNet.PageSet.createSinglePage(1));
    assert.match(await text(await reload(doc)), /Привет — Ελληνικά ✓ €/);
  });
});
