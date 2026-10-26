// End-to-end example written exactly like a @pdftron/pdfnet-node script.
// Run inside podman: make examples   (outputs go to ./out)
import { mkdirSync } from 'node:fs';
import { PDFNet } from '../dist/index.js';

const input = new URL('../test/fixtures/text.pdf', import.meta.url).pathname;
const other = new URL('../test/fixtures/other.pdf', import.meta.url).pathname;
const out = new URL('../out/', import.meta.url).pathname;
mkdirSync(out, { recursive: true });

async function main() {
  const doc = await PDFNet.PDFDoc.createFromFilePath(input);
  doc.initSecurityHandler();
  console.log('Pages:', await doc.getPageCount());

  // Text of every page
  const txt = await PDFNet.TextExtractor.create();
  for (const itr = await doc.getPageIterator(); await itr.hasNext(); itr.next()) {
    const page = await itr.current();
    await txt.begin(page);
    console.log(`--- page ${await page.getIndex()} ---\n${await txt.getAsText()}`);
  }

  // Search
  const ts = await PDFNet.TextSearch.create();
  ts.begin(doc, 'target', PDFNet.TextSearch.Mode.e_ambient_string);
  for (let r = await ts.run(); r.code !== PDFNet.TextSearch.ResultCode.e_done; r = await ts.run()) {
    if (r.code === PDFNet.TextSearch.ResultCode.e_found) console.log(`found "${r.out_str}" on page ${r.page_num}: ${r.ambient_str}`);
  }

  // Merge + metadata + save
  const merged = await PDFNet.PDFDoc.create();
  await merged.insertPages(0, doc, 1, await doc.getPageCount(), PDFNet.PDFDoc.InsertFlag.e_none);
  await merged.insertPages(await merged.getPageCount() + 1, await PDFNet.PDFDoc.createFromFilePath(other), 1, 2);
  await (await merged.getDocInfo()).setTitle('Merged document');
  await merged.save(`${out}merged.pdf`, PDFNet.SDFDoc.SaveOptions.e_remove_unused);
  console.log('Saved', `${out}merged.pdf`);

  // Thumbnails
  const draw = await PDFNet.PDFDraw.create(50);
  for (let i = 1; i <= (await merged.getPageCount()); i++) {
    await draw.export(await merged.getPage(i), `${out}page-${i}.png`, 'PNG');
  }
  console.log('Rendered', await merged.getPageCount(), 'thumbnails');
}

PDFNet.runWithCleanup(main, 'no-license-needed')
  .catch((e) => {
    console.error(e);
    process.exitCode = 1;
  })
  .then(() => PDFNet.shutdown());
