import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, it } from 'node:test';

import { PDFNet } from '../dist/index.js';

const fixture = (name) => new URL(`./fixtures/${name}`, import.meta.url).pathname;
const tmp = mkdtempSync(join(tmpdir(), 'pdfnet-test-'));

async function pageTexts(doc) {
  const out = [];
  const txt = await PDFNet.TextExtractor.create();
  for (const it = await doc.getPageIterator(); await it.hasNext(); it.next()) {
    await txt.begin(await it.current());
    out.push(await txt.getAsText());
  }
  return out;
}

describe('PDFNet entry point', () => {
  it('runs a function with runWithCleanup and returns its value', async () => {
    const v = await PDFNet.runWithCleanup(async () => 42, 'any-license-key');
    assert.equal(v, 42);
    assert.match(await PDFNet.getVersionString(), /^\d+\.\d+\.\d+$/);
  });
});

describe('PDFDoc: opening and pages', () => {
  it('reads a nested page tree with inherited attributes', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    assert.equal(await doc.initSecurityHandler(), true);
    assert.equal(await doc.getPageCount(), 3);

    const p1 = await doc.getPage(1);
    assert.equal(await p1.getIndex(), 1);
    assert.deepEqual((await p1.getMediaBox()).toArray(), [0, 0, 612, 792]); // inherited
    assert.equal(await p1.getPageWidth(), 612);

    const p2 = await doc.getPage(2);
    assert.deepEqual((await p2.getCropBox()).toArray(), [36, 36, 576, 756]);
    assert.equal(await p2.getPageWidth(PDFNet.Page.Box.e_crop), 540);
    assert.equal(await p2.getPageWidth(PDFNet.Page.Box.e_media), 612);

    const p3 = await doc.getPage(3);
    assert.equal(await p3.getRotation(), PDFNet.Page.Rotate.e_90);
    // 842x595 rotated by 90° displays as 595x842
    assert.equal(await p3.getPageWidth(), 595);
    assert.equal(await p3.getPageHeight(), 842);
  });

  it('iterates pages with the PDFNet loop idiom', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const seen = [];
    for (const it = await doc.getPageIterator(); await it.hasNext(); it.next()) {
      seen.push(await (await it.current()).getIndex());
    }
    assert.deepEqual(seen, [1, 2, 3]);
  });

  it('throws PDFNetError on invalid page access and unreadable files', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    await assert.rejects(doc.getPage(99), PDFNet.PDFNetError);
    await assert.rejects(PDFNet.PDFDoc.createFromFilePath('/nope.pdf'), PDFNet.PDFNetError);
    await assert.rejects(PDFNet.PDFDoc.createFromBuffer(Buffer.from('not a pdf')), PDFNet.PDFNetError);
  });

  it('creates pages, rotates them, saves and reloads', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const a4 = await doc.pageCreate(new PDFNet.Rect(0, 0, 595, 842));
    assert.equal(await a4.getIndex(), 0, 'detached until pushed');
    await doc.pagePushBack(a4);
    await doc.pagePushFront(await doc.pageCreate()); // Letter
    await (await doc.getPage(2)).setRotation(PDFNet.Page.Rotate.e_270);

    const out = join(tmp, 'created.pdf');
    await doc.save(out, PDFNet.SDFDoc.SaveOptions.e_remove_unused);
    const re = await PDFNet.PDFDoc.createFromFilePath(out);
    assert.equal(await re.getPageCount(), 2);
    assert.equal(await (await re.getPage(1)).getPageWidth(), 612);
    assert.equal(await (await re.getPage(2)).getRotation(), PDFNet.Page.Rotate.e_270);
    assert.deepEqual((await (await re.getPage(2)).getMediaBox()).toArray(), [0, 0, 595, 842]);
  });

  it('removes and inserts pages through iterators', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const it = await doc.getPageIterator(2);
    await doc.pageRemove(it);
    assert.equal(await doc.getPageCount(), 2);
    assert.match((await pageTexts(doc))[1], /Rotated/);

    const blank = await doc.pageCreate();
    await doc.pageInsert(await doc.getPageIterator(1), blank);
    assert.equal(await blank.getIndex(), 1);
    assert.equal(await doc.getPageCount(), 3);
  });
});

