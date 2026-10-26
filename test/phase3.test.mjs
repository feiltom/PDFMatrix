import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { describe, it } from 'node:test';

import { PDFNet } from '../dist/index.js';

const fixture = (name) => new URL(`./fixtures/${name}`, import.meta.url).pathname;
const open = (name) => PDFNet.PDFDoc.createFromFilePath(fixture(name));
const fromBytes = (b) => PDFNet.PDFDoc.createFromBuffer(b);
const P = PDFNet.SecurityHandler.Permission;
const S = PDFNet.PDFDoc.SignaturesVerificationStatus;
const INC = PDFNet.SDFDoc.SaveOptions.e_incremental;

async function text(doc, pageNum = 1) {
  const txt = await PDFNet.TextExtractor.create();
  await txt.begin(await doc.getPage(pageNum));
  return txt.getAsText();
}

async function trustOptions(...certs) {
  const opts = await PDFNet.VerificationOptions.create();
  for (const c of certs) await opts.addTrustedCertificateUString(fixture(`certs/${c}`));
  return opts;
}

async function addSignatureField(doc, name, rect = new PDFNet.Rect(350, 40, 560, 100)) {
  const field = await doc.createDigitalSignatureField(name);
  const widget = await PDFNet.SignatureWidget.createWithDigitalSignatureField(doc, rect, field);
  await (await doc.getPage(1)).annotPushBack(widget);
  return field;
}

describe('Encryption on save', () => {
  it('encrypts with AES-256, passwords and permissions', async () => {
    const doc = await open('text.pdf');
    const h = await PDFNet.SecurityHandler.createDefault();
    await h.changeUserPasswordUString('user');
    await h.changeMasterPasswordUString('owner');
    await h.setPermission(P.e_print, false);
    await h.setPermission(P.e_extract_content, false);
    await doc.setSecurityHandler(h);
    const re = await fromBytes(await doc.saveMemoryBuffer());
    assert.equal(await re.isEncrypted(), true);
    assert.equal(await re.initSecurityHandler(), false, 'needs a password');
    assert.equal(await re.initStdSecurityHandler('nope'), false);
    assert.equal(await re.initStdSecurityHandler('user'), true);
    assert.match(await text(re), /café crème/);
    const sh = await re.getSecurityHandler();
    assert.equal(await sh.getEncryptionAlgorithmID(), PDFNet.SecurityHandler.AlgorithmType.e_AES_256);
    assert.equal(await sh.getKeyLength(), 256);
    assert.equal(await sh.getPermission(P.e_print), false);
    assert.equal(await sh.getPermission(P.e_extract_content), false);
    assert.equal(await sh.getPermission(P.e_fill_forms), true);
  });

  for (const [alg, name] of [[1, 'RC4-40'], [2, 'RC4-128'], [3, 'AES-128'], [4, 'AES-256']]) {
    it(`round-trips ${name}`, async () => {
      const doc = await open('other.pdf');
      const h = await PDFNet.SecurityHandler.create(alg);
      await h.changeUserPasswordUString('pw');
      await doc.setSecurityHandler(h);
      const re = await fromBytes(await doc.saveMemoryBuffer());
      assert.equal(await re.initStdSecurityHandler('pw'), true);
      assert.equal(await (await re.getSecurityHandler()).getEncryptionAlgorithmID(), alg);
      assert.equal(await text(re, 2), 'Other B');
    });
  }

  it('keeps the original security when saving an encrypted document, unless removed', async () => {
    const doc = await open('encrypted-aes256.pdf');
    await doc.initStdSecurityHandler('user-pw');
    await (await doc.getDocInfo()).setTitle('Still secret');
    const kept = await fromBytes(await doc.saveMemoryBuffer());
    assert.equal(await kept.initSecurityHandler(), false, 'still locked');
    assert.equal(await kept.initStdSecurityHandler('user-pw'), true);
    assert.equal(await (await kept.getDocInfo()).getTitle(), 'Still secret');

    await doc.removeSecurity();
    const open2 = await fromBytes(await doc.saveMemoryBuffer());
    assert.equal(await open2.initSecurityHandler(), true);
    assert.equal(await open2.isEncrypted(), false);
  });

  it('saves an encrypted document incrementally', async () => {
    const doc = await open('encrypted-aes256.pdf');
    await doc.initStdSecurityHandler('user-pw');
    await (await doc.getDocInfo()).setAuthor('Incremental');
    const bytes = await doc.saveMemoryBuffer(INC);
    const orig = readFileSync(fixture('encrypted-aes256.pdf'));
    assert.ok(Buffer.from(bytes.subarray(0, orig.length)).equals(orig), 'original bytes untouched');
    const re = await fromBytes(bytes);
    assert.equal(await re.initStdSecurityHandler('user-pw'), true);
    assert.equal(await (await re.getDocInfo()).getAuthor(), 'Incremental');
  });
});

