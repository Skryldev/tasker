import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DateField } from "@/components/ui/date-field";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";

/**
 * UI-level calendar tests: the picker displays Solar Hijri values and hands
 * the app back the canonical Gregorian `YYYY-MM-DD`. Reference mapping:
 * Aban 15, 1405 = 2026-11-06 (Aban 1 = 2026-10-23).
 */
describe("DateField (Doran calendar boundary)", () => {
  afterEach(cleanup);

  it("displays the Jalali form of the canonical Gregorian value", () => {
    render(<DateField value="2026-10-31" onChange={() => undefined} ariaLabel="Due date" />);
    const input = screen.getByLabelText("Due date") as HTMLInputElement;
    expect(input.value).toBe("۱۴۰۵/۰۸/۰۹");
  });

  it("shows an empty trigger for an unset value", () => {
    render(
      <DateField value={null} onChange={() => undefined} ariaLabel="Due date" placeholder="۱۴۰۵/۰۱/۰۱" />,
    );
    const input = screen.getByLabelText("Due date") as HTMLInputElement;
    expect(input.value).toBe("");
    expect(input.getAttribute("placeholder")).toBe("۱۴۰۵/۰۱/۰۱");
  });

  it("selecting a Persian day in the grid yields the correct Gregorian day", async () => {
    const user = userEvent.setup();
    let received: string | undefined;
    render(
      <DateField
        value="2026-10-31"
        onChange={(v) => (received = v)}
        ariaLabel="Due date"
      />,
    );
    // Open the pop-over calendar via the icon button (Doran's own control).
    for (const b of Array.from(document.querySelectorAll("button"))) {
      await user.click(b).catch(() => undefined);
      if (document.querySelector('[role="grid"]')) break;
    }
    expect(document.querySelector('[role="grid"]')).not.toBeNull();
    // The grid shows Aban 1405 (the value's month). Aban 15 = 2026-11-06.
    const day15 = Array.from(
      document.querySelectorAll('[role="grid"] button'),
    ).find((b) => b.textContent?.trim() === "۱۵");
    expect(day15).toBeTruthy();
    await user.click(day15!);
    expect(received).toBe("2026-11-06");
  });

  it("typing a Jalali date parses on blur into the canonical Gregorian day", async () => {
    const user = userEvent.setup();
    let received: string | undefined;
    render(<DateField value={null} onChange={(v) => (received = v)} ariaLabel="Due date" />);
    const input = screen.getByLabelText("Due date") as HTMLInputElement;
    await user.type(input, "1405/08/15");
    await user.tab(); // Doran surfaces parse errors on blur, not per keystroke
    expect(received).toBe("2026-11-06");
  });

  it("clearing the field emits the empty string (optional semantics)", async () => {
    const user = userEvent.setup();
    let received: string | undefined;
    render(
      <DateField
        value="2026-10-31"
        onChange={(v) => (received = v)}
        ariaLabel="Due date"
      />,
    );
    const input = screen.getByLabelText("Due date") as HTMLInputElement;
    await user.clear(input);
    await user.tab();
    expect(received).toBe("");
  });

  it("ConfirmDialog keeps destructive confirmations accessible (focus on open)", () => {
    render(
      <ConfirmDialog
        open
        onOpenChange={() => undefined}
        title="Delete this task?"
        description="x"
        confirmLabel="Delete task"
        onConfirm={() => undefined}
      />,
    );
    const dialog = screen.getByRole("alertdialog");
    expect(dialog).toBeTruthy();
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(screen.getByRole("button", { name: "Delete task" })).toBeTruthy();
  });
});