describe('PDFDoc: merging and reordering', () => {
  it('inserts pages from another document (merge)', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const other = await PDFNet.PDFDoc.createFromFilePath(fixture('other.pdf'));
    await doc.insertPages(2, other, 1, 2, PDFNet.PDFDoc.InsertFlag.e_none);
    assert.equal(await doc.getPageCount(), 5);

    const bytes = await doc.saveMemoryBuffer(0);
    const re = await PDFNet.PDFDoc.createFromBuffer(bytes);
    const texts = await pageTexts(re);
    assert.match(texts[0], /Hello World/);
    assert.match(texts[1], /Other A/);
    assert.match(texts[2], /Other B/);
    assert.match(texts[3], /Second page/);
    assert.deepEqual((await (await re.getPage(2)).getMediaBox()).toArray(), [0, 0, 595, 842]);
    // the source document is untouched
    assert.equal(await other.getPageCount(), 2);
  });

  it('pushes a page of another document (copy semantics)', async () => {
    const doc = await PDFNet.PDFDoc.create();
    const src = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    await doc.pagePushBack(await src.getPage(3));
    const texts = await pageTexts(doc);
    assert.match(texts[0], /Rotated landscape page/);
    assert.equal(await (await doc.getPage(1)).getRotation(), PDFNet.Page.Rotate.e_90);
  });

  it('duplicates pages of the same document', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('other.pdf'));
    await doc.insertPages(10, doc, 1, 2);
    assert.deepEqual(await pageTexts(doc), ['Other A', 'Other B', 'Other A', 'Other B']);
  });

  it('moves pages within a document', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    await doc.movePages(1, doc, 3, 3); // last page to the front
    const texts = await pageTexts(doc);
    assert.match(texts[0], /Rotated/);
    assert.match(texts[1], /Hello/);
    await doc.movePages(4, doc, 1, 1); // and back to the end
    assert.match((await pageTexts(doc))[2], /Rotated/);
  });

  it('splits a document page by page', async () => {
    const src = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    for (let i = 1; i <= 3; i++) {
      const part = await PDFNet.PDFDoc.create();
      await part.insertPages(0, src, i, i);
      const re = await PDFNet.PDFDoc.createFromBuffer(await part.saveMemoryBuffer(PDFNet.SDFDoc.SaveOptions.e_remove_unused));
      assert.equal(await re.getPageCount(), 1);
    }
  });
});

describe('PDFDocInfo', () => {
  it('reads and writes metadata, including non-Latin text', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const info = await doc.getDocInfo();
    assert.equal(await info.getTitle(), 'Fixture document');
    assert.equal(await info.getAuthor(), 'Bewai');
    const created = await info.getCreationDate();
    assert.equal(created.year, 2026);
    assert.equal(created.UT, '+');
    assert.equal(created.toJSDate().toISOString(), '2026-01-02T02:04:05.000Z');

    await info.setTitle('Résumé — 日本語 ✓');
    await info.setKeywords('pdf, rust');
    await info.setModDate(await PDFNet.Date.init(2026, 10, 26, 12, 0, 0));
    const re = await PDFNet.PDFDoc.createFromBuffer(await doc.saveMemoryBuffer());
    const reInfo = await re.getDocInfo();
    assert.equal(await reInfo.getTitle(), 'Résumé — 日本語 ✓');
    assert.equal(await reInfo.getKeywords(), 'pdf, rust');
    assert.equal((await reInfo.getModDate()).toPDFString(), 'D:20261026120000Z');
  });
});

describe('TextExtractor', () => {
  it('extracts text, lines, words and accents', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const txt = await PDFNet.TextExtractor.create();
    await txt.begin(await doc.getPage(1));
    assert.equal(await txt.getAsText(), 'Hello World\nBonjour à tous, café crème.');
    assert.equal(await txt.getNumLines(), 2);
    assert.equal(await txt.getWordCount(), 7);

    const line = await txt.getFirstLine();
    const word = await line.getFirstWord();
    assert.equal(await word.getString(), 'Hello');
    const box = await word.getBBox();
    assert.ok(Math.abs(box.x1 - 72) < 1, `x1=${box.x1}`);
    assert.ok(box.y1 < 700 && box.y2 > 700 + 10, `y=${box.y1}..${box.y2}`);
    assert.equal(await (await word.getNextWord()).getString(), 'World');
    assert.equal(await (await (await word.getNextWord()).getNextWord()).isValid(), false);
    const second = await line.getNextLine();
    assert.equal(await second.getNumWords(), 5);
    assert.equal(await (await second.getNextLine()).isValid(), false);
  });

  it('restricts extraction to a clip rectangle', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const txt = await PDFNet.TextExtractor.create();
    await txt.begin(await doc.getPage(1), new PDFNet.Rect(0, 650, 612, 680));
    assert.equal(await txt.getAsText(), 'Bonjour à tous, café crème.');
  });
});

