// Features needed by existing PDFNet-based projects (validated against the "api" project):
// XFDF import, FDFDoc, Optimizer, JPEG 2000, and API details (destroy, runWithCleanup options…).
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { describe, it } from 'node:test';
import { crc32, deflateSync } from 'node:zlib';

import { PDFNet } from '../dist/index.js';

const fixture = (name) => new URL(`./fixtures/${name}`, import.meta.url).pathname;
const open = (name) => PDFNet.PDFDoc.createFromFilePath(fixture(name));
const reload = async (doc) => PDFNet.PDFDoc.createFromBuffer(await doc.saveMemoryBuffer());
const T = PDFNet.Annot.Type;

const XFDF = `<?xml version="1.0" encoding="UTF-8" ?><xfdf xmlns="http://ns.adobe.com/xfdf/" xml:space="preserve"><annots>
<square page="0" rect="60,600,220,700" color="#E44234" flags="print" name="sq-1" title="Admin Bewai" subject="Rectangle" date="D:20240515091551+02'00'" creationdate="D:20240515091551+02'00'" dashes="" width="3"/>
<ink page="0" rect="250,600,400,700" color="#0000FF" name="ink-1" width="2"><inklist><gesture>260,610;300,690;340,620</gesture></inklist></ink>
<freetext page="0" rect="60,450,350,520" name="ft-1" width="0"><contents>Validé &amp; signé</contents><defaultappearance>/Helv 14 Tf 0 0.5 0 rg</defaultappearance></freetext>
<highlight page="0" rect="70,690,230,725" color="#FFFF00" name="hl-1" coords="70,725,230,725,70,690,230,690"/>
<line page="0" rect="300,300,500,420" color="#000000" name="line-1" start="310,310" end="490,410" head="None" tail="ClosedArrow" width="2"/>
<stamp page="0" rect="400,450,500,550" name="stamp-1" icon="Approved"/>
</annots></xfdf>`;

function noisyPng(w, h) {
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w * 3; x++) raw[y * (w * 3 + 1) + 1 + x] = (x * 7 + y * 13 + ((x * y) % 97)) & 255;
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

describe('XFDF import (mergeXFDFString / FDFDoc)', () => {
  it('imports WebViewer annotations with their properties and appearances', async () => {
    const doc = await open('text.pdf');
    await doc.mergeXFDFString(XFDF);
    const re = await reload(doc);
    const page = await re.getPage(1);
    assert.equal(await page.getNumAnnots(), 6);
    const types = [];
    for (let i = 0; i < 6; i++) types.push(await (await page.getAnnot(i)).getType());
    assert.deepEqual(types, [T.e_Square, T.e_Ink, T.e_FreeText, T.e_Highlight, T.e_Line, T.e_Stamp]);
    const sq = await PDFNet.Markup.createFromAnnot(await page.getAnnot(0));
    assert.equal(await sq.getUniqueID(), 'sq-1');
    assert.equal(await sq.getTitle(), 'Admin Bewai');
    assert.equal((await sq.getBorderStyle()).width, 3);
    const c = await sq.getColorAsRGB();
    assert.deepEqual(c.c.slice(0, 3).map((v) => Math.round(v * 255)), [0xe4, 0x42, 0x34]);
    assert.equal(await (await page.getAnnot(2)).getContents(), 'Validé & signé');
    // appearances are burned in on flatten → the FreeText becomes page text
    await re.flattenAnnotations();
    const txt = await PDFNet.TextExtractor.create();
    await txt.begin(await re.getPage(1));
    assert.match(await txt.getAsText(), /Validé & signé/);
    assert.match(await txt.getAsText(), /APPROVED/);
  });

  it('replaces annotations with the same name and applies delete commands', async () => {
    const doc = await open('text.pdf');
    await doc.mergeXFDFString(XFDF);
    await doc.mergeXFDFString(XFDF.replace('color="#E44234"', 'color="#00FF00"'));
    const page = await doc.getPage(1);
    assert.equal(await page.getNumAnnots(), 6, 'replaced, not duplicated');
    const cmd = '<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><delete><id page="0">ink-1</id><id page="0">hl-1</id></delete></xfdf>';
    await doc.mergeXFDF(await PDFNet.FDFDoc.createFromXFDF(cmd));
    assert.equal(await page.getNumAnnots(), 4);
  });

  it('imports form field values', async () => {
    const doc = await open('form.pdf');
    await doc.fdfMerge(await PDFNet.FDFDoc.createFromXFDF(
      '<xfdf xmlns="http://ns.adobe.com/xfdf/"><fields><field name="name"><value>Marie</value></field><field name="agree"><value>Oui</value></field></fields></xfdf>',
    ));
    assert.equal(await (await doc.getField('name')).getValueAsString(), 'Marie');
    assert.equal(await (await doc.getField('agree')).getValueAsBool(), true);
  });

  it('rejects invalid XFDF and bad page numbers', async () => {
    await assert.rejects(PDFNet.FDFDoc.createFromXFDF('<html/>'), /expected <xfdf>/);
    await assert.rejects(PDFNet.FDFDoc.createFromXFDF('not xml <'), /invalid XFDF/);
    const doc = await open('other.pdf');
    await assert.rejects(doc.mergeXFDFString(XFDF.replace(/page="0"/g, 'page="7"')), /page 7/);
  });
});

