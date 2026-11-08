import { expect, test } from "@playwright/test";

/**
 * Theme engine journeys (Phase 9): the switcher applies data-theme +
 * persists, a reload re-applies the theme before first paint (no flash), and
 * the legacy dark/light pair keeps working.
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

test.beforeEach(async ({ page }) => {
  await page.goto("/login");
  await page.getByLabel("Email").fill(ADMIN.email);
  await page.getByLabel("Password").fill(ADMIN.password);
  await page.getByTestId("login-submit").click();
  await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
});

test("lists every shipped theme in the switcher", async ({ page }) => {
  await page.getByTestId("theme-switcher").click();
  const expected = [
    "light",
    "dark",
    "midnight",
    "nord",
    "dracula",
    "solarized-light",
    "solarized-dark",
    "rose-pine",
    "catppuccin-mocha",
  ];
  for (const id of expected) {
    await expect(page.getByTestId(`theme-option-${id}`)).toBeVisible();
  }
});

test("selects a dark theme, persists it, and survives a reload without flash", async ({
  page,
}) => {
  await page.getByTestId("theme-switcher").click();
  await page.getByTestId("theme-option-dracula").click();

  // Applied immediately, persisted, dark class follows the appearance.
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dracula");
  const stored = await page.evaluate(() => localStorage.getItem("tasker-theme"));
  expect(stored).toBe("dracula");
  const hasDark = await page.evaluate(() =>
    document.documentElement.classList.contains("dark"),
  );
  expect(hasDark).toBe(true);

  // Reload: the inline pre-paint script re-applies the theme — assert the
  // attribute on the document element as early as the DOM is available.
  await page.reload();
  await page.waitForFunction(
    () => document.documentElement.getAttribute("data-theme") === "dracula",
  );
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dracula");
  await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

  // Clean up: back to the default so other specs are unaffected.
  await page.getByTestId("theme-switcher").click();
  await page.getByTestId("theme-option-light").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  const darkAfter = await page.evaluate(() =>
    document.documentElement.classList.contains("dark"),
  );
  expect(darkAfter).toBe(false);
});

test("a light theme never sets the legacy dark class", async ({ page }) => {
  await page.getByTestId("theme-switcher").click();
  await page.getByTestId("theme-option-solarized-light").click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "solarized-light");
  const hasDark = await page.evaluate(() =>
    document.documentElement.classList.contains("dark"),
  );
  expect(hasDark).toBe(false);
  await page.getByTestId("theme-switcher").click();
  await page.getByTestId("theme-option-light").click();
});
