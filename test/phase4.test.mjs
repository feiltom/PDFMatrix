import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { describe, it } from 'node:test';

import { PDFNet } from '../dist/index.js';

const fixture = (name) => new URL(`./fixtures/${name}`, import.meta.url).pathname;
const gotenberg = process.env.GOTENBERG_URL;
const needsGotenberg = gotenberg ? {} : { skip: 'GOTENBERG_URL not set (run `make test-all`)' };

async function allText(doc) {
  const txt = await PDFNet.TextExtractor.create();
  const out = [];
  for (const it = await doc.getPageIterator(); await it.hasNext(); it.next()) {
    await txt.begin(await it.current());
    out.push(await txt.getAsText());
  }
  return out.join('\n');
}

describe('Convert: local formats', () => {
  it('appends the pages of a PDF', async () => {
    const doc = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.toPdf(doc, fixture('text.pdf'));
    await PDFNet.Convert.toPdf(doc, fixture('other.pdf'));
    assert.equal(await doc.getPageCount(), 5);
    assert.match(await allText(doc), /Hello World[\s\S]*Other B/);
  });

  it('turns images into pages at the requested resolution', async () => {
    const doc = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.toPdf(doc, fixture('red-alpha.png')); // 96 dpi
    await PDFNet.Convert.fromImage(doc, readFileSync(fixture('red-alpha.png')), 72);
    assert.equal(await doc.getPageCount(), 2);
    assert.equal(await (await doc.getPage(1)).getPageWidth(), 3);
    assert.equal(await (await doc.getPage(2)).getPageWidth(), 4);
    const draw = await PDFNet.PDFDraw.create(72 * 10);
    const bmp = await draw.getBitmap(await doc.getPage(2), PDFNet.PDFDraw.PixelFormat.e_rgb);
    const [r, g] = bmp.buf;
    assert.ok(r > 240 && g > 100 && g < 160, `half-transparent red: ${[...bmp.buf.subarray(0, 3)]}`);
  });

  it('reports an unreachable conversion service clearly', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const opts = new PDFNet.OfficeToPDFOptions().setGotenbergUrl('http://127.0.0.1:9').setTimeout(2000);
    await assert.rejects(PDFNet.Convert.officeToPdf(doc, fixture('letter.docx'), opts), /unreachable at http:\/\/127\.0\.0\.1:9/);
  });
});

describe('Convert: Office and HTML through Gotenberg', needsGotenberg, () => {
  it('converts a Word document with accents, bold text and a table', async () => {
    const doc = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.officeToPdf(doc, fixture('letter.docx'), null);
    assert.equal(await doc.getPageCount(), 1);
    const page = await doc.getPage(1);
    assert.ok(Math.abs((await page.getPageWidth()) - 595.3) < 1, 'A4 from the docx section');
    const t = await allText(doc);
    for (const s of ['Lettre de test', 'café, crème brûlée, garçon, œuvre', 'Croissant', '1,20 €', 'Fin du document']) {
      assert.ok(t.includes(s), `missing "${s}" in:\n${t}`);
    }
  });

  it('converts RTF with toPdf and honours landscape', async () => {
    const doc = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.toPdf(doc, fixture('note.rtf'), new PDFNet.OfficeToPDFOptions().setLandscape(true));
    assert.match(await allText(doc), /été, à bientôt/);
    const p = await doc.getPage(1);
    assert.ok((await p.getPageWidth()) > (await p.getPageHeight()), 'landscape page');
  });

  it('converts in-memory Office bytes into a new document', async () => {
    const doc = await PDFNet.Convert.officeToPdfDoc(readFileSync(fixture('letter.docx')));
    assert.match(await allText(doc), /Lettre de test/);
    // The result is a regular document: stamp it and save it.
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_font_size, 20, 20);
    await s.stampText(doc, 'COPIE', await PDFNet.PageSet.createSinglePage(1));
    const re = await PDFNet.PDFDoc.createFromBuffer(await doc.saveMemoryBuffer());
    assert.match(await allText(re), /COPIE/);
  });

  it('converts HTML with HTML2PDF', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const h = await PDFNet.HTML2PDF.create();
    await h.setPaperSize(8.27, 11.7);
    await h.insertFromHtmlString('<html><body><h1>Facture n°42</h1><p>Total à payer : 99,90 €</p></body></html>');
    await h.insertFromHtmlString('<p style="page-break-before:always">Deuxième document</p>');
    assert.equal(await h.convert(doc), true);
    assert.equal(await doc.getPageCount(), 2);
    const t = await allText(doc);
    assert.match(t, /Facture n°42/);
    assert.match(t, /Total à payer : 99,90 €/);
    assert.match(t, /Deuxième document/);
  });

  it('surfaces conversion errors from the service', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const opts = new PDFNet.OfficeToPDFOptions().setFileName('file.xyz');
    await assert.rejects(PDFNet.Convert.officeToPdf(doc, Buffer.from('whatever'), opts), /conversion failed \(HTTP 400\).*\.docx/);
  });
});
