import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, it } from 'node:test';
import { inflateRawSync } from 'node:zlib';

import { PDFNet } from '../dist/index.js';

const fixture = (name) => new URL(`./fixtures/${name}`, import.meta.url).pathname;
const tmp = mkdtempSync(join(tmpdir(), 'pdfnet-docgen-'));
const needsGotenberg = process.env.GOTENBERG_URL ? {} : { skip: 'GOTENBERG_URL not set (run `make test-all`)' };

/** Reads one entry of a ZIP (.docx) — enough for tests. */
function unzipEntry(buf, name) {
  const b = Buffer.from(buf);
  let p = 0;
  while ((p = b.indexOf(Buffer.from([0x50, 0x4b, 3, 4]), p)) >= 0) {
    const method = b.readUInt16LE(p + 8);
    const size = b.readUInt32LE(p + 18);
    const nl = b.readUInt16LE(p + 26);
    const xl = b.readUInt16LE(p + 28);
    const entry = b.subarray(p + 30, p + 30 + nl).toString();
    const data = b.subarray(p + 30 + nl + xl, p + 30 + nl + xl + size);
    if (entry === name) return method === 8 ? inflateRawSync(data) : data;
    p += 30 + nl + xl + size;
  }
  return null;
}

/** Visible text of a WordprocessingML part: paragraphs on their own lines. */
const wordText = (xml) =>
  xml
    .toString()
    .replace(/<w:p[ >]/g, '\n<w:p ')
    .replace(/<w:drawing>.*?<\/w:drawing>/g, '[IMAGE]')
    .replace(/<[^>]+>/g, '')
    .replace(/&amp;/g, '&')
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);

const DATA = {
  numero: 'F-2026-042',
  client: { nom: 'Café Dupont & Fils', vip: 'oui', adresse: { ville: 'Lyon' } },
  logo: { image_url: fixture('red-alpha.png'), width: 40, height: 40 },
  payee: false,
  total: 150.5,
  lignes: [
    { produit: 'Croissant', qte: 3, prix: 1.2 },
    { produit: 'Pain au chocolat', qte: 2, prix: 1.4 },
  ],
  notes: ['Livraison mardi', 'Fragile'],
  sections: [
    { titre: 'A', items: [{ nom: 'a1' }, { nom: 'a2' }] },
    { titre: 'B', items: [{ nom: 'b1' }] },
  ],
};

