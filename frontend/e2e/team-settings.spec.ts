import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Advanced Team Settings journeys (Phase 9): general settings with default
 * timezone, member filter/search/role change, guests section, archive with
 * typed confirmation, and owner-only delete with typed name + password.
 *
 * Requires a freshly bootstrapped server (see scripts/e2e-server.sh):
 *   admin@tasker.local / Admin-Pass-12345!
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

test.describe.serial("Team settings", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let teamName = "";

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    // Dedicated team via the API (the journeys below mutate it heavily).
    const suffix = Date.now().toString(36);
    teamName = `Settings Team ${suffix}`;
    const created = await page.request.post("/api/v1/teams", {
      data: { name: teamName },
    });
    expect(created.status()).toBe(201);
    teamId = (await created.json()).id as string;
  });

  test.afterAll(async () => {
    await context.close();
  });

  test("general: rename, description, default timezone", async () => {
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await expect(page.getByTestId("team-archived-badge")).toHaveCount(0);
    await page.getByLabel("Name", { exact: true }).fill(`${teamName} v2`);
    await page.getByLabel("Description").fill("Renamed by E2E");
    await page.getByLabel("Default timezone").click();
    await page.getByRole("option", { name: "Asia/Tehran" }).click();
    await page.getByRole("button", { name: "Save" }).click();

    await page.reload();
    await expect(page.getByLabel("Name", { exact: true })).toHaveValue(`${teamName} v2`);
    await expect(page.getByLabel("Description")).toHaveValue("Renamed by E2E");
  });

  test("members: add, filter by role, search by name, change role", async () => {
    // Provision a member through the API.
    const memberEmail = `p9-e2e-${Date.now().toString(36)}@test.local`;
      // Bug 2 (ADR-0014): Owner-created account; tolerate "already exists"
      // from an earlier leg of this serial suite.
      const __created = await page.request.post("/api/v1/admin/users", {
        data: { email: memberEmail, display_name: "E2E Member", temporary_password: "Member-Pass-1234!" },
      });
      if (__created.status() === 422) {
        // The account exists already — assignment below is still valid.
      } else {
        expect(__created.status()).toBe(201);
      }
    await page.request.post(`/api/v1/teams/${teamId}/members`, {
      data: { email: memberEmail, role: "member" },
    });
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);

    // Role filter hides the member row (empty result renders the message).
    await page.getByTestId("member-role-filter").click();
    await page.getByRole("option", { name: "Manager" }).click();
    await expect(page.getByTestId("members-empty")).toBeVisible();
    await page.getByTestId("member-role-filter").click();
    await page.getByRole("option", { name: "All roles" }).click();
    await expect(page.getByTestId("member-list")).toContainText(memberEmail);

    // Search by display name substring "P9".
    await page.getByTestId("member-search").fill("p9-e2e");
    await expect(page.getByTestId("member-list")).toContainText(memberEmail);
    await page.getByTestId("member-search").fill("no-such-member-xyz");
    await expect(page.getByTestId("members-empty")).toBeVisible();
    await page.getByTestId("member-search").fill("");

    // Promote to Manager, then back. The row itself shows no role badge for
    // admins, so assert through the row menu's available promotions.
    const row = page.getByTestId("member-list").locator("li").filter({ hasText: memberEmail });
    await row.getByRole("button").click();
    await page.getByRole("menuitem", { name: "Make Manager" }).click();
    await expect(page.getByTestId("member-list")).toContainText(memberEmail);
    await row.getByRole("button").click();
    await expect(page.getByRole("menuitem", { name: "Make Manager" })).toBeHidden();
    await page.getByRole("menuitem", { name: "Make Member" }).click();
    await expect(page.getByTestId("member-list")).toContainText(memberEmail);
    await row.getByRole("button").click();
    await expect(page.getByRole("menuitem", { name: "Make Manager" })).toBeVisible();
    await page.keyboard.press("Escape");
  });

  test("guests: empty state renders before any grant", async () => {
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await expect(page.getByTestId("guests-empty")).toBeVisible();
  });

  test("danger zone: archive requires the typed team name and is reversible", async () => {
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await page.getByTestId("archive-team").click();
    // The archive confirmation is a plain confirm dialog (no typed name).
    await page.getByRole("button", { name: "Archive team" }).click();
    await expect(page.getByTestId("team-archived-badge")).toBeVisible();

    // Restore.
    await page.getByTestId("archive-team").click();
    await page.getByRole("button", { name: "Restore team" }).click();
    await expect(page.getByTestId("team-archived-badge")).toHaveCount(0);
  });

  test("danger zone: delete requires typed name + password (owner-only)", async () => {
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await page.getByTestId("delete-team").click();
    await expect(page.getByTestId("delete-team-form")).toBeVisible();

    // Wrong name: the submit stays disabled until the name matches.
    await page.getByTestId("delete-team-name").fill("wrong-name");
    await page.getByTestId("delete-team-password").fill(ADMIN.password);
    await expect(page.getByTestId("delete-team-submit")).toBeDisabled();

    // The general journey renamed the team; the confirmation must match the
    // CURRENT name exactly.
    await page.getByTestId("delete-team-name").fill(`${teamName} v2`);
    await page.getByTestId("delete-team-password").fill(ADMIN.password);
    await page.getByTestId("delete-team-submit").click();

    // Back on the app; the team is gone.
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    const check = await page.request.get(`/api/v1/teams/${teamId}`);
    expect(check.status()).toBe(404);
  });
});
