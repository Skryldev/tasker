import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AvatarError, fileToAvatarDataUrl } from "@/lib/avatar";
import { formatInstant } from "@/lib/utils";
import { Timestamp } from "@/components/app/Timestamp";
import { TIMEZONE_NONE, timezoneValue } from "@/components/app/TimezoneSelect";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AuthProvider } from "@/lib/auth";
import { api } from "@/api/client";

// jsdom has no <canvas> 2D context or createImageBitmap, so the crop path is
// covered by the Playwright suite; the input validation is testable here.
describe("avatar upload validation", () => {
  it("rejects non-image types before any processing", async () => {
    const file = new File(["x"], "doc.pdf", { type: "application/pdf" });
    await expect(fileToAvatarDataUrl(file)).rejects.toThrow(AvatarError);
    await expect(fileToAvatarDataUrl(file)).rejects.toThrow(/JPG, PNG or WebP/);
  });

  it("rejects files over 2 MB before any processing", async () => {
    const file = new File([new Uint8Array(2 * 1024 * 1024 + 1)], "big.png", {
      type: "image/png",
    });
    await expect(fileToAvatarDataUrl(file)).rejects.toThrow(/2 MB or smaller/);
  });
});

describe("formatInstant (profile timezone display)", () => {
  // 2026-03-01 12:00:00 UTC. Tehran is UTC+3:30, New York is UTC-5 (EST).
  const instant = Date.UTC(2026, 2, 1, 12, 0, 0);

  it("renders the same instant differently per timezone", () => {
    const tehran = formatInstant(instant, "Asia/Tehran");
    const newYork = formatInstant(instant, "America/New_York");
    expect(tehran).toContain("15:30");
    expect(newYork).toContain("07:00");
    expect(tehran).not.toEqual(newYork);
  });

  it("falls back safely for null and invalid zones", () => {
    expect(formatInstant(null, "Asia/Tehran")).toBe("—");
    // An invalid stored zone must not throw.
    expect(formatInstant(instant, "Not/AZone")).toBeTruthy();
  });
});

describe("Timestamp component", () => {
  function renderAt(value: number | null) {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <AuthProvider>
          <Timestamp value={value} />
        </AuthProvider>
      </QueryClientProvider>,
    );
  }

  it("shows a dash for missing values", () => {
    renderAt(null);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});

describe("timezone select value mapping", () => {
  it("maps the none marker to null", () => {
    expect(timezoneValue(TIMEZONE_NONE)).toBeNull();
    expect(timezoneValue("Europe/Berlin")).toBe("Europe/Berlin");
  });
});

describe("profile API surface", () => {
  it("exposes avatar URLs derived from ids", () => {
    expect(api.userAvatarUrl("u1")).toBe("/api/v1/users/u1/avatar");
    expect(api.teamAvatarUrl("t1")).toBe("/api/v1/teams/t1/avatar");
  });
});