describe('DocGen: templates', () => {
  it('lists the keys the template expects', async () => {
    const t = await PDFNet.Convert.createOfficeTemplateWithPath(fixture('invoice-template.docx'));
    assert.deepEqual(JSON.parse(await t.getTemplateKeysJson()), {
      client: { adresse: { ville: '' }, nom: '', vip: '' },
      lignes: [{ prix: '', produit: '', qte: '' }],
      logo: '',
      notes: [''],
      numero: '',
      payee: '',
      sections: [{ items: [{ nom: '' }], titre: '' }],
      total: '',
    });
  });

  it('fills values, loops (rows, paragraphs, nested), conditions and images', async () => {
    const t = await PDFNet.Convert.createOfficeTemplateWithPath(fixture('invoice-template.docx'));
    const docx = await t.fillTemplateJsonToBuffer(JSON.stringify(DATA));
    assert.equal(await t.getErrorString(), '');
    const lines = wordText(unzipEntry(docx, 'word/document.xml'));
    assert.deepEqual(lines, [
      'Facture n°F-2026-042', // tag split over 3 runs + proofErr
      'Client : Café Dupont & Fils, Lyon',
      '[IMAGE]',
      'Statut : à régler. Remise appliquée.', // inline if/else and comparison
      'Produit', 'Qté', 'Prix',
      'Croissant', '3', '1.2 €', // table row loop
      'Pain au chocolat', '2', '1.4 €',
      'Total', '150.5 €',
      'Notes :',
      '- Livraison mardi', // paragraph loop over strings
      '- Fragile',
      'Merci pour votre fidélité VIP.', // block if with typographic quotes
      'Section A :', '* a1 (n°1)', '* a2 (n°2)', // nested loops
      'Section B :', '* b1 (n°1)',
      'Fin.',
    ]);
    assert.deepEqual(wordText(unzipEntry(docx, 'word/header1.xml')), ['En-tête : facture F-2026-042 & client Café Dupont & Fils']);
    // the image is stored, referenced and typed
    assert.ok(unzipEntry(docx, 'word/media/docgen1.png'));
    assert.match(unzipEntry(docx, 'word/_rels/document.xml.rels').toString(), /Id="rIdDocGen1"[^>]+Target="media\/docgen1\.png"/);
    assert.match(unzipEntry(docx, '[Content_Types].xml').toString(), /Extension="png"/);
    assert.match(unzipEntry(docx, 'word/document.xml').toString(), /<wp:extent cx="508000" cy="508000"\/>/, '40pt = 508000 EMU');
  });

  it('takes the other branches and reports missing values', async () => {
    const t = await PDFNet.Convert.createOfficeTemplateWithBuffer(readFileSync(fixture('invoice-template.docx')));
    const data = { ...DATA, payee: true, total: 50, client: { nom: 'X', vip: 'non' }, notes: [], logo: undefined };
    delete data.logo;
    const lines = wordText(unzipEntry(await t.fillTemplateJsonToBuffer(data), 'word/document.xml'));
    assert.ok(lines.includes('Statut : payée.'), lines.join('|'));
    assert.ok(lines.includes('Merci pour votre commande.'));
    assert.ok(!lines.some((l) => l.startsWith('- ')), 'empty loop produces nothing');
    assert.ok(lines.includes('Client : X,'), 'missing nested value is empty');
    const warnings = await t.getErrorString();
    assert.match(warnings, /client\.adresse\.ville/);
    assert.match(warnings, /logo/);
  });

  it('writes the filled .docx with fillTemplateJsonToOffice', async () => {
    const t = await PDFNet.Convert.createOfficeTemplateWithPath(fixture('invoice-template.docx'));
    const out = join(tmp, 'facture.docx');
    await t.fillTemplateJsonToOffice(JSON.stringify(DATA), out);
    assert.match(wordText(unzipEntry(readFileSync(out), 'word/document.xml')).join('\n'), /Pain au chocolat/);
  });

  it('rejects invalid templates and data', async () => {
    await assert.rejects(PDFNet.Convert.createOfficeTemplateWithPath(fixture('broken-template.docx')), /never closed/);
    await assert.rejects(PDFNet.Convert.createOfficeTemplateWithPath(fixture('text.pdf')), /not a \.docx/);
    const t = await PDFNet.Convert.createOfficeTemplateWithPath(fixture('invoice-template.docx'));
    await assert.rejects(t.fillTemplateJsonToBuffer('{ not json'), /invalid JSON/);
    await assert.rejects(t.fillTemplateJsonToBuffer({ ...DATA, logo: { image_url: '/nope.png' } }), /cannot read \/nope\.png/);
  });
});

describe('DocGen: PDF output through Gotenberg', needsGotenberg, () => {
  it('fills the template and converts it to PDF (LibreOffice opens the generated .docx)', async () => {
    const t = await PDFNet.Convert.createOfficeTemplateWithPath(fixture('invoice-template.docx'), new PDFNet.OfficeToPDFOptions());
    const pdf = await t.fillTemplateJson(JSON.stringify(DATA));
    const txt = await PDFNet.TextExtractor.create();
    await txt.begin(await pdf.getPage(1));
    const text = await txt.getAsText();
    for (const s of ['En-tête : facture F-2026-042', 'Facture n°F-2026-042', 'Café Dupont & Fils, Lyon', 'Pain au chocolat', '150.5 €', 'Merci pour votre fidélité VIP.', '* b1 (n°1)']) {
      assert.ok(text.includes(s), `missing "${s}" in:\n${text}`);
    }
    // The red logo is rendered.
    const draw = await PDFNet.PDFDraw.create(72);
    const bmp = await draw.getBitmap(await pdf.getPage(1), PDFNet.PDFDraw.PixelFormat.e_rgb);
    let pink = 0;
    for (let k = 0; k < bmp.buf.length; k += 3) if (bmp.buf[k] > 240 && bmp.buf[k + 1] > 100 && bmp.buf[k + 1] < 160) pink++;
    assert.ok(pink > 500, `logo pixels: ${pink}`);
  });
});
