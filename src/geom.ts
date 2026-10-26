/**
 * Geometry primitives, in PDF user space (origin bottom-left, units = points).
 * Accessors are async like in @pdftron/pdfnet-node; public fields are also readable directly.
 */

export class Point {
  constructor(public x = 0, public y = 0) {}

  static async init(x = 0, y = 0): Promise<Point> {
    return new Point(x, y);
  }
}

export class Rect {
  constructor(public x1 = 0, public y1 = 0, public x2 = 0, public y2 = 0) {}

  static async init(x1 = 0, y1 = 0, x2 = 0, y2 = 0): Promise<Rect> {
    return new Rect(x1, y1, x2, y2);
  }

  /** From a native [x1, y1, x2, y2] array. */
  static fromArray(a: ArrayLike<number>): Rect {
    return new Rect(a[0], a[1], a[2], a[3]);
  }

  toArray(): number[] {
    const n = this.normalized();
    return [n.x1, n.y1, n.x2, n.y2];
  }

  normalized(): Rect {
    return new Rect(
      Math.min(this.x1, this.x2),
      Math.min(this.y1, this.y2),
      Math.max(this.x1, this.x2),
      Math.max(this.y1, this.y2),
    );
  }

  async get(): Promise<{ x1: number; y1: number; x2: number; y2: number }> {
    return { x1: this.x1, y1: this.y1, x2: this.x2, y2: this.y2 };
  }

  async set(x1: number, y1: number, x2: number, y2: number): Promise<void> {
    Object.assign(this, { x1, y1, x2, y2 });
  }

  async attach(): Promise<void> {}

  async update(): Promise<boolean> {
    return true;
  }

  async width(): Promise<number> {
    return Math.abs(this.x2 - this.x1);
  }

  async height(): Promise<number> {
    return Math.abs(this.y2 - this.y1);
  }

  async contains(x: number, y: number): Promise<boolean> {
    const n = this.normalized();
    return x >= n.x1 && x <= n.x2 && y >= n.y1 && y <= n.y2;
  }

  /** Sets this rect to the intersection of a and b; returns false when they do not overlap. */
  async intersectRect(a: Rect, b: Rect): Promise<boolean> {
    const p = a.normalized();
    const q = b.normalized();
    const r = new Rect(Math.max(p.x1, q.x1), Math.max(p.y1, q.y1), Math.min(p.x2, q.x2), Math.min(p.y2, q.y2));
    if (r.x1 >= r.x2 || r.y1 >= r.y2) return false;
    await this.set(r.x1, r.y1, r.x2, r.y2);
    return true;
  }

  async normalize(): Promise<void> {
    const n = this.normalized();
    await this.set(n.x1, n.y1, n.x2, n.y2);
  }

  async inflate(amount: number, amountY = amount): Promise<void> {
    const n = this.normalized();
    await this.set(n.x1 - amount, n.y1 - amountY, n.x2 + amount, n.y2 + amountY);
  }
}

/** Quadrilateral; p1..p4 = lower-left, lower-right, upper-right, upper-left. */
export class QuadPoint {
  constructor(
    public p1x = 0, public p1y = 0,
    public p2x = 0, public p2y = 0,
    public p3x = 0, public p3y = 0,
    public p4x = 0, public p4y = 0,
  ) {}

  static async init(...coords: number[]): Promise<QuadPoint> {
    return new QuadPoint(...coords);
  }

  static fromArray(q: ArrayLike<number>): QuadPoint {
    return new QuadPoint(q[0], q[1], q[2], q[3], q[4], q[5], q[6], q[7]);
  }

  toArray(): number[] {
    return [this.p1x, this.p1y, this.p2x, this.p2y, this.p3x, this.p3y, this.p4x, this.p4y];
  }
}

/** Color point; meaning depends on the colorspace (1 = gray, 3 = RGB, 4 = CMYK components). */
export class ColorPt {
  c: [number, number, number, number];

  constructor(x = 0, y = 0, z = 0, w = 0) {
    this.c = [x, y, z, w];
  }

  static async init(x = 0, y = 0, z = 0, w = 0): Promise<ColorPt> {
    return new ColorPt(x, y, z, w);
  }

  async set(x = 0, y = 0, z = 0, w = 0): Promise<void> {
    this.c = [x, y, z, w];
  }

  async get(colorantNum: number): Promise<number> {
    return this.c[colorantNum] ?? 0;
  }

  async setColorantNum(colorantNum: number, value: number): Promise<void> {
    this.c[colorantNum] = value;
  }
}

/** Affine matrix [a b c d h v] in PDF order. */
export class Matrix2D {
  constructor(
    public m_a = 1, public m_b = 0,
    public m_c = 0, public m_d = 1,
    public m_h = 0, public m_v = 0,
  ) {}

  static async create(a = 1, b = 0, c = 0, d = 1, h = 0, v = 0): Promise<Matrix2D> {
    return new Matrix2D(a, b, c, d, h, v);
  }

  static async createIdentityMatrix(): Promise<Matrix2D> {
    return new Matrix2D();
  }

  static async createRotationMatrix(angleRad: number): Promise<Matrix2D> {
    const c = Math.cos(angleRad);
    const s = Math.sin(angleRad);
    return new Matrix2D(c, s, -s, c, 0, 0);
  }

  async mult(x: number, y: number): Promise<{ x: number; y: number }> {
    return { x: this.m_a * x + this.m_c * y + this.m_h, y: this.m_b * x + this.m_d * y + this.m_v };
  }

  /** Returns this × m (this applied first, then m). */
  async multiply(m: Matrix2D): Promise<Matrix2D> {
    return new Matrix2D(
      this.m_a * m.m_a + this.m_b * m.m_c,
      this.m_a * m.m_b + this.m_b * m.m_d,
      this.m_c * m.m_a + this.m_d * m.m_c,
      this.m_c * m.m_b + this.m_d * m.m_d,
      this.m_h * m.m_a + this.m_v * m.m_c + m.m_h,
      this.m_h * m.m_b + this.m_v * m.m_d + m.m_v,
    );
  }

  async inverse(): Promise<Matrix2D> {
    const det = this.m_a * this.m_d - this.m_b * this.m_c;
    const a = this.m_d / det;
    const b = -this.m_b / det;
    const c = -this.m_c / det;
    const d = this.m_a / det;
    return new Matrix2D(a, b, c, d, -(this.m_h * a + this.m_v * c), -(this.m_h * b + this.m_v * d));
  }
}