describe('TextSearch', () => {
  async function collect(doc, pattern, mode, start = -1, end = -1) {
    const ts = await PDFNet.TextSearch.create();
    assert.ok(await ts.begin(doc, pattern, mode, start, end));
    const hits = [];
    for (;;) {
      const r = await ts.run();
      if (r.code === PDFNet.TextSearch.ResultCode.e_found) hits.push(r);
      else if (r.code === PDFNet.TextSearch.ResultCode.e_done) break;
    }
    return hits;
  }

  it('finds plain text case-insensitively across pages', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const hits = await collect(doc, 'target', 0);
    assert.deepEqual(hits.map((h) => [h.page_num, h.out_str]), [[2, 'target'], [2, 'TARGET'], [2, 'target']]);
  });

  it('honours case-sensitive, whole-word and regex modes', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const M = PDFNet.TextSearch.Mode;
    assert.equal((await collect(doc, 'TARGET', M.e_case_sensitive)).length, 1);
    assert.equal((await collect(doc, 'target', M.e_whole_word)).length, 2);
    const re = await collect(doc, 'caf\\w+ cr\\w+', M.e_reg_expression);
    assert.deepEqual(re.map((h) => h.out_str), ['café crème']);
  });

  it('returns highlights, ambient string and respects the page range', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const M = PDFNet.TextSearch.Mode;
    const [hit] = await collect(doc, 'World', M.e_highlight | M.e_ambient_string, 1, 1);
    assert.match(hit.ambient_str, /Hello World/);
    const quads = await hit.highlights.getAllQuads();
    assert.equal(quads.length, 1);
    assert.ok(quads[0].p1x > 100, 'quad starts after "Hello "');
    assert.equal((await collect(doc, 'target', 0, 3, 3)).length, 0);
  });

  it('reports page boundaries with e_page_stop', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const ts = await PDFNet.TextSearch.create();
    await ts.begin(doc, 'zzz-not-found', PDFNet.TextSearch.Mode.e_page_stop);
    const codes = [];
    for (let r = await ts.run(); r.code !== PDFNet.TextSearch.ResultCode.e_done; r = await ts.run()) codes.push(r.code);
    assert.deepEqual(codes, [PDFNet.TextSearch.ResultCode.e_page, PDFNet.TextSearch.ResultCode.e_page]);
  });
});

describe('PDFDraw', () => {
  const PNG = [0x89, 0x50, 0x4e, 0x47];

  it('renders PNG at the requested DPI', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const draw = await PDFNet.PDFDraw.create(72);
    const out = join(tmp, 'page1.png');
    await draw.export(await doc.getPage(1), out, 'PNG');
    const png = readFileSync(out);
    assert.deepEqual([...png.subarray(0, 4)], PNG);
    assert.equal(png.readUInt32BE(16), 612);
    assert.equal(png.readUInt32BE(20), 792);
  });

  it('renders rotated pages with swapped dimensions and JPEG output', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const draw = await PDFNet.PDFDraw.create(36);
    const jpg = await draw.exportBuffer(await doc.getPage(3), 'JPEG');
    assert.deepEqual([...jpg.subarray(0, 2)], [0xff, 0xd8]);
    const bmp = await draw.getBitmap(await doc.getPage(3), PDFNet.PDFDraw.PixelFormat.e_rgba);
    assert.equal(bmp.width, Math.round(595 / 2));
    assert.equal(bmp.height, Math.round(842 / 2));
  });

  it('draws text: pixels under the title are dark, margins are white', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const draw = await PDFNet.PDFDraw.create(72);
    const bmp = await draw.getBitmap(await doc.getPage(1), PDFNet.PDFDraw.PixelFormat.e_gray);
    const at = (x, yPdf) => bmp.buf[(792 - yPdf) * bmp.stride + x];
    assert.equal(at(5, 5), 255);
    let dark = 0;
    for (let x = 72; x < 200; x++) for (let y = 700; y < 718; y++) if (at(x, y) < 128) dark++;
    assert.ok(dark > 100, `dark pixels in title area: ${dark}`);
  });

  it('honours setImageSize', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const draw = await PDFNet.PDFDraw.create();
    await draw.setImageSize(200, 200, true);
    const bmp = await draw.getBitmap(await doc.getPage(1));
    assert.equal(bmp.height, 200);
    assert.equal(bmp.width, Math.round(612 * (200 / 792)));
  });
});

describe('Encrypted documents', () => {
  it('stays locked until the right password is given (AES-256)', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('encrypted-aes256.pdf'));
    assert.equal(await doc.isEncrypted(), true);
    assert.equal(await doc.initSecurityHandler(), false);
    await assert.rejects(doc.getPageCount(), /initStdSecurityHandler/);
    assert.equal(await doc.initStdSecurityHandler('wrong'), false);
    assert.equal(await doc.initStdSecurityHandler('user-pw'), true);
    assert.equal(await doc.getPageCount(), 3);
    assert.match((await pageTexts(doc))[0], /café crème/);
  });

  it('opens owner-password-only documents directly (RC4-128)', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('encrypted-rc4-owner-only.pdf'));
    assert.equal(await doc.initSecurityHandler(), true);
    assert.equal(await doc.isEncrypted(), true);
    assert.deepEqual(await pageTexts(doc), ['Other A', 'Other B']);
  });
});

describe('Un-awaited calls (PDFNet samples often omit await)', () => {
  it('applies effects synchronously so later awaited calls see them', async () => {
    const doc = await PDFNet.PDFDoc.createFromFilePath(fixture('text.pdf'));
    const ts = await PDFNet.TextSearch.create();
    ts.begin(doc, 'Hello', 0); // no await
    assert.equal((await ts.run()).code, PDFNet.TextSearch.ResultCode.e_found);

    const blank = await doc.pageCreate();
    doc.pagePushBack(blank); // no await
    doc.pagePushFront(await doc.pageCreate()); // no await
    assert.equal(await doc.getPageCount(), 5);
    assert.equal(await blank.getIndex(), 5);
  });
});
