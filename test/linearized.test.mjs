// Linearized save (SDFDoc.SaveOptions.e_linearized, ISO 32000-1 Annex F).
// The hint tables are checked against `qpdf --check-linearization` during development; these
// tests check the file structure and that every reader path still works.
import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { PDFNet } from '../dist/index.js';

const fixture = (name) => new URL(`./fixtures/${name}`, import.meta.url).pathname;
const LIN = PDFNet.SDFDoc.SaveOptions.e_linearized;

async function pageTexts(doc) {
  const out = [];
  const txt = await PDFNet.TextExtractor.create();
  for (const it = await doc.getPageIterator(); await it.hasNext(); it.next()) {
    await txt.begin(await it.current());
    out.push(await txt.getAsText());
  }
  return out;
}

/** Checks the Annex F layout and returns the linearization parameters. */
function checkLayout(bytes) {
  const s = Buffer.from(bytes).toString('latin1');
  const m = s.match(
    /^%PDF-\d\.\d\n%[\x80-\xff]{4}\n(\d+) 0 obj\n<<\/Linearized 1\/L (\d+) *\/H \[(\d+) +(\d+) *\]\/O (\d+) *\/E (\d+) *\/N (\d+) *\/T (\d+) *>>/,
  );
  assert.ok(m, 'the linearization dictionary is the first object');
  const [lin, L, hOffset, hLength, O, E, N, T] = m.slice(1).map(Number);
  assert.equal(L, bytes.length, '/L is the file length');

  // startxref points to the first-page cross-reference section, right after the dictionary.
  const startxref = Number(s.match(/startxref\n(\d+)\n%%EOF\n$/)[1]);
  assert.equal(s.slice(startxref, startxref + 5 + String(lin).length), `xref\n${lin}`);
  const prev = Number(s.slice(startxref).match(/\/Prev (\d+)/)[1]);
  assert.equal(s.slice(prev, prev + 7), 'xref\n0 ');
  assert.equal(s[T], '\n');
  assert.equal(s.slice(T + 1, T + 19), '0000000000 65535 f', '/T precedes the first main xref entry');

  assert.match(s.slice(hOffset, hOffset + hLength), /^\d+ 0 obj\n<<.*\/S \d+.*stream\n[^]*endstream\nendobj\n$/);
  assert.match(s.slice(0, E), new RegExp(`\\n${O} 0 obj\\n<<`), 'the first page object is before /E');
  return { N, O, E };
}

describe('Linearized save (e_linearized)', () => {
  it('writes a valid linearized file that reads back unchanged', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const before = await pageTexts(doc);
    const bytes = await doc.saveMemoryBuffer(LIN);
    assert.equal(checkLayout(bytes).N, 3);
    const re = await PDFNet.PDFDoc.createFromBuffer(bytes);
    assert.deepEqual(await pageTexts(re), before);
    assert.deepEqual((await (await re.getPage(1)).getMediaBox()).toArray(), [0, 0, 612, 792]);
  });

  it('linearizes a page copied into a new document (page splitting)', async () => {
    const src = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const page = await PDFNet.PDFDoc.create();
    await page.insertPages(0, src, 2, 2, PDFNet.PDFDoc.InsertFlag.e_none);
    const bytes = await page.saveMemoryBuffer(LIN);
    assert.equal(checkLayout(bytes).N, 1);
    const re = await PDFNet.PDFDoc.createFromBuffer(bytes);
    assert.match((await pageTexts(re))[0], /Second page/);
  });

  it('linearizes merged documents with objects shared between pages', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('other.pdf'));
    const text = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    await doc.insertPages(1, text, 1, 3, PDFNet.PDFDoc.InsertFlag.e_none);
    await doc.insertPages(6, text, 1, 3, PDFNet.PDFDoc.InsertFlag.e_none);
    const before = await pageTexts(doc);
    const bytes = await doc.saveMemoryBuffer(LIN);
    assert.equal(checkLayout(bytes).N, 8);
    assert.deepEqual(await pageTexts(await PDFNet.PDFDoc.createFromBuffer(bytes)), before);
  });

  it('linearizes converted images and forms', async () => {
    const img = await PDFNet.PDFDoc.create();
    await PDFNet.Convert.toPdf(img, fixture('red-alpha.png'));
    checkLayout(await img.saveMemoryBuffer(LIN));

    const form = await PDFNet.PDFDoc.createFromFilePath(fixture('form.pdf'));
    const bytes = await form.saveMemoryBuffer(LIN | PDFNet.SDFDoc.SaveOptions.e_remove_unused);
    checkLayout(bytes);
    const re = await PDFNet.PDFDoc.createFromBuffer(bytes);
    assert.equal(await (await re.getField('name')).getValueAsString(), 'Jean Dupont');
  });

  it('encrypts linearized output (new AES-256 handler and kept RC4 security)', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const h = await PDFNet.SecurityHandler.createDefault();
    await h.changeUserPasswordUString('user');
    await doc.setSecurityHandler(h);
    const bytes = await doc.saveMemoryBuffer(LIN);
    checkLayout(bytes);
    assert.match(Buffer.from(bytes).toString('latin1'), /\/Length 256/, 'AES-256 key length is written');
    const re = await PDFNet.PDFDoc.createFromBuffer(bytes);
    assert.equal(await re.initStdSecurityHandler('user'), true);
    assert.match((await pageTexts(re))[0], /café crème/);

    const rc4 = await PDFNet.PDFDoc.createFromFilePath(fixture('encrypted-rc4-owner-only.pdf'));
    const out = await rc4.saveMemoryBuffer(LIN);
    checkLayout(out);
    const back = await PDFNet.PDFDoc.createFromBuffer(out);
    assert.equal(await back.isEncrypted(), true);
    assert.deepEqual(await pageTexts(back), ['Other A', 'Other B']);
  });

  it('signs a linearized save without breaking the layout', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const field = await doc.createDigitalSignatureField('Approval');
    const widget = await PDFNet.SignatureWidget.createWithDigitalSignatureField(doc, new PDFNet.Rect(350, 40, 560, 100), field);
    await (await doc.getPage(1)).annotPushBack(widget);
    await field.signOnNextSave(fixture('certs/signer-rsa.p12'), 'secret');
    const bytes = await doc.saveMemoryBuffer(LIN);
    checkLayout(bytes);
    const opts = await PDFNet.VerificationOptions.create();
    await opts.addTrustedCertificateUString(fixture('certs/ca.pem'));
    const re = await PDFNet.PDFDoc.createFromBuffer(bytes);
    assert.equal(await re.verifySignedDigitalSignatures(opts), PDFNet.PDFDoc.SignaturesVerificationStatus.e_verified);
  });

  it('accepts an incremental save on top of a linearized file', async () => {
    const doc = await PDFNet.PDFDoc.createFromBuffer(
      await (await PDFNet.PDFDoc.createFromFilePath(fixture('other.pdf'))).saveMemoryBuffer(LIN),
    );
    await (await doc.getDocInfo()).setTitle('Updated');
    const bytes = await doc.saveMemoryBuffer(PDFNet.SDFDoc.SaveOptions.e_incremental);
    const re = await PDFNet.PDFDoc.createFromBuffer(bytes);
    assert.equal(await (await re.getDocInfo()).getTitle(), 'Updated');
    assert.deepEqual(await pageTexts(re), ['Other A', 'Other B']);
  });
});
