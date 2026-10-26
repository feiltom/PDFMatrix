// Generates the test fixtures by hand (no PDF library involved), so tests do not depend on the
// code under test to produce their inputs. Run: node test/fixtures/make-fixtures.mjs
import { writeFileSync } from 'node:fs';
import { crc32, deflateSync } from 'node:zlib';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

/** Serializes numbered objects into a PDF with a classic xref table (latin1 bytes). */
function buildPdf(objects, trailerExtra = '') {
  let out = '%PDF-1.7\n%\xE2\xE3\xCF\xD3\n';
  const offsets = [];
  objects.forEach((body, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const o of offsets) out += `${String(o).padStart(10, '0')} 00000 n \n`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R ${trailerExtra}>>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(out, 'latin1');
}

function stream(content) {
  return `<< /Length ${Buffer.byteLength(content, 'latin1')} >>\nstream\n${content}\nendstream`;
}

const font = '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>';

// 1 catalog, 2 root Pages (inherits MediaBox + Resources), 3 intermediate Pages,
// 4-6 pages, 7-9 contents, 10 font, 11 info.
const text = buildPdf(
  [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 3 /MediaBox [0 0 612 792] /Resources << /Font << /F1 10 0 R >> >> >>',
    '<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 >>',
    '<< /Type /Page /Parent 3 0 R /Contents 7 0 R >>',
    '<< /Type /Page /Parent 3 0 R /Contents 8 0 R /CropBox [36 36 576 756] >>',
    '<< /Type /Page /Parent 2 0 R /Contents 9 0 R /Rotate 90 /MediaBox [0 0 842 595] >>',
    stream('BT /F1 24 Tf 72 700 Td (Hello World) Tj ET\nBT /F1 12 Tf 72 660 Td (Bonjour \xE0 tous, caf\xE9 cr\xE8me.) Tj ET'),
    stream('BT /F1 12 Tf 72 700 Td (Second page with the word target here.) Tj 0 -20 Td (Another TARGET, and targets.) Tj ET'),
    stream('BT /F1 18 Tf 72 500 Td (Rotated landscape page) Tj ET'),
    font,
    '<< /Title (Fixture document) /Author (Bewai) /Producer (make-fixtures) /CreationDate (D:20260102030405+01\'00\') >>',
  ],
  '/Info 11 0 R ',
);
writeFileSync(join(here, 'text.pdf'), text);

// A two-page document with distinct text, used for merge tests.
const other = buildPdf([
  '<< /Type /Catalog /Pages 2 0 R >>',
  '<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>',
  '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 7 0 R >> >> /Contents 5 0 R >>',
  '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>',
  stream('BT /F1 14 Tf 50 780 Td (Other A) Tj ET'),
  stream('BT /F1 14 Tf 50 780 Td (Other B) Tj ET'),
  font,
]);
writeFileSync(join(here, 'other.pdf'), other);

// An AcroForm document: text (NeedAppearances, no AP), checkbox with its own On/Off appearances,
// a radio group of two buttons, a combo box and a list box.
const ap = (content) => stream(content).replace('<< /Length', '<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length');
const form = buildPdf([
  '<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>',
  '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
  '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [6 0 R 7 0 R 9 0 R 10 0 R 11 0 R 12 0 R] /Contents 15 0 R /Resources << /Font << /F1 5 0 R >> >> >>',
  '<< /Fields [6 0 R 7 0 R 8 0 R 11 0 R 12 0 R] /DR << /Font << /Helv 5 0 R >> >> /DA (/Helv 0 Tf 0 g) /NeedAppearances true >>',
  font,
  '<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Jean Dupont) /Rect [100 700 300 720] /P 3 0 R /DA (/Helv 12 Tf 0 0 1 rg) /MK << /BC [0] >> /F 4 >>',
  '<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [100 660 120 680] /P 3 0 R /AP << /N << /Oui 13 0 R /Off 14 0 R >> >> /F 4 >>',
  '<< /FT /Btn /Ff 49152 /T (size) /V /M /Kids [9 0 R 10 0 R] >>',
  '<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [100 620 120 640] /P 3 0 R /AS /Off /AP << /N << /S 13 0 R /Off 14 0 R >> >> /F 4 >>',
  '<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [140 620 160 640] /P 3 0 R /AS /M /AP << /N << /M 13 0 R /Off 14 0 R >> >> /F 4 >>',
  '<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (country) /V (FR) /Opt [[(FR) (France)] [(BE) (Belgique)] [(CH) (Suisse)]] /Rect [100 580 300 600] /P 3 0 R /DA (/Helv 10 Tf 0 g) /F 4 >>',
  '<< /Type /Annot /Subtype /Widget /FT /Ch /T (lang) /V (fr) /Opt [(en) (fr) (de)] /Rect [100 500 200 560] /P 3 0 R /DA (/Helv 10 Tf 0 g) /F 4 >>',
  ap('0 0 1 rg 4 4 12 12 re f'),
  ap(''),
  stream('BT /F1 10 Tf 30 705 Td (Name:) Tj 0 -40 Td (Agree:) Tj 0 -40 Td (Size:) Tj ET'),
]);
writeFileSync(join(here, 'form.pdf'), form);

// 4x4 RGBA PNG: red, half transparent.
const raw = Buffer.alloc(4 * (1 + 4 * 4));
for (let y = 0; y < 4; y++) {
  raw[y * 17] = 0; // filter: none
  for (let x = 0; x < 4; x++) raw.set([255, 0, 0, 128], y * 17 + 1 + x * 4);
}
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
};
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(4, 0);
ihdr.writeUInt32BE(4, 4);
ihdr.set([8, 6, 0, 0, 0], 8);
writeFileSync(
  join(here, 'red-alpha.png'),
  Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]),
);

