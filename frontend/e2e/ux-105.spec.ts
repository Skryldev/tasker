import { expect, test } from "@playwright/test";

/**
 * Phase 10.5 UX polish journeys: command palette, keyboard shortcuts, theme
 * preview hover, notification-prefs auto-save, and the 375px mobile audit.
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

// Dedicated fixtures: never assume teams.items[0] has a project (other
// specs create teams without one).
let teamId = "";
let projectId = "";

test.beforeAll(async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto("/login");
  await page.getByLabel("Email").fill(ADMIN.email);
  await page.getByLabel("Password").fill(ADMIN.password);
  await page.getByTestId("login-submit").click();
  await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  const suffix = Date.now().toString(36);
  const team = await page.request.post("/api/v1/teams", {
    data: { name: `Ux Team ${suffix}` },
  });
  teamId = (await team.json()).id as string;
  const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
    data: { name: `Ux Project ${suffix}` },
  });
  projectId = (await project.json()).id as string;
  await context.close();
});

test.beforeEach(async ({ page }) => {
  await page.goto("/login");
  await page.getByLabel("Email").fill(ADMIN.email);
  await page.getByLabel("Password").fill(ADMIN.password);
  await page.getByTestId("login-submit").click();
  await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();
});

test("command palette: open with Cmd+K, fuzzy search, navigate", async ({ page }) => {
  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByTestId("palette-input")).toBeVisible();

  // Fuzzy navigation: a subsequence of "Capacity" matches.
  await page.getByTestId("palette-input").fill("cap");
  const option = page.getByRole("option", { name: /Capacity/ });
  await expect(option).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/teams\/.+\/capacity/);
  await expect(page.getByRole("heading", { name: "Capacity" })).toBeVisible();
});

test("command palette: task search hits the server", async ({ page, browser }) => {
  // The palette searches the caller's tasks (server-side, assignee-scoped),
  // so a teammate must create the marker task with the admin as assignee.
  const memberEmail = `palette-${Date.now().toString(36)}@test.local`;
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
  const members = await page.request.get(`/api/v1/teams/${teamId}/members`);
  const adminId = (
    (await members.json()).items as { user: { id: string; email: string }; role: string }[]
  ).find((m) => m.role === "owner")!.user.id;
  const firstProject = { id: projectId };

  const memberContext = await browser.newContext();
  const mp = await memberContext.newPage();
  await mp.goto("/login");
  await mp.getByLabel("Email").fill(memberEmail);
  await mp.getByLabel("Password").fill("Member-Pass-1234!");
  await mp.getByTestId("login-submit").click();
  await expect(mp.getByRole("heading", { name: "My Tasks" })).toBeVisible();
  const marker = `palette ${Date.now().toString(36)}`;
  const created = await mp.request.post(`/api/v1/projects/${firstProject.id}/tasks`, {
    data: { title: marker, assignee_id: adminId },
  });
  expect(created.status()).toBe(201);
  await memberContext.close();

  await page.keyboard.press("ControlOrMeta+k");
  await page.getByTestId("palette-input").fill(marker);
  await expect(page.getByRole("option", { name: marker })).toBeVisible({ timeout: 10_000 });
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(new RegExp(`/tasks/`));
});

test("keyboard shortcuts: C, /, G+T, ?", async ({ page }) => {
  // "C" on a project page opens the create-task dialog. The shortcuts
  // manager is code-split (Phase 12.5 bundle gate): wait for its trigger to
  // replace the placeholder before sending keys.
  await page.goto(`/projects/${projectId}?team=${teamId}`);
  await expect(page.getByRole("button", { name: "Keyboard shortcuts" })).toBeEnabled();
  await page.keyboard.press("c");
  await expect(page.getByTestId("task-title-input")).toBeVisible();
  await page.keyboard.press("Escape");

  // "/" focuses the My Tasks search (wait for the lazy page chunk first).
  await page.goto("/");
  await expect(page.getByTestId("task-search")).toBeVisible();
  await page.keyboard.press("/");
  await expect(page.getByTestId("task-search")).toBeFocused();

  // "G then T" navigates to My Tasks from anywhere (workspace defaults the
  // team once the shell mounts).
  await page.goto("/account");
  // Wait for the shell (and its lazy key listener) to mount before the
  // sequence.
  await expect(page.getByRole("heading", { name: "Profile" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Keyboard shortcuts" })).toBeEnabled();
  await page.keyboard.press("g");
  await page.keyboard.press("t");
  await expect(page).toHaveURL(/\/$|\/\?team=/);
  await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

  // "?" opens the cheatsheet.
  await page.keyboard.press("?");
  await expect(page.getByTestId("shortcuts-list")).toBeVisible();
  await page.keyboard.press("Escape");

  // Shortcuts never fire while typing.
  await page.getByTestId("task-search").click();
  await page.keyboard.type("c");
  await expect(page.getByTestId("task-search")).toBeFocused();
  await expect(page.getByTestId("task-search")).toHaveValue("c");
});

test("theme preview: hover applies, leaving restores, click commits", async ({ page }) => {
  await page.getByTestId("theme-switcher").click();
  const html = page.locator("html");
  await expect(html).toHaveAttribute("data-theme", /light|dark/);
  const before = await html.getAttribute("data-theme");

  // Hover dracula: live preview without persisting.
  await page.getByTestId("theme-option-dracula").hover();
  await expect(html).toHaveAttribute("data-theme", "dracula");
  expect(await page.evaluate(() => localStorage.getItem("tasker-theme"))).toBe(before);

  // Move away: restore the committed theme.
  await page.getByTestId("theme-option-light").hover();
  await expect(html).toHaveAttribute("data-theme", before ?? "light");

  // Click dracula: commits and persists.
  await page.getByTestId("theme-option-dracula").click();
  await expect(html).toHaveAttribute("data-theme", "dracula");
  expect(await page.evaluate(() => localStorage.getItem("tasker-theme"))).toBe("dracula");

  // Back to light for the rest of the run.
  await page.getByTestId("theme-switcher").click();
  await page.getByTestId("theme-option-light").click();
  await expect(html).toHaveAttribute("data-theme", "light");
});

test("notification preferences auto-save with a Saved flash", async ({ page }) => {
  await page.goto("/account");
  const toggle = page.getByTestId("pref-task-rejected");
  const initial = (await toggle.getAttribute("aria-checked")) === "true";
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", String(!initial));
  await expect(page.getByTestId("prefs-saved")).toHaveAttribute("style", /opacity: 1/, {
    timeout: 5_000,
  });

  // Persisted: a reload reflects the new value.
  await page.reload();
  await expect(page.getByTestId("pref-task-rejected")).toHaveAttribute(
    "aria-checked",
    String(!initial),
    { timeout: 10_000 },
  );
  // Restore.
  await page.getByTestId("pref-task-rejected").click();
  await expect(page.getByTestId("pref-task-rejected")).toHaveAttribute(
    "aria-checked",
    String(initial),
  );
});

test("mobile 375px renders the five key screens without overflow", async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 812 });
  const overflowAt = async (label: string) => {
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
    );
    expect(overflow, `${label} horizontal overflow`).toBeLessThanOrEqual(1);
  };

  await page.goto("/");
  await overflowAt("my tasks");
  await page.goto("/account");
  await overflowAt("account");
  await page.goto(`/projects/${projectId}?team=${teamId}`);
  await overflowAt("project list view");
  await page.goto(`/projects/${projectId}?team=${teamId}&view=board`);
  await overflowAt("kanban board");
  await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
  await overflowAt("team settings");
});
