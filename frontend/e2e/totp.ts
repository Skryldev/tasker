import { createHmac } from "node:crypto";

/**
 * TOTP helpers for the MFA E2E specs. Pure functions over epoch time so the
 * step-boundary logic is unit-testable without a browser.
 *
 * Reference mapping used throughout (independent of the app under test):
 *   Aban 1, 1405  = 2026-10-23
 *   Aban 15, 1405 = 2026-11-06
 */

export const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
export const TOTP_STEP_MS = 30_000;
/** Codes are generated only when at least this much of the step remains. */
export const MIN_STEP_REMAINING_MS = 5_000;

export function base32Decode(input: string): Buffer {
  let bits = 0;
  let value = 0;
  const out: number[] = [];
  for (const c of input.replace(/=+$/, "").toUpperCase()) {
    const idx = BASE32.indexOf(c);
    if (idx < 0) continue;
    value = (value << 5) | idx;
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return Buffer.from(out);
}

/** RFC 6238 TOTP for a Base32 secret at an explicit step counter. */
export function totpAtStep(secretBase32: string, step: number): string {
  const key = base32Decode(secretBase32);
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(step));
  const digest = createHmac("sha1", key).update(counter).digest();
  const offset = digest[digest.length - 1]! & 0x0f;
  const binary =
    ((digest[offset]! & 0x7f) << 24) |
    (digest[offset + 1]! << 16) |
    (digest[offset + 2]! << 8) |
    digest[offset + 3]!;
  return String(binary % 1_000_000).padStart(6, "0");
}

export const currentStep = (nowMs = Date.now()) =>
  Math.floor(nowMs / TOTP_STEP_MS);

/** Milliseconds remaining in the current step at `nowMs`. */
export const stepRemainingMs = (nowMs = Date.now()) =>
  TOTP_STEP_MS - (nowMs % TOTP_STEP_MS);

/**
 * How long to wait so that a code generated immediately afterwards is
 * safely inside a fresh step: 0 when at least {@link MIN_STEP_REMAINING_MS}
 * remain, otherwise the time to the next boundary plus a small settle pad.
 */
export const waitMsForFreshStep = (nowMs = Date.now()) => {
  const remaining = stepRemainingMs(nowMs);
  if (remaining >= MIN_STEP_REMAINING_MS) {
    return 0;
  }
  return remaining + 500;
};

/** Convenience: the current step's code, computed at `nowMs`. */
export function totpNow(secretBase32: string, nowMs = Date.now()): string {
  return totpAtStep(secretBase32, currentStep(nowMs));
}
