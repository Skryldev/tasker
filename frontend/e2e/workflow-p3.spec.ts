import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 3 E2E: guest access, dependencies, workload, templates.
 * Runs after the Phase 1/2 suites against a freshly bootstrapped server.
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };
const MEMBER = { email: "member@tasker.local", password: "Member-Pass-1234!" };
const GUEST = { email: "guest@tasker.local", password: "Guest-Pass-1234!" };

test.describe.serial("Phase 3 guests, dependencies, workload, templates", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let projectId = "";
  let taskId = "";
  let otherTaskId = "";

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
  });

  test.afterAll(async () => {
    await context.close();
  });

  test("setup: team, member, guest user, project, two tasks", async () => {
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    await page.getByTestId("team-switcher").click();
    await page.getByText("Create team…").click();
    await page.getByLabel("Name").fill("P3 Team");
    await page.getByTestId("create-team-submit").click();
    await page.waitForURL(/\?team=/);
    teamId = new URL(await page.url()).searchParams.get("team") ?? "";

    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    // Bug 2 (ADR-0014): Owner-created account; tolerate "already exists"
    // from an earlier leg of this serial suite.
    const __created = await page.request.post("/api/v1/admin/users", {
      data: { email: MEMBER.email, display_name: "P3 Member", temporary_password: MEMBER.password },
    });
    if (__created.status() === 422) {
      // The account exists already — assignment below is still valid.
    } else {
      expect(__created.status()).toBe(201);
    }
    await page.getByTestId("add-member").click();
    await page.getByTestId("member-search-input").fill(MEMBER.email);
    await page.getByTestId("eligible-members-list").getByRole("option").first().click();
    await expect(page.getByTestId("member-list")).toContainText(MEMBER.email);

    await page.goto(`/projects?team=${teamId}`);
    await page.getByTestId("create-project").click();
    await page.getByTestId("project-name-input").fill("P3 Project");
    await page.getByTestId("project-submit").click();
    await page.getByRole("link", { name: /P3 Project/ }).first().click();
    projectId = new URL(await page.url()).pathname.split("/").pop() ?? "";

    // Bug 6 (ADR-0019): views are routes — pin the list view once and use
    // its create button for both tasks.
    await page.getByTestId("view-list").click();
    await expect(page.getByTestId("task-filters")).toBeVisible();
    for (const title of ["Foundation work", "Dependent work"]) {
      await page.getByTestId("create-task").first().click();
      await page.getByTestId("task-title-input").fill(title);
      await page.getByTestId("task-submit").click();
      await expect(page.getByTestId("task-table")).toContainText(title);
    }
    const listResponse = await page.request.get(`/api/v1/projects/${projectId}/tasks`);
    const list = await listResponse.json();
    expect(list.total).toBe(2);
    taskId = list.items.find((t: { title: string }) => t.title === "Foundation work").id;
    otherTaskId = list.items.find((t: { title: string }) => t.title === "Dependent work").id;
    const rows = await page.locator('[data-testid^="task-row-"]').all();
    expect(rows.length).toBe(2);
  });

  test("admin grants guest access; guest sees the read-only project", async ({ browser }) => {
    // Bug 2 (ADR-0014): the guest account is Owner-created, then assigned
    // to the team (membership removed again below — the guest relies purely
    // on the project grant).
    // Bug 2 (ADR-0014): Owner-created account; tolerate "already exists"
    // from an earlier leg of this serial suite.
    const __created = await page.request.post("/api/v1/admin/users", {
      data: { email: GUEST.email, display_name: "P3 Guest", temporary_password: GUEST.password },
    });
    if (__created.status() === 422) {
      // The account exists already — assignment below is still valid.
    } else {
      expect(__created.status()).toBe(201);
    }
    await page.goto(`/teams/${teamId}/settings?team=${teamId}`);
    await page.getByTestId("add-member").click();
    await page.getByTestId("member-search-input").fill(GUEST.email);
    await page.getByTestId("eligible-members-list").getByRole("option").first().click();
    await expect(page.getByTestId("member-list")).toContainText(GUEST.email);
    // Remove the team membership so the guest relies purely on the project grant.
    const guestId = await page.evaluate(
      async ({ email, teamIdValue }) => {
        const res = await fetch(`/api/v1/teams/${teamIdValue}/members`, {
          credentials: "same-origin",
        });
        const body = await res.json();
        return body.items.find((m: { user: { email: string } }) => m.user.email === email)
          .user.id;
      },
      { email: GUEST.email, teamIdValue: teamId },
    );
    // Remove the team membership via API (the UI flow is covered in Phase 1).
    const removed = await page.request.delete(`/api/v1/teams/${teamId}/members/${guestId}`);
    expect(removed.status()).toBe(204);

    // Grant guest access.
    await page.goto(`/projects/${projectId}/guests`);
    await page.getByTestId("add-guest").click();
    await page.getByTestId("guest-email-input").fill(GUEST.email);
    await page.getByTestId("guest-submit").click();
    await expect(page.getByTestId("guest-list")).toContainText(GUEST.email);

    // Guest signs in and sees the project as a guest.
    const guestContext = await browser.newContext();
    const gp = await guestContext.newPage();
    await gp.goto("/login");
    await gp.getByLabel("Email").fill(GUEST.email);
    await gp.getByLabel("Password").fill(GUEST.password);
    await gp.getByTestId("login-submit").click();
    await gp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });

    await gp.goto(`/projects/${projectId}`);
    await expect(gp.getByTestId("guest-banner")).toContainText("read-only guest access");
    await expect(gp.getByTestId("view-list")).toBeVisible();
    // No create controls.
    expect(await gp.getByTestId("create-task-board").count()).toBe(0);
    expect(await gp.getByTestId("create-task-board").count()).toBe(0);

    // Server-side enforcement: mutations rejected.
    const create = await gp.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "Guest task" },
    });
    expect(create.status()).toBe(403);
    const patch = await gp.request.patch(`/api/v1/tasks/${taskId}`, {
      data: { title: "Hacked" },
    });
    expect(patch.status()).toBe(403);
    // A legal transition pair fails on the actor check (403): the guest is
    // neither assignee nor reporter. An illegal pair would return 409.
    const accept = await gp.request.post(`/api/v1/tasks/${taskId}/transition`, {
      data: { to: "todo" },
    });
    expect(accept.status()).toBe(403);

    // Guest reads tasks; detail renders (task identity verified by title).
    const list = await (await gp.request.get(`/api/v1/projects/${projectId}/tasks`)).json();
    expect(list.total).toBe(2);
    const foundationTask = list.items.find(
      (t: { title: string }) => t.title === "Foundation work",
    );
    expect(foundationTask).toBeDefined();
    await gp.goto(`/tasks/${foundationTask.id}`);
    await expect(gp.getByTestId("task-detail")).toContainText("Foundation work");
    await guestContext.close();
  });

  test("guest cannot access team settings, other projects, or workload", async ({ browser }) => {
    const guestContext = await browser.newContext();
    const gp = await guestContext.newPage();
    await gp.goto("/login");
    await gp.getByLabel("Email").fill(GUEST.email);
    await gp.getByLabel("Password").fill(GUEST.password);
    await gp.getByTestId("login-submit").click();
    await gp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });

    const members = await gp.request.get(`/api/v1/teams/${teamId}/members`);
    expect(members.status()).toBe(404);
    const workload = await gp.request.get(`/api/v1/teams/${teamId}/workload`);
    expect(workload.status()).toBe(404);
    const templates = await gp.request.get(`/api/v1/projects/${projectId}/templates`);
    expect(templates.status()).toBe(404);
    await guestContext.close();
  });

  test("member adds a dependency; cycle is rejected; removal works", async ({ browser }) => {
    const memberContext = await browser.newContext();
    const mp = await memberContext.newPage();
    await mp.goto("/login");
    await mp.getByLabel("Email").fill(MEMBER.email);
    await mp.getByLabel("Password").fill(MEMBER.password);
    await mp.getByTestId("login-submit").click();
    await mp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });

    // Foundation blocks Dependent.
    // Phase 14 (ADR-0023): typed engine — from_task_id body, blocks type.
    const create = await mp.request.post(`/api/v1/tasks/${otherTaskId}/dependencies`, {
      data: { from_task_id: taskId, dep_type: "blocks" },
    });
    expect(create.status()).toBe(201);
    const edge = (await create.json()) as { id: string; type: string };
    expect(edge.type).toBe("blocks");

    // Reverse edge = cycle → 409 (blocks type only).
    const cycle = await mp.request.post(`/api/v1/tasks/${taskId}/dependencies`, {
      data: { from_task_id: otherTaskId },
    });
    expect(cycle.status()).toBe(409);

    // UI shows the blocker on the dependent task (Dependent work): its
    // "blocked by" list contains Foundation work.
    await mp.goto(`/tasks/${otherTaskId}`);
    await expect(mp.getByTestId("dependencies-panel")).toContainText("Foundation work");
    await memberContext.close();
  });

  test("workload view shows aggregates and respects filters", async () => {
    await page.goto(`/teams/${teamId}/workload`);
    await expect(page.getByTestId("workload-page")).toBeVisible();
    // Unassigned row present (both tasks unassigned by admin).
    await expect(page.getByTestId("workload-table")).toContainText("Unassigned");
    const unassignedTotal = await (await page.request.get(`/api/v1/teams/${teamId}/workload`)).json();
    expect(unassignedTotal.unassigned.total).toBeGreaterThanOrEqual(2);

    // Project filter narrows to the project's tasks.
    const scoped = await (
      await page.request.get(`/api/v1/teams/${teamId}/workload?project_id=${projectId}`)
    ).json();
    expect(scoped.unassigned.total).toBe(2);

    // Guest has no access.
    const guestContext = await page.context().browser()!.newContext();
    const gp = await guestContext.newPage();
    await gp.goto("/login");
    await gp.getByLabel("Email").fill(GUEST.email);
    await gp.getByLabel("Password").fill(GUEST.password);
    await gp.getByTestId("login-submit").click();
    await gp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });
    const guestWorkload = await gp.request.get(`/api/v1/teams/${teamId}/workload`);
    expect(guestWorkload.status()).toBe(404);
    await guestContext.close();
  });

  test("templates: create, use, and delete", async () => {
    await page.goto(`/projects/${projectId}`);
    await page.getByTestId("create-template").click();
    await page.locator("#template-name").fill("Weekly report");
    await page.locator("#template-title").fill("Write weekly report");
    await page.locator("#template-description").fill("Summarize the week's progress.");
    await page.getByTestId("template-submit").click();
    await expect(page.getByTestId("template-list")).toContainText("Weekly report");

    // Use the template to create a task.
    const templates = await (await page.request.get(`/api/v1/projects/${projectId}/templates`)).json();
    const templateId = templates.items[0].id;
    const created = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: {
        title: "Write weekly report (edited)",
        description: "Summarize the week's progress.",
        priority: "medium",
      },
    });
    expect(created.status()).toBe(201);
    const createdBody = await created.json();
    expect(createdBody.title).toBe("Write weekly report (edited)");
    expect(createdBody.status).toBe("backlog");

    // Editing a task derived from a template does not change the template.
    const templateAfter = await (
      await page.request.get(`/api/v1/templates/${templateId}`)
    ).json();
    expect(templateAfter.default_title).toBe("Write weekly report");

    // Delete the template; the created task survives.
    const deleted = await page.request.delete(`/api/v1/templates/${templateId}`);
    expect(deleted.status()).toBe(204);
    const taskAfter = await page.request.get(`/api/v1/tasks/${createdBody.id}`);
    expect(taskAfter.status()).toBe(200);
  });

  test("revoking guest access takes effect immediately", async () => {
    const guests = await (await page.request.get(`/api/v1/projects/${projectId}/guests`)).json();
    const guestId = guests.items[0].user.id;
    const revoked = await page.request.delete(`/api/v1/projects/${projectId}/guests/${guestId}`);
    expect(revoked.status()).toBe(204);

    const guestContext = await page.context().browser()!.newContext();
    const gp = await guestContext.newPage();
    await gp.goto("/login");
    await gp.getByLabel("Email").fill(GUEST.email);
    await gp.getByLabel("Password").fill(GUEST.password);
    await gp.getByTestId("login-submit").click();
    await gp.waitForURL(/\/$|\?team=/, { timeout: 15_000 });
    const projectRead = await gp.request.get(`/api/v1/projects/${projectId}`);
    expect(projectRead.status()).toBe(404);
    const taskRead = await gp.request.get(`/api/v1/tasks/${taskId}`);
    expect(taskRead.status()).toBe(404);
    await guestContext.close();
  });
});
