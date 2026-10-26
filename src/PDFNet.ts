import {
  Action, Annot, AnnotBorderStyle, CircleAnnot, Destination, FreeTextAnnot, HighlightAnnot, InkAnnot, LineAnnot,
  LinkAnnot, Markup, PolygonAnnot, PolyLineAnnot, PopupAnnot, RedactionAnnot, RubberStampAnnot, SquareAnnot,
  SquigglyAnnot, StrikeOutAnnot, TextAnnot, TextMarkupAnnot, UnderlineAnnot,
} from './Annot.js';
import { Convert, HTML2PDF, OfficeToPDFOptions, TemplateDocument } from './Convert.js';
import { PDFDate } from './Date.js';
import { NotSupportedError, PDFNetError } from './errors.js';
import {
  CheckBoxWidget, ComboBoxWidget, Field, FieldIterator, ListBoxWidget, PushButtonWidget, RadioButtonGroup,
  RadioButtonWidget, SignatureWidget, TextWidget, WidgetAnnot,
} from './Field.js';
import { DigitalSignatureField, DigitalSignatureFieldIterator, VerificationOptions, VerificationResult } from './DigitalSignature.js';
import { FDFDoc } from './FDFDoc.js';
import { Font } from './Font.js';
import { ColorPt, Matrix2D, Point, QuadPoint, Rect } from './geom.js';
import { Image } from './Image.js';
import { native } from './native.js';
import { Optimizer } from './Optimizer.js';
import { Page } from './Page.js';
import { PageIterator, PDFDoc, PDFDocInfo, SDFDoc } from './PDFDoc.js';
import { PDFDraw } from './PDFDraw.js';
import { PageSet } from './PageSet.js';
import { Redaction, Redactor } from './Redactor.js';
import { SecurityHandler } from './SecurityHandler.js';
import { Stamper } from './Stamper.js';
import { Line, TextExtractor, Word } from './TextExtractor.js';
import { Highlights, TextSearch } from './TextSearch.js';

let initialized = false;

async function initialize(_licenseKey?: string | null): Promise<void> {
  initialized = true;
}

async function shutdown(): Promise<void> {
  initialized = false;
}

/**
 * Runs `fn` after initializing the library. The license key is accepted for API parity and
 * ignored: this implementation is open source and needs no key.
 */
async function runWithCleanup<T>(fn: () => Promise<T> | T, licenseKey?: string | null, _options?: { persistentCachePath?: string; [key: string]: unknown }): Promise<T> {
  await initialize(licenseKey);
  return await fn();
}

async function runWithoutCleanup<T>(fn: () => Promise<T> | T, licenseKey?: string | null, _options?: { persistentCachePath?: string; [key: string]: unknown }): Promise<T> {
  return runWithCleanup(fn, licenseKey);
}

/**
 * The PDFNet namespace, shaped like the one exported by @pdftron/pdfnet-node so existing code
 * (`const { PDFNet } = require(...)`) keeps working for the supported subset.
 */