// Office fixtures for Convert tests (converted by Gotenberg/LibreOffice).
/** Minimal ZIP writer (stored entries, no compression). */
function zip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, content] of entries) {
    const data = Buffer.from(content, 'utf8');
    const nameBuf = Buffer.from(name, 'utf8');
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBuf.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBuf.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBuf, data);
    centrals.push(central, nameBuf);
    offset += local.length + nameBuf.length + data.length;
  }
  const cd = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cd.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, end]);
}

const W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"';
const para = (text, bold = false, style = '') =>
  `<w:p>${style ? `<w:pPr><w:pStyle w:val="${style}"/></w:pPr>` : ''}<w:r>${bold ? '<w:rPr><w:b/></w:rPr>' : ''}<w:t xml:space="preserve">${text}</w:t></w:r></w:p>`;
const cell = (t) => `<w:tc><w:p><w:r><w:t>${t}</w:t></w:r></w:p></w:tc>`;
writeFileSync(
  join(here, 'letter.docx'),
  zip([
    ['[Content_Types].xml', '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>'],
    ['_rels/.rels', '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>'],
    [
      'word/document.xml',
      `<?xml version="1.0" encoding="UTF-8"?><w:document ${W}><w:body>` +
        para('Lettre de test', true) +
        para('Bonjour, voici un document Word généré à la main : café, crème brûlée, garçon, œuvre.') +
        `<w:tbl><w:tr>${cell('Produit')}${cell('Prix')}</w:tr><w:tr>${cell('Croissant')}${cell('1,20 €')}</w:tr></w:tbl>` +
        para('Fin du document.') +
        '<w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:body></w:document>',
    ],
  ]),
);
writeFileSync(join(here, 'note.rtf'), '{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Helvetica;}}\\f0\\fs24 Note RTF avec accents : \\\'e9t\\\'e9, \\\'e0 bient\\\'f4t.\\par}');


// DocGen template mimicking what Word produces (split runs, proofErr, typographic quotes).
{
  const R = (t, bold = false) => `<w:r>${bold ? '<w:rPr><w:b/></w:rPr>' : ''}<w:t xml:space="preserve">${t}</w:t></w:r>`;
  const P = (...runs) => `<w:p>${runs.join('')}</w:p>`;
  const C = (...runs) => `<w:tc><w:tcPr><w:tcW w:w="3000" w:type="dxa"/></w:tcPr>${P(...runs)}</w:tc>`;
  const proof = '<w:proofErr w:type="spellStart"/>';
  const body = [
    P(R('Facture n°', true), R('{{nu', true), proof, R('me', true), R('ro}}', true)),
    P(R('Client : {{client.nom}}, {{client.adresse.ville}}')),
    P(R('{{logo}}')),
    P(R('Statut : '), R('{{if payee}}payée{{else}}à régler{{endif}}'), R('. {{if total > 100}}Remise appliquée.{{endif}}')),
    `<w:tbl><w:tblPr><w:tblBorders><w:insideH w:val="single" w:sz="4"/></w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>` +
      `<w:tr>${C(R('Produit', true))}${C(R('Qté', true))}${C(R('Prix', true))}</w:tr>` +
      `<w:tr>${C(R('{{loop lignes}}{{produit}}'))}${C(R('{{qte}}'))}${C(R('{{prix}} €{{endloop}}'))}</w:tr>` +
      `<w:tr>${C(R('Total', true))}${C(R(''))}${C(R('{{total}} €', true))}</w:tr></w:tbl>`,
    P(R('Notes :')),
    P(R('{{loop notes}}')),
    P(R('- {{this}}')),
    P(R('{{endloop}}')),
    P(R('{{if client.vip == “oui”}}')),
    P(R('Merci pour votre fidélité VIP.')),
    P(R('{{else}}')),
    P(R('Merci pour votre commande.')),
    P(R('{{endif}}')),
    P(R('{{loop sections}}')),
    P(R('Section {{titre}} :', true)),
    P(R('{{loop items}}')),
    P(R('* {{nom}} (n°{{loop_number}})')),
    P(R('{{endloop}}')),
    P(R('{{endloop}}')),
    P(R('Fin.')),
  ].join('');
  const NS = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"';
  writeFileSync(
    join(here, 'invoice-template.docx'),
    zip([
      ['[Content_Types].xml', '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/></Types>'],
      ['_rels/.rels', '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>'],
      ['word/_rels/document.xml.rels', '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdH1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/></Relationships>'],
      ['word/header1.xml', `<?xml version="1.0" encoding="UTF-8"?><w:hdr ${NS}>${P(R('En-tête : facture {{numero}} &amp; client {{client.nom}}'))}</w:hdr>`],
      ['word/document.xml', `<?xml version="1.0" encoding="UTF-8"?><w:document ${NS}><w:body>${body}<w:sectPr><w:headerReference w:type="default" r:id="rIdH1"/><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:body></w:document>`],
    ]),
  );
  // An invalid template (unclosed loop).
  writeFileSync(
    join(here, 'broken-template.docx'),
    zip([
      ['[Content_Types].xml', '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>'],
      ['_rels/.rels', '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>'],
      ['word/document.xml', `<?xml version="1.0" encoding="UTF-8"?><w:document ${NS}><w:body>${P(R('{{loop lignes}}'))}${P(R('{{produit}}'))}</w:body></w:document>`],
    ]),
  );
}

console.log('fixtures written to', here);
