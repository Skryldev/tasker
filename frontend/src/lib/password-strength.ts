/**
 * Client-side strength scoring for the UI meter (Phase 12.5 Bug 8).
 * Advisory only — the server policy (ADR-0016) is the enforcement point.
 * The scale mirrors `security::password::strength` on the backend.
 */
export function passwordStrength(password: string): 0 | 1 | 2 | 3 | 4 {
  let score = 0;
  const len = password.length;
  if (len >= 12) score += 1;
  if (len >= 16) score += 1;
  const hasLower = /[a-z]/.test(password);
  const hasUpper = /[A-Z]/.test(password);
  const hasDigit = /[0-9]/.test(password);
  const hasSpecial = /[^a-zA-Z0-9]/.test(password);
  const classes = [hasLower, hasUpper, hasDigit, hasSpecial].filter(Boolean).length;
  if (classes >= 3) score += 1;
  if (classes === 4) score += 1;
  return Math.min(4, score) as 0 | 1 | 2 | 3 | 4;
}

/** Weak / fair / strong / very strong labels for the 0-4 score. */
export function strengthLabel(score: number): string {
  const labels = ["Very weak", "Weak", "Fair", "Strong", "Very strong"] as const;
  return labels[Math.max(0, Math.min(4, score))] as string;
}

/** Tailwind tone class for the filled segments at a given score. */
export function strengthTone(score: number): string {
  if (score <= 1) return "bg-destructive";
  if (score === 2) return "bg-warning";
  if (score === 3) return "bg-primary";
  return "bg-success";
}