export const PDFNet = {
  initialize,
  shutdown,
  runWithCleanup,
  runWithoutCleanup,
  isInitialized: () => initialized,
  getVersion: async () => Number.parseFloat(native.nativeVersion()),
  getVersionString: async () => native.nativeVersion(),
  // Memory-management helpers of the original SDK are no-ops: objects are garbage collected.
  startDeallocateStack: async () => {},
  endDeallocateStack: async () => {},
  beginOperation: async () => {},
  finishOperation: async () => {},
  setDefaultDiskCachingEnabled: async (_on: boolean) => {},
  setResourcesPath: async (_path: string) => true,
  addResourceSearchPath: async (_path: string) => {},

  PDFDoc,
  PDFDocInfo,
  SDFDoc,
  Page,
  PageIterator,
  PDFDraw,
  TextExtractor,
  TextExtractorLine: Line,
  TextExtractorWord: Word,
  TextSearch,
  Highlights,
  Font,
  Image,
  PageSet,
  Stamper,
  Field,
  FieldIterator,
  Annot,
  AnnotBorderStyle,
  Action,
  Destination,
  Markup,
  TextAnnot,
  FreeTextAnnot,
  SquareAnnot,
  CircleAnnot,
  LineAnnot,
  PolygonAnnot,
  PolyLineAnnot,
  InkAnnot,
  TextMarkupAnnot,
  HighlightAnnot,
  UnderlineAnnot,
  StrikeOutAnnot,
  SquigglyAnnot,
  LinkAnnot,
  RubberStampAnnot,
  PopupAnnot,
  RedactionAnnot,
  WidgetAnnot,
  TextWidget,
  CheckBoxWidget,
  RadioButtonWidget,
  RadioButtonGroup,
  ComboBoxWidget,
  ListBoxWidget,
  PushButtonWidget,
  SignatureWidget,
  SecurityHandler,
  Redactor,
  Redaction,
  Convert,
  OfficeToPDFOptions,
  HTML2PDF,
  TemplateDocument,
  DigitalSignatureField,
  DigitalSignatureFieldIterator,
  VerificationOptions,
  VerificationResult,
  Date: PDFDate,
  Rect,
  Point,
  QuadPoint,
  ColorPt,
  Matrix2D,
  Optimizer,
  FDFDoc,
  PDFNetError,
  NotSupportedError,
} as const;


/**
 * Type side of the namespace: `PDFNet.PDFDoc`, `PDFNet.Page`… can be used as types, like with
 * @pdftron/pdfnet-node (`const doc: PDFNet.PDFDoc = …`).
 */
