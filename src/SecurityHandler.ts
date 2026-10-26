enum AlgorithmType {
  e_RC4_40 = 1,
  e_RC4_128 = 2,
  e_AES = 3,
  e_AES_256 = 4,
}

enum Permission {
  e_owner = 1,
  e_doc_open = 2,
  e_doc_modify = 3,
  e_print = 4,
  e_print_high = 5,
  e_extract_content = 6,
  e_mod_annot = 7,
  e_fill_forms = 8,
  e_access_support = 9,
  e_assemble_doc = 10,
}

/** PDF /P bits (ISO 32000-1 table 22) of each permission. */
const P_BITS: Partial<Record<Permission, number>> = {
  [Permission.e_print]: 1 << 2,
  [Permission.e_doc_modify]: 1 << 3,
  [Permission.e_extract_content]: 1 << 4,
  [Permission.e_mod_annot]: 1 << 5,
  [Permission.e_fill_forms]: 1 << 8,
  [Permission.e_access_support]: 1 << 9,
  [Permission.e_assemble_doc]: 1 << 10,
  [Permission.e_print_high]: 1 << 11,
};

const ALL = Object.values(P_BITS).reduce((a, b) => a! | b!, 0)!;

/**
 * PDFNet.SecurityHandler — standard (password) security.
 * Set it on a document with doc.setSecurityHandler(handler); it applies on the next full save.
 */
export class SecurityHandler {
  static readonly AlgorithmType = AlgorithmType;
  static readonly Permission = Permission;

  #user = '';
  #owner = '';
  #permissions = ALL;
  #encryptMetadata = true;

  /** @internal */
  constructor(private algorithm: AlgorithmType, private readonly keyLength: number, permissions?: number, readonly fromDocument = false) {
    if (permissions !== undefined) this.#permissions = permissions;
  }

  /** AES-256 (PDF 2.0 standard security). */
  static async createDefault(): Promise<SecurityHandler> {
    return new SecurityHandler(AlgorithmType.e_AES_256, 256);
  }

  static async create(cryptType: AlgorithmType): Promise<SecurityHandler> {
    const bits = { 1: 40, 2: 128, 3: 128, 4: 256 }[cryptType] ?? 256;
    return new SecurityHandler(cryptType, bits);
  }

  async changeUserPasswordUString(password: string): Promise<void> {
    this.#user = password;
  }

  async changeUserPassword(password: string): Promise<void> {
    this.#user = password;
  }

  async changeMasterPasswordUString(password: string): Promise<void> {
    this.#owner = password;
  }

  async changeMasterPassword(password: string): Promise<void> {
    this.#owner = password;
  }

  async isUserPasswordRequired(): Promise<boolean> {
    return this.#user !== '';
  }

  async isMasterPasswordRequired(): Promise<boolean> {
    return this.#owner !== '';
  }

  async getPermission(p: Permission): Promise<boolean> {
    if (p === Permission.e_owner) return !this.fromDocument;
    if (p === Permission.e_doc_open) return true;
    const bit = P_BITS[p];
    return bit ? (this.#permissions & bit) !== 0 : false;
  }

  async setPermission(p: Permission, value: boolean): Promise<void> {
    const bit = P_BITS[p];
    if (!bit) return;
    this.#permissions = value ? this.#permissions | bit : this.#permissions & ~bit;
  }

  async setEncryptMetadata(encrypt: boolean): Promise<void> {
    this.#encryptMetadata = encrypt;
  }

  async getKeyLength(): Promise<number> {
    return this.keyLength;
  }

  async getEncryptionAlgorithmID(): Promise<AlgorithmType> {
    return this.algorithm;
  }

  async isAES(): Promise<boolean> {
    return this.algorithm >= AlgorithmType.e_AES;
  }

  async isRC4(): Promise<boolean> {
    return this.algorithm <= AlgorithmType.e_RC4_128;
  }

  async getHandlerDocName(): Promise<string> {
    return 'Standard';
  }

  /** @internal */
  toNative() {
    return {
      algorithm: this.algorithm,
      userPassword: this.#user,
      ownerPassword: this.#owner,
      permissions: this.#permissions,
      encryptMetadata: this.#encryptMetadata,
    };
  }
}
