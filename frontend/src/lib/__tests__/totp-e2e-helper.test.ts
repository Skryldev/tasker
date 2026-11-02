import { describe, expect, it } from "vitest";
import {
  currentStep,
  MIN_STEP_REMAINING_MS,
  stepRemainingMs,
  totpAtStep,
  totpNow,
  waitMsForFreshStep,
} from "../../../e2e/totp";

/**
 * Unit coverage for the E2E helpers' pure step-boundary math. The TOTP
 * values are cross-checked against a known RFC 6238 test vector
 * (RFC Appendix B: secret "12345678901234567890", 30s step, SHA1) so the
 * specs' codes stay valid independent of the app under test.
 */
describe("E2E TOTP helpers", () => {
  const RFC_VECTOR_SECRET = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"; // base32("1234567890"*4)
  const RFC_VECTOR_SECRET_20 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"; // base32("1234567890")

  it("matches the RFC 6238 appendix-B test vector at T=59s", () => {
    // RFC: TOTP-SHA1(8 digits) at time 59 = 94287082; 6-digit truncation → 287082.
    const step = Math.floor(59_000 / 30_000); // = 1
    expect(totpAtStep(RFC_VECTOR_SECRET_20, step)).toBe("287082");
  });

  it("keeps step math consistent: step, remaining, wait", () => {
    const midStep = 30_000_000_000 + 10_000; // 10s into step 1_000_000
    expect(currentStep(midStep)).toBe(1_000_000);
    expect(stepRemainingMs(midStep)).toBe(20_000);
    // 20s remaining >= 5s → no wait.
    expect(waitMsForFreshStep(midStep)).toBe(0);

    const nearBoundary = 30_000_000_000 + 29_200; // 800ms remain
    expect(stepRemainingMs(nearBoundary)).toBe(800);
    // < 5s remaining → wait to the next boundary plus settle pad.
    expect(waitMsForFreshStep(nearBoundary)).toBe(800 + 500);

    const atBoundary = 30_000_000_000 + 30_000; // exact next step start
    expect(stepRemainingMs(atBoundary)).toBe(30_000);
    expect(waitMsForFreshStep(atBoundary)).toBe(0);
  });

  it("flags codes generated too close to the boundary", () => {
    const now = 30_000_000_000 + 29_000; // 1s remaining
    const remaining = stepRemainingMs(now);
    expect(remaining).toBeLessThan(MIN_STEP_REMAINING_MS);
  });

  it("totpNow equals totpAtStep(currentStep())", () => {
    const now = 1_700_000_012_345;
    expect(totpNow(RFC_VECTOR_SECRET, now)).toBe(
      totpAtStep(RFC_VECTOR_SECRET, currentStep(now)),
    );
  });

  it("codes differ across steps (never retry the same code)", () => {
    expect(totpAtStep(RFC_VECTOR_SECRET_20, 100)).not.toBe(
      totpAtStep(RFC_VECTOR_SECRET_20, 101),
    );
    void RFC_VECTOR_SECRET; // silence unused if tree-shaken
  });
});
