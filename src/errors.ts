/**
 * Error thrown by every API of the module. Mirrors the PDFNetException objects thrown by
 * @pdftron/pdfnet-node (message + location).
 */
export class PDFNetError extends Error {
  readonly location: string;

  constructor(message: string, location = '', options?: { cause?: unknown }) {
    super(location ? `${message} (${location})` : message, options);
    this.name = 'PDFNetError';
    this.location = location;
  }
}

export class NotSupportedError extends PDFNetError {
  constructor(feature: string, hint = '') {
    super(`${feature} is not supported by this implementation${hint ? `: ${hint}` : ''}`);
    this.name = 'NotSupportedError';
  }
}

/** Runs a synchronous native call and re-throws its errors as PDFNetError. */
export function call<T>(location: string, fn: () => T): T {
  try {
    return fn();
  } catch (e) {
    if (e instanceof PDFNetError) throw e;
    throw new PDFNetError(e instanceof Error ? e.message : String(e), location, { cause: e });
  }
}
