import { call } from './errors.js';
import type { PDFDoc } from './PDFDoc.js';

enum CompressionMode {
  e_retain = 0,
  e_flate = 1,
  e_jpeg = 2,
  e_jpeg2000 = 3,
  e_none = 4,
}

enum DownsampleMode {
  e_off = 0,
  e_default = 1,
}

/** PDFNet.Optimizer.ImageSettings — color/grayscale image settings. */
class ImageSettings {
  static readonly CompressionMode = CompressionMode;
  static readonly DownsampleMode = DownsampleMode;

  /** @internal */
  opts = {
    compressionMode: CompressionMode.e_retain as number,
    quality: 5,
    maxDpi: 225,
    resampleDpi: 150,
    downsample: true,
    forceRecompression: false,
    forceChanges: false,
  };

  // Setters apply immediately (PDFNet code often calls them without await).
  setCompressionMode(mode: CompressionMode): Promise<void> {
    this.opts.compressionMode = mode;
    return Promise.resolve();
  }

  /** 1 (smallest) – 10 (best). */
  setQuality(quality: number): Promise<void> {
    this.opts.quality = Math.round(quality);
    return Promise.resolve();
  }

  /** Images above `maximum` dpi are resampled to `resampling` dpi. */
  setImageDPI(maximum: number, resampling: number): Promise<void> {
    this.opts.maxDpi = maximum;
    this.opts.resampleDpi = resampling;
    return Promise.resolve();
  }

  setDownsampleMode(mode: DownsampleMode): Promise<void> {
    this.opts.downsample = mode !== DownsampleMode.e_off;
    return Promise.resolve();
  }

  /** Recompress images already using the selected compression (kept only if smaller). */
  forceRecompression(force: boolean): Promise<void> {
    this.opts.forceRecompression = force;
    return Promise.resolve();
  }

  /** Keep the recompressed image even when it is not smaller. */
  forceChanges(force: boolean): Promise<void> {
    this.opts.forceChanges = force;
    return Promise.resolve();
  }
}

/** Monochrome settings: accepted for API parity (1-bit images are left untouched). */
class MonoImageSettings {
  static readonly CompressionMode = { e_jbig2: 0, e_flate: 1, e_none: 2, e_ccitt: 3 } as const;
  static readonly DownsampleMode = DownsampleMode;
  setCompressionMode(_m: number): Promise<void> { return Promise.resolve(); }
  setImageDPI(_max: number, _resampling: number): Promise<void> { return Promise.resolve(); }
  setDownsampleMode(_m: number): Promise<void> { return Promise.resolve(); }
  setJBIG2Threshold(_t: number): Promise<void> { return Promise.resolve(); }
  forceRecompression(_f: boolean): Promise<void> { return Promise.resolve(); }
  forceChanges(_f: boolean): Promise<void> { return Promise.resolve(); }
}

/** Text settings: accepted for API parity (fonts are not re-subsetted). */
class TextSettings {
  subsetFonts(_subset: boolean): Promise<void> { return Promise.resolve(); }
  embedFonts(_embed: boolean): Promise<void> { return Promise.resolve(); }
}

class OptimizerSettings {
  /** @internal */
  color = new ImageSettings();
  /** @internal */
  gray = new ImageSettings();

  setColorImageSettings(settings: ImageSettings): Promise<void> {
    this.color = settings;
    return Promise.resolve();
  }

  setGrayscaleImageSettings(settings: ImageSettings): Promise<void> {
    this.gray = settings;
    return Promise.resolve();
  }

  setMonoImageSettings(_settings: MonoImageSettings): Promise<void> {
    return Promise.resolve();
  }

  setTextSettings(_settings: TextSettings): Promise<void> {
    return Promise.resolve();
  }

  removeCustomEntries(_remove: boolean): Promise<void> {
    return Promise.resolve();
  }
}

export interface OptimizeResult {
  images: number;
  optimized: number;
  bytesBefore: number;
  bytesAfter: number;
}

/**
 * PDFNet.Optimizer — downsamples and recompresses images (8-bit gray/RGB; JPEG or Flate).
 * Unused objects are removed when saving with SDFDoc.SaveOptions.e_remove_unused.
 */
export class Optimizer {
  static readonly ImageSettings = ImageSettings;
  static readonly MonoImageSettings = MonoImageSettings;
  static readonly TextSettings = TextSettings;
  static readonly OptimizerSettings = OptimizerSettings;

  static async optimize(doc: PDFDoc, settings?: OptimizerSettings | null): Promise<OptimizeResult> {
    const s = settings ?? new OptimizerSettings();
    return call('Optimizer.optimize', () => doc.native().optimizeImages(s.color.opts, s.gray.opts));
  }
}

// Type aliases so that `PDFNet.Optimizer.ImageSettings` works as a type too.
// eslint-disable-next-line @typescript-eslint/no-namespace
export declare namespace Optimizer {
  type ImageSettings = InstanceType<typeof Optimizer.ImageSettings>;
  type MonoImageSettings = InstanceType<typeof Optimizer.MonoImageSettings>;
  type TextSettings = InstanceType<typeof Optimizer.TextSettings>;
  type OptimizerSettings = InstanceType<typeof Optimizer.OptimizerSettings>;
}
