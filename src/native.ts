import { createRequire } from 'node:module';
import type * as Binding from '../native/index.cjs';

export type { NativeDoc, ObjRef, RenderOptions, SaveOptions, SearchOptions, PageTextResult } from '../native/index.cjs';

const require = createRequire(import.meta.url);

/** The Rust core (napi-rs addon), loaded once. */
export const native: typeof Binding = require('../native/index.cjs');