describe('Optimizer', () => {
  it('downsamples and JPEG-recompresses images (PDFNet settings API, setters not awaited)', async () => {
    const doc = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.fromImage(doc, noisyPng(1200, 1200), 300);
    const before = (await doc.saveMemoryBuffer()).length;

    const imageSettings = new PDFNet.Optimizer.ImageSettings();
    imageSettings.setCompressionMode(PDFNet.Optimizer.ImageSettings.CompressionMode.e_jpeg);
    imageSettings.setQuality(4);
    imageSettings.setImageDPI(144, 96);
    imageSettings.forceRecompression(true);
    const opt = new PDFNet.Optimizer.OptimizerSettings();
    opt.setColorImageSettings(imageSettings);
    opt.setGrayscaleImageSettings(imageSettings);
    const r = await PDFNet.Optimizer.optimize(doc, opt);
    assert.equal(r.optimized, 1);
    const after = await doc.saveMemoryBuffer(PDFNet.SDFDoc.SaveOptions.e_remove_unused);
    assert.ok(after.length < before / 5, `${before} → ${after.length}`);
    // page size unchanged, image still drawn
    const re = await PDFNet.PDFDoc.createFromBuffer(after);
    assert.equal(await (await re.getPage(1)).getPageWidth(), 288);
    const bmp = await (await PDFNet.PDFDraw.create(36)).getBitmap(await re.getPage(1), PDFNet.PDFDraw.PixelFormat.e_rgb);
    assert.ok(bmp.buf.some((v) => v < 200), 'image still rendered');
  });

  it('leaves small images alone with default settings', async () => {
    const doc = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.fromImage(doc, readFileSync(fixture('red-alpha.png')));
    const r = await PDFNet.Optimizer.optimize(doc);
    assert.equal(r.optimized, 0);
  });
});

describe('JPEG 2000 and API details', () => {
  it('converts .jp2 images (embedded as JPXDecode) and renders them', async () => {
    const doc = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.toPdf(doc, fixture('halves.jp2'));
    const page = await doc.getPage(1);
    assert.equal(await page.getPageWidth(), 48); // 64 px at 96 dpi
    const bmp = await (await PDFNet.PDFDraw.create(72 * 4)).getBitmap(page, PDFNet.PDFDraw.PixelFormat.e_rgb);
    const at = (x, y) => [...bmp.buf.subarray(y * bmp.stride + x * 3, y * bmp.stride + x * 3 + 3)];
    const left = at(10, 10);
    const right = at(bmp.width - 10, 10);
    assert.ok(left[0] > 180 && left[2] < 90, `left is red: ${left}`);
    assert.ok(right[2] > 160 && right[0] < 60, `right is blue: ${right}`);
  });

  it('accepts PDFNet call shapes used by existing code', async () => {
    const v = await PDFNet.runWithCleanup(async () => 'ok', null, { persistentCachePath: '/tmp' });
    assert.equal(v, 'ok');
    const draw = await PDFNet.PDFDraw.create();
    draw.setDPI(72); // not awaited
    const doc = await open('text.pdf');
    const first = await (await doc.getPageIterator()).current();
    const jpg = await draw.exportBuffer(first, 'JPEG');
    assert.deepEqual([...jpg.subarray(0, 2)], [0xff, 0xd8]);
    (await doc.getPage(1)).setRotation(PDFNet.Page.Rotate.e_180); // not awaited
    assert.equal(await (await doc.getPage(1)).getRotation(), PDFNet.Page.Rotate.e_180);
    await draw.destroy();
    await doc.destroy();
    const t = await PDFNet.Convert.createOfficeTemplateWithPath(fixture('invoice-template.docx'));
    await t.destroy();
  });
});
