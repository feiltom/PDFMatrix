// Type-level compatibility with code written for @pdftron/pdfnet-node (compiled with `npm run test:types`).
import { PDFNet } from '../../dist/index.js';

async function optimize(doc: PDFNet.PDFDoc, params: Record<string, unknown>): Promise<void> {
  let optSettings: PDFNet.Optimizer.OptimizerSettings | undefined;
  if (params.reduceImages) {
    const imageSettings: PDFNet.Optimizer.ImageSettings = new PDFNet.Optimizer.ImageSettings();
    imageSettings.setCompressionMode(PDFNet.Optimizer.ImageSettings.CompressionMode.e_jpeg);
    imageSettings.setQuality(4);
    imageSettings.setImageDPI(144, 96);
    imageSettings.forceRecompression(true);
    optSettings = new PDFNet.Optimizer.OptimizerSettings();
    optSettings.setColorImageSettings(imageSettings);
  }
  await PDFNet.Optimizer.optimize(doc, optSettings);
}

async function save(doc: PDFNet.PDFDoc, target?: string): Promise<true | Buffer> {
  if (target) {
    await doc.save(target, PDFNet.SDFDoc.SaveOptions.e_linearized);
    return true;
  }
  return Buffer.from(await doc.saveMemoryBuffer(PDFNet.SDFDoc.SaveOptions.e_remove_unused));
}

export const run = <T>(fn: () => Promise<T>): Promise<T> =>
  PDFNet.runWithCleanup(async () => {
    await PDFNet.startDeallocateStack();
    try {
      return await fn();
    } finally {
      await PDFNet.endDeallocateStack();
    }
  }, null, { persistentCachePath: '/tmp' });

export async function main(): Promise<void> {
  const doc: PDFNet.PDFDoc = await PDFNet.PDFDoc.create();
  const page: PDFNet.Page = await (await doc.getPageIterator()).current();
  const draw: PDFNet.PDFDraw = await PDFNet.PDFDraw.create();
  draw.setDPI(72);
  await draw.exportBuffer(page, 'JPEG');
  await doc.mergeXFDFString('<xfdf/>');
  await PDFNet.FDFDoc.createFromXFDF('<xfdf/>');
  const template = await PDFNet.Convert.createOfficeTemplateWithPath('t.docx');
  const pdf: PDFNet.PDFDoc = await template.fillTemplateJson('{}');
  await optimize(pdf, { reduceImages: true });
  await save(pdf);
  await doc.insertPages(0, pdf, 1, 1, PDFNet.PDFDoc.InsertFlag.e_none);
}