// eslint-disable-next-line @typescript-eslint/no-namespace
export declare namespace PDFNet {
  type PDFDoc = import('./PDFDoc.js').PDFDoc;
  type PDFDocInfo = InstanceType<typeof PDFNet.PDFDocInfo>;
  type SDFDoc = InstanceType<typeof PDFNet.SDFDoc>;
  type Page = InstanceType<typeof PDFNet.Page>;
  type PageIterator = InstanceType<typeof PDFNet.PageIterator>;
  type PDFDraw = import('./PDFDraw.js').PDFDraw;
  type TextExtractor = InstanceType<typeof PDFNet.TextExtractor>;
  type TextExtractorLine = InstanceType<typeof PDFNet.TextExtractorLine>;
  type TextExtractorWord = InstanceType<typeof PDFNet.TextExtractorWord>;
  type TextSearch = InstanceType<typeof PDFNet.TextSearch>;
  type Highlights = InstanceType<typeof PDFNet.Highlights>;
  type Font = InstanceType<typeof PDFNet.Font>;
  type Image = InstanceType<typeof PDFNet.Image>;
  type PageSet = InstanceType<typeof PDFNet.PageSet>;
  type Stamper = import('./Stamper.js').Stamper;
  type Field = InstanceType<typeof PDFNet.Field>;
  type FieldIterator = InstanceType<typeof PDFNet.FieldIterator>;
  type Annot = InstanceType<typeof PDFNet.Annot>;
  type AnnotBorderStyle = InstanceType<typeof PDFNet.AnnotBorderStyle>;
  type Action = InstanceType<typeof PDFNet.Action>;
  type Destination = InstanceType<typeof PDFNet.Destination>;
  type Markup = InstanceType<typeof PDFNet.Markup>;
  type TextAnnot = InstanceType<typeof PDFNet.TextAnnot>;
  type FreeTextAnnot = InstanceType<typeof PDFNet.FreeTextAnnot>;
  type SquareAnnot = InstanceType<typeof PDFNet.SquareAnnot>;
  type CircleAnnot = InstanceType<typeof PDFNet.CircleAnnot>;
  type LineAnnot = InstanceType<typeof PDFNet.LineAnnot>;
  type PolygonAnnot = InstanceType<typeof PDFNet.PolygonAnnot>;
  type PolyLineAnnot = InstanceType<typeof PDFNet.PolyLineAnnot>;
  type InkAnnot = InstanceType<typeof PDFNet.InkAnnot>;
  type TextMarkupAnnot = InstanceType<typeof PDFNet.TextMarkupAnnot>;
  type HighlightAnnot = InstanceType<typeof PDFNet.HighlightAnnot>;
  type UnderlineAnnot = InstanceType<typeof PDFNet.UnderlineAnnot>;
  type StrikeOutAnnot = InstanceType<typeof PDFNet.StrikeOutAnnot>;
  type SquigglyAnnot = InstanceType<typeof PDFNet.SquigglyAnnot>;
  type LinkAnnot = InstanceType<typeof PDFNet.LinkAnnot>;
  type RubberStampAnnot = InstanceType<typeof PDFNet.RubberStampAnnot>;
  type PopupAnnot = InstanceType<typeof PDFNet.PopupAnnot>;
  type RedactionAnnot = InstanceType<typeof PDFNet.RedactionAnnot>;
  type WidgetAnnot = InstanceType<typeof PDFNet.WidgetAnnot>;
  type TextWidget = InstanceType<typeof PDFNet.TextWidget>;
  type CheckBoxWidget = InstanceType<typeof PDFNet.CheckBoxWidget>;
  type RadioButtonWidget = InstanceType<typeof PDFNet.RadioButtonWidget>;
  type RadioButtonGroup = InstanceType<typeof PDFNet.RadioButtonGroup>;
  type ComboBoxWidget = InstanceType<typeof PDFNet.ComboBoxWidget>;
  type ListBoxWidget = InstanceType<typeof PDFNet.ListBoxWidget>;
  type PushButtonWidget = InstanceType<typeof PDFNet.PushButtonWidget>;
  type SignatureWidget = InstanceType<typeof PDFNet.SignatureWidget>;
  type SecurityHandler = InstanceType<typeof PDFNet.SecurityHandler>;
  type Redactor = InstanceType<typeof PDFNet.Redactor>;
  type Redaction = InstanceType<typeof PDFNet.Redaction>;
  type Convert = InstanceType<typeof PDFNet.Convert>;
  type OfficeToPDFOptions = InstanceType<typeof PDFNet.OfficeToPDFOptions>;
  type HTML2PDF = InstanceType<typeof PDFNet.HTML2PDF>;
  type TemplateDocument = InstanceType<typeof PDFNet.TemplateDocument>;
  type DigitalSignatureField = InstanceType<typeof PDFNet.DigitalSignatureField>;
  type DigitalSignatureFieldIterator = InstanceType<typeof PDFNet.DigitalSignatureFieldIterator>;
  type VerificationOptions = InstanceType<typeof PDFNet.VerificationOptions>;
  type VerificationResult = InstanceType<typeof PDFNet.VerificationResult>;
  type Date = InstanceType<typeof PDFNet.Date>;
  type Rect = InstanceType<typeof PDFNet.Rect>;
  type Point = InstanceType<typeof PDFNet.Point>;
  type QuadPoint = InstanceType<typeof PDFNet.QuadPoint>;
  type ColorPt = InstanceType<typeof PDFNet.ColorPt>;
  type Matrix2D = InstanceType<typeof PDFNet.Matrix2D>;
  type Optimizer = InstanceType<typeof PDFNet.Optimizer>;
  namespace Optimizer {
    type ImageSettings = import('./Optimizer.js').Optimizer.ImageSettings;
    type MonoImageSettings = import('./Optimizer.js').Optimizer.MonoImageSettings;
    type TextSettings = import('./Optimizer.js').Optimizer.TextSettings;
    type OptimizerSettings = import('./Optimizer.js').Optimizer.OptimizerSettings;
  }
  type FDFDoc = InstanceType<typeof PDFNet.FDFDoc>;
  type PDFNetError = InstanceType<typeof PDFNet.PDFNetError>;
  type NotSupportedError = InstanceType<typeof PDFNet.NotSupportedError>;
}