describe('Incremental save', () => {
  it('appends changes after the original bytes', async () => {
    const orig = readFileSync(fixture('text.pdf'));
    const doc = await fromBytes(orig);
    await (await doc.getDocInfo()).setTitle('Updated');
    await (await doc.getPage(1)).setRotation(PDFNet.Page.Rotate.e_90);
    const bytes = Buffer.from(await doc.saveMemoryBuffer(INC));
    assert.ok(bytes.length > orig.length);
    assert.ok(bytes.subarray(0, orig.length).equals(orig));
    assert.match(bytes.subarray(orig.length).toString('latin1'), /\/Prev \d+/);
    const re = await fromBytes(bytes);
    assert.equal(await (await re.getDocInfo()).getTitle(), 'Updated');
    assert.equal(await (await re.getPage(1)).getRotation(), PDFNet.Page.Rotate.e_90);
    assert.match(await text(re, 2), /Second page/);
  });
});

describe('Digital signatures', () => {
  it('signs with RSA (PKCS#12 + CA chain) and verifies trust', async () => {
    const doc = await open('text.pdf');
    const field = await addSignatureField(doc, 'Approval');
    await field.setReason('Approbation');
    await field.setLocation('Paris');
    await field.setContactInfo('jean@example.org');
    await field.signOnNextSave(fixture('certs/signer-rsa.p12'), 'secret');
    const signed = await doc.saveMemoryBuffer(INC);

    const re = await fromBytes(signed);
    assert.equal(await re.verifySignedDigitalSignatures(await PDFNet.VerificationOptions.create()), S.e_untrusted);
    assert.equal(await re.verifySignedDigitalSignatures(await trustOptions('ca.pem')), S.e_verified);

    const it = await re.getDigitalSignatureFieldIterator();
    const sig = await it.current();
    assert.equal(await sig.hasCryptographicSignature(), true);
    assert.equal(await sig.getSignatureName(), 'Jean Dupont');
    assert.equal(await sig.getReason(), 'Approbation');
    assert.equal(await sig.getLocation(), 'Paris');
    assert.equal(await sig.getContactInfo(), 'jean@example.org');
    assert.equal(await sig.getSubFilter(), PDFNet.DigitalSignatureField.SubFilterType.e_adbe_pkcs7_detached);
    assert.equal((await sig.getSigningTime()).year >= 2026, true);

    const VR = PDFNet.VerificationResult;
    const v = await sig.verify(await trustOptions('ca.der'));
    assert.equal(await v.getVerificationStatus(), true);
    assert.equal(await v.getDocumentStatus(), VR.DocumentStatus.e_no_error);
    assert.equal(await v.getDigestStatus(), VR.DigestStatus.e_digest_verified);
    assert.equal(await v.getTrustStatus(), VR.TrustStatus.e_trust_verified);
    assert.equal(await v.getPermissionsStatus(), VR.ModificationPermissionsStatus.e_unmodified);
    assert.equal(await v.getDigestAlgorithm(), 'SHA-256');
    assert.equal((await v.getCertificates()).length, 2, 'signer + CA embedded');

    // The visible appearance is drawn (annotation content is not page text: flatten a copy).
    const copy = await fromBytes(signed);
    await copy.flattenAnnotations();
    assert.match(await text(copy), /Digitally signed by Jean Dupont/);
    assert.match(await text(copy), /Reason: Approbation/);
  });

  it('signs with ECDSA P-256 and trusts a self-signed certificate explicitly', async () => {
    const doc = await open('form.pdf');
    const field = await addSignatureField(doc, 'Ec', new PDFNet.Rect(0, 0, 0, 0)); // invisible
    await field.signOnNextSaveFromBuffer(readFileSync(fixture('certs/signer-ec.p12')), 'secret');
    const re = await fromBytes(await doc.saveMemoryBuffer(INC));
    assert.equal(await re.verifySignedDigitalSignatures(await trustOptions('ec.pem')), S.e_verified);
    assert.equal(await re.verifySignedDigitalSignatures(await trustOptions('ca.pem')), S.e_untrusted);
  });

  it('detects tampering inside the signed byte range', async () => {
    const doc = await open('text.pdf');
    const field = await addSignatureField(doc, 'Sig');
    await field.signOnNextSave(fixture('certs/signer-rsa.p12'), 'secret');
    const signed = Buffer.from(await doc.saveMemoryBuffer(INC));
    const i = signed.indexOf('(Hello World)');
    assert.ok(i > 0);
    signed.write('J', i + 1, 'latin1'); // "Jello World", same length
    const re = await fromBytes(signed);
    assert.equal(await re.verifySignedDigitalSignatures(await trustOptions('ca.pem')), S.e_failure);
    const v = await (await (await re.getDigitalSignatureFieldIterator()).current()).verify(await trustOptions('ca.pem'));
    assert.equal(await v.getDigestStatus(), PDFNet.VerificationResult.DigestStatus.e_digest_invalid);
    assert.match(await v.getMessage(), /digest mismatch/);
  });

  it('keeps signatures valid across incremental updates and countersignatures', async () => {
    const doc = await open('text.pdf');
    const a = await addSignatureField(doc, 'First', new PDFNet.Rect(50, 40, 250, 100));
    await a.signOnNextSave(fixture('certs/signer-rsa.p12'), 'secret');
    const once = await fromBytes(await doc.saveMemoryBuffer(INC));

    await (await once.getDocInfo()).setTitle('After signing');
    const b = await addSignatureField(once, 'Second');
    await b.signOnNextSave(fixture('certs/signer-ec.p12'), 'secret');
    const twice = await fromBytes(await once.saveMemoryBuffer(INC));

    assert.equal(await twice.verifySignedDigitalSignatures(await trustOptions('ca.pem', 'ec.pem')), S.e_verified);
    const results = [];
    for (const it = await twice.getDigitalSignatureFieldIterator(); await it.hasNext(); it.next()) {
      const v = await (await it.current()).verify(await trustOptions('ca.pem', 'ec.pem'));
      results.push([await v.getVerificationStatus(), await v.getPermissionsStatusAsString()]);
    }
    assert.deepEqual(results, [
      [true, 'e_has_allowed_changes'],
      [true, 'e_unmodified'],
    ]);
    assert.equal(await (await twice.getDocInfo()).getTitle(), 'After signing');
  });

  it('a full (non-incremental) re-save invalidates earlier signatures', async () => {
    const doc = await open('text.pdf');
    const f = await addSignatureField(doc, 'Sig');
    await f.signOnNextSave(fixture('certs/signer-rsa.p12'), 'secret');
    const signed = await fromBytes(await doc.saveMemoryBuffer(INC));
    await (await signed.getDocInfo()).setTitle('rewritten');
    const rewritten = await fromBytes(await signed.saveMemoryBuffer(0));
    assert.notEqual(await rewritten.verifySignedDigitalSignatures(await trustOptions('ca.pem')), S.e_verified);
  });

  it('reports errors clearly', async () => {
    const doc = await open('text.pdf');
    const f = await addSignatureField(doc, 'Sig');
    await assert.rejects(f.signOnNextSave(fixture('certs/signer-rsa.p12'), 'wrong'), /PKCS#12/);
    assert.equal(await doc.verifySignedDigitalSignatures(), S.e_unsigned);
    const enc = await open('encrypted-aes256.pdf');
    await enc.initStdSecurityHandler('user-pw');
    const g = await addSignatureField(enc, 'Sig');
    await g.signOnNextSave(fixture('certs/signer-rsa.p12'), 'secret');
    await assert.rejects(enc.saveMemoryBuffer(INC), /encrypted documents/);
  });
});

describe('Redaction', () => {
  async function findBoxes(doc, pattern, page = 1) {
    const ts = await PDFNet.TextSearch.create();
    await ts.begin(doc, pattern, PDFNet.TextSearch.Mode.e_highlight, page, page);
    const boxes = [];
    for (let r = await ts.run(); r.code !== PDFNet.TextSearch.ResultCode.e_done; r = await ts.run()) {
      if (r.code !== PDFNet.TextSearch.ResultCode.e_found) continue;
      for (const q of await r.highlights.getAllQuads()) boxes.push(new PDFNet.Rect(q.p1x, q.p1y, q.p3x, q.p3y));
    }
    return boxes;
  }

  async function wordBoxes(doc, page = 1) {
    const txt = await PDFNet.TextExtractor.create();
    await txt.begin(await doc.getPage(page));
    const out = {};
    for (let line = await txt.getFirstLine(); await line.isValid(); line = await line.getNextLine()) {
      for (let w = await line.getFirstWord(); await w.isValid(); w = await w.getNextWord()) out[await w.getString()] = await w.getBBox();
    }
    return out;
  }

  it('removes the text under the area and keeps the rest in place', async () => {
    const doc = await open('text.pdf');
    const before = await wordBoxes(doc);
    const [box] = await findBoxes(doc, 'café');
    const stats = await PDFNet.Redactor.redact(doc, [await PDFNet.Redactor.redactionCreate(1, box, false, '')]);
    assert.equal(stats.glyphs, 4);
    const re = await fromBytes(await doc.saveMemoryBuffer());
    assert.equal(await text(re), 'Hello World\nBonjour à tous, crème.');
    const after = await wordBoxes(re);
    assert.ok(Math.abs(after['crème.'].x1 - before['crème.'].x1) < 0.01, 'following word did not move');
    // the text really is gone from the content stream, not only hidden
    const ts = await PDFNet.TextSearch.create();
    await ts.begin(re, 'caf', 0);
    assert.equal((await ts.run()).code, PDFNet.TextSearch.ResultCode.e_done);
  });

  it('draws overlay text and supports negative (keep-only) redaction', async () => {
    const doc = await open('text.pdf');
    const R = PDFNet.Redactor;
    await R.redact(
      doc,
      [await R.redactionCreate(2, (await findBoxes(doc, 'target here', 2))[0], false, 'REDACTED')],
      { positive_overlay_color: await PDFNet.ColorPt.init(0, 0, 0), text_color: await PDFNet.ColorPt.init(1, 1, 1) },
    );
    const t2 = await text(doc, 2);
    assert.match(t2, /Second page with the word/);
    assert.doesNotMatch(t2, /target here/);
    assert.match(t2, /REDACTED/, 'overlay text drawn');

    await R.redact(doc, [await R.redactionCreate(1, new PDFNet.Rect(60, 690, 250, 730), true)], { redaction_overlay: false });
    assert.equal(await text(doc, 1), 'Hello World');
  });

  it('blanks image pixels under the area (not just covering them)', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const page = await doc.pageCreate(new PDFNet.Rect(0, 0, 200, 200));
    await doc.pagePushBack(page);
    const img = await PDFNet.Image.createFromFile(doc, fixture('red-alpha.png'));
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_absolute_size, 200, 200);
    await s.stampImage(doc, img, await PDFNet.PageSet.createSinglePage(1));
    const stats = await PDFNet.Redactor.redact(doc, [await PDFNet.Redactor.redactionCreate(1, new PDFNet.Rect(0, 0, 100, 200))], { redaction_overlay: false });
    assert.equal(stats.images, 1);
    const draw = await PDFNet.PDFDraw.create(72);
    const bmp = await draw.getBitmap(await doc.getPage(1), PDFNet.PDFDraw.PixelFormat.e_rgb);
    const at = (x, y) => [...bmp.buf.subarray((200 - y) * bmp.stride + x * 3, (200 - y) * bmp.stride + x * 3 + 3)];
    const [lr, lg] = at(50, 100);
    const [rr, rg] = at(150, 100);
    assert.ok(lr < 160 && lg < 160, `left half blanked (dark): ${at(50, 100)}`);
    assert.ok(rr > 240 && rg > 100, `right half untouched (pink): ${at(150, 100)}`);
  });

  it('redacts inside Form XObjects (stamps) and removes covered annotations', async () => {
    const doc = await open('text.pdf');
    const s = await PDFNet.Stamper.create(PDFNet.Stamper.SizeType.e_font_size, 30, 30);
    await s.setAlignment(PDFNet.Stamper.HorizontalAlignment.e_horizontal_left, PDFNet.Stamper.VerticalAlignment.e_vertical_bottom);
    await s.setPosition(72, 300);
    await s.stampText(doc, 'SECRET CODE 42', await PDFNet.PageSet.createSinglePage(1));
    const page = await doc.getPage(1);
    await page.annotPushBack(await PDFNet.TextAnnot.create(doc, new PDFNet.Point(100, 330), 'note'));
    const [box] = await findBoxes(doc, 'SECRET CODE');
    const stats = await PDFNet.Redactor.redact(doc, [await PDFNet.Redactor.redactionCreate(1, box)]);
    assert.ok(stats.glyphs >= 10);
    assert.equal(stats.annotations, 1);
    const t = await text(await fromBytes(await doc.saveMemoryBuffer()));
    assert.doesNotMatch(t, /SECRET|CODE/);
    assert.match(t, /42/);
  });

  it('applies redaction annotations', async () => {
    const doc = await open('text.pdf');
    const [box] = await findBoxes(doc, 'Hello');
    const mark = await PDFNet.RedactionAnnot.create(doc, box);
    await mark.setOverlayText('X');
    await (await doc.getPage(1)).annotPushBack(mark);
    assert.match(await text(doc), /Hello World/, 'marking alone does not remove anything');
    await doc.applyRedactions();
    assert.equal(await (await doc.getPage(1)).getNumAnnots(), 0);
    const t = await text(doc);
    assert.doesNotMatch(t, /Hello/);
    assert.match(t, /World/);
    assert.match(t, /\bX\b/, 'overlay text');
  });
});
