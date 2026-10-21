// Secrets for sign-in: device codes, user codes, magic-link and session tokens.
// Only SHA-256 hashes of secrets are stored, so a database leak gives no working tokens.

import { createHash, randomBytes, randomInt } from "node:crypto";

export function secret(prefix = ""): string {
  return prefix + randomBytes(32).toString("base64url");
}

export function hash(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

// Easy to read and type: no vowels (no words), no 0/O or 1/I.
const ALPHABET = "BCDFGHJKLMNPQRSTVWXZ";

export function userCode(): string {
  const pick = () => Array.from({ length: 4 }, () => ALPHABET[randomInt(ALPHABET.length)]).join("");
  return `${pick()}-${pick()}`;
}

export function normalizeUserCode(code: string): string {
  const c = code.toUpperCase().replace(/[^A-Z]/g, "");
  return c.length === 8 ? `${c.slice(0, 4)}-${c.slice(4)}` : c;
}

export function normalizeEmail(email: string): string | null {
  const e = email.trim().toLowerCase();
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(e) && e.length <= 254 ? e : null;
}

// Fixed-window rate limits kept in memory (per server instance).
export class Limiter {
  private hits = new Map<string, { count: number; until: number }>();
  constructor(
    private max: number,
    private windowMs: number,
  ) {}
  allow(key: string): boolean {
    const now = Date.now();
    const h = this.hits.get(key);
    if (!h || h.until < now) {
      if (this.hits.size > 50_000) this.hits.clear();
      this.hits.set(key, { count: 1, until: now + this.windowMs });
      return true;
    }
    h.count += 1;
    return h.count <= this.max;
  }
}
