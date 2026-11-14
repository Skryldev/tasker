import { expect, test, type BrowserContext, type Page } from "@playwright/test";

/**
 * Phase 14 E2E: deep permission model, priority guidance, notification
 * action center & dependency engine.
 *   WS1  — deadline picker: opens on click + keyboard, renders above the
 *          dialog, picks a date, inline deadline >= due date validation.
 *   WS3  — priority dropdown: bold name + muted bilingual description.
 *   WS4  — role badge on every member row.
 *   WS2  — granular guest grants: board-only guest, drawer toggles,
 *          invite-request queue (ADR-0021).
 *   WS5  — notification action center: tabs with counts, search, pinned
 *          section, quick actions (ADR-0022).
 *   WS6  — dependency engine: searchable picker with type chips, edge
 *          cards, graph modal, server-enforced close-with-blockers
 *          override (ADR-0023).
 */

const ADMIN = { email: "admin@tasker.local", password: "Admin-Pass-12345!" };

test.describe.serial("Phase 14", () => {
  let context: BrowserContext;
  let page: Page;
  let teamId = "";
  let projectId = "";
  const suffix = Date.now().toString(36);

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext();
    page = await context.newPage();
    await page.goto("/login");
    await page.getByLabel("Email").fill(ADMIN.email);
    await page.getByLabel("Password").fill(ADMIN.password);
    await page.getByTestId("login-submit").click();
    await expect(page.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    const team = await page.request.post("/api/v1/teams", {
      data: { name: `P14 Team ${suffix}` },
    });
    teamId = ((await team.json()) as { id: string }).id;
    const project = await page.request.post(`/api/v1/teams/${teamId}/projects`, {
      data: { name: `P14 Project ${suffix}` },
    });
    projectId = ((await project.json()) as { id: string }).id;
  });

  test.afterAll(async () => {
    await context.close();
  });

  // -- WS1 + WS3: the New Task dialog ---------------------------------------
  test("deadline picker opens above the dialog, on click and keyboard; inline validation", async () => {
    test.setTimeout(120_000);
    await page.goto(`/projects/${projectId}?team=${teamId}&view=board`);
    await page.getByTestId("create-task-board").first().click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();

    // Pick a day in a Doran field: open via the icon button, click the
    // Nth in-month day, then make sure no popover lingers (a stale grid
    // would swallow the next field's pick).
    const pickDay = async (fieldId: string, nth: number) => {
      const field = dialog.locator(`#${fieldId}`);
      const popovers = page.locator(".doran-datepicker__popover");
      // A click during the dialog's open animation can be swallowed —
      // retry once if the popover did not appear.
      await field.locator(".doran-datepicker__icon").click();
      try {
        await expect(popovers).toHaveCount(1, { timeout: 2000 });
      } catch {
        await field.locator(".doran-datepicker__icon").click();
        await expect(popovers).toHaveCount(1);
      }
      const day = popovers
        .locator(".doran-day:not(.doran-day--outside)")
        .nth(nth);
      await day.click();
      // A successful pick closes the popover; if it is still open the pick
      // was swallowed by the dialog animation — click again.
      try {
        await expect(popovers).toHaveCount(0, { timeout: 2000 });
      } catch {
        await day.click();
        await expect(popovers).toHaveCount(0);
      }
      await expect(field.locator(".doran-datepicker__control")).toHaveValue(
        /^\p{Nd}{4}\/\p{Nd}{2}\/\p{Nd}{2}$/u,
      );
      if ((await popovers.count()) > 0) {
        await dialog.getByRole("heading").first().click();
        await expect(popovers).toHaveCount(0);
      }
    };

    // Click path: the calendar popover renders inside the dialog's portal
    // container (never clipped behind it).
    const dlField = dialog.locator("#task-deadline");
    const dlInput = dlField.locator(".doran-datepicker__control");
    await dlField.locator(".doran-datepicker__icon").click();
    const grid = page.locator(".doran-calendar").first();
    await expect(grid).toBeVisible();
    const gridBox = await grid.boundingBox();
    const triggerBox = await dlField.boundingBox();
    expect(gridBox).not.toBeNull();
    expect(triggerBox).not.toBeNull();
    // The popover overlaps the field (above it in z-order) rather than
    // being pushed behind the dialog.
    expect(gridBox!.y < triggerBox!.y + triggerBox!.height).toBe(true);

    // Pick the 5th day of the displayed month; the input carries the value
    // (Doran displays Jalali dates in Persian digits; the canonical
    // Gregorian value reaches the API through onChange).
    await pickDay("task-deadline", 4);

    // Keyboard path: focus + ArrowDown reopens the grid; clicking the
    // dialog heading (a neutral area) closes it — Escape would bubble to
    // the Radix dialog itself and close everything.
    await dlInput.focus();
    await page.keyboard.press("ArrowDown");
    await expect(grid).toBeVisible();
    await dialog.getByRole("heading").first().click();
    await expect(grid).toBeHidden();

    // WS3: the priority dropdown describes every level bilingually, always
    // visible in the row (icon + bold name + muted fa — en description).
    await dialog.getByTestId("task-title-input").fill("P14 deadline task");
    await dialog.locator("#task-priority").click();
    const listbox = page.getByRole("listbox");
    await expect(listbox).toBeVisible();
    await expect(listbox).toContainText("مهم");
    await expect(listbox).toContainText("High");
    // Escape closes only the Radix Select — the dialog stays open.
    await page.keyboard.press("Escape");
    await expect(listbox).toBeHidden();

    // Submit with only a deadline (no due date): the dialog closes and the
    // task carries a deadline.
    await page.getByTestId("task-submit").click();
    await expect(dialog).toBeHidden();
    const created = (await (
      await page.request.get(`/api/v1/projects/${projectId}/tasks`)
    ).json()) as { items: { title: string; deadline: string | null }[] };
    const made = created.items.find((t) => t.title === "P14 deadline task");
    expect(made, "deadline-only task created").toBeTruthy();
    expect(made!.deadline).toMatch(/^\d{4}-\d{2}-\d{2}$/);

    // Inline validation: a second dialog with due day 10 and deadline day 5
    // is rejected inline; moving the deadline past it submits.
    await expect(page.getByTestId("board-view")).toContainText("P14 deadline task");
    await page.getByTestId("create-task-board").first().click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId("task-title-input")).toBeVisible();
    await dialog.getByTestId("task-title-input").fill("P14 invalid deadline");
    await pickDay("task-due", 9);
    await pickDay("task-deadline", 4);
    await page.getByTestId("task-submit").click();
    await expect(
      dialog.getByText("The deadline must be on or after the due date"),
    ).toBeVisible();
    await pickDay("task-deadline", 11);
    await page.getByTestId("task-submit").click();
    await expect(dialog).toBeHidden();
  });

  // -- WS4: role badges ------------------------------------------------------
  test("every member row shows a role badge", async () => {
    await page.goto(`/teams/${teamId}/settings`);
    await expect(page.getByTestId("member-list")).toBeVisible();
    const rows = page.getByTestId("member-list").locator("> li");
    const rowCount = await rows.count();
    expect(rowCount).toBeGreaterThanOrEqual(1);
    for (let i = 0; i < rowCount; i++) {
      await expect(rows.nth(i).locator("[data-testid^=role-badge-]")).toBeVisible();
    }
    await expect(page.getByTestId("role-badge-owner").first()).toBeVisible();
  });

  // -- WS2: granular guest grants (ADR-0021) ---------------------------------
  test("board-only guest: drawer toggles, restricted surfaces, invite-request queue", async () => {
    const guestEmail = `guest14-${suffix}@test.local`;
    const guestPw = "Guest-Pass-1234!";
    const created = await page.request.post("/api/v1/admin/users", {
      data: { email: guestEmail, display_name: "P14 Guest", temporary_password: guestPw },
    });
    expect(created.status()).toBe(201);

    await page.goto(`/teams/${teamId}/settings`);
    await page.getByTestId("invite-guest").click();
    await page.getByTestId("guest-search-input").fill(guestEmail);
    await page.getByTestId("guest-eligible-list").getByRole("option").first().click();

    const guests = (await (
      await page.request.get(`/api/v1/teams/${teamId}/guests2`)
    ).json()) as { items: { guest_id: string; user: { email: string } }[] };
    const guestId = guests.items.find((g) => g.user.email === guestEmail)!.guest_id;

    // The redesigned drawer: domain accordions with grant summary.
    await page.getByTestId(`edit-permissions-${guestId}`).click();
    await expect(page.getByTestId("permission-drawer")).toBeVisible();
    await expect(page.getByTestId("perm-summary")).toContainText("0 grants");
    await expect(page.getByTestId("perm-section-tasks")).toBeVisible();
    await page.getByTestId(`perm-toggle-${projectId}-view_project_board`).click();
    await expect(page.getByTestId("perm-summary")).toContainText("1 grant");
    await page.getByTestId("perm-save").click();
    await expect(page.getByTestId("permission-drawer")).toBeHidden();

    // The guest sees the board, and the badge list groups it by domain.
    await expect(page.getByTestId(`guest-badges-${guestId}`)).toContainText("See the board");

    const guestContext = await context.browser()!.newContext();
    const gp = await guestContext.newPage();
    await gp.goto("/login");
    await gp.getByLabel("Email").fill(guestEmail);
    await gp.getByLabel("Password").fill(guestPw);
    await gp.getByTestId("login-submit").click();
    await expect(gp.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    // Board: visible for a guest with view_project_board.
    await gp.goto(`/projects/${projectId}?team=${teamId}&view=board`);
    await expect(gp.getByTestId("board-view")).toBeVisible();

    // Invite-request queue: the guest files a request for a colleague via
    // the API (invite_guest grant); the manager sees and approves it.
    await page.request.post(`/api/v1/teams/${teamId}/guests2/${guestId}/permissions`, {
      data: { permission: "invite_guest", scope_type: "team" },
    });
    const helperEmail = `helper14-${suffix}@test.local`;
    const helper = await page.request.post("/api/v1/admin/users", {
      data: { email: helperEmail, display_name: "P14 Helper", temporary_password: "Helper-Pass-1234!" },
    });
    expect(helper.status()).toBe(201);
    // Filed through the GUEST session — only invite_guest holders may ask.
    const filed = await gp.request.post(`/api/v1/teams/${teamId}/guests/invite-requests`, {
      data: { email: helperEmail },
    });
    expect(filed.status()).toBe(201);

    await page.goto(`/teams/${teamId}/settings`);
    const queue = page.getByTestId("invite-requests-list");
    await expect(queue).toBeVisible();
    await expect(queue).toContainText(helperEmail);
    const approveButton = page.locator("[data-testid^=approve-invite-request-]").first();
    await approveButton.click();
    await expect(queue).toContainText("Approved");
    // The helper is now a guest of the team (no grants yet — default deny).
    const guestsAfter = (await (
      await page.request.get(`/api/v1/teams/${teamId}/guests2`)
    ).json()) as { items: { user: { email: string } }[] };
    expect(
      guestsAfter.items.some((g) => g.user.email === helperEmail),
      "approved request created the guest registration",
    ).toBe(true);

    await guestContext.close();
  });

  // -- WS5: notification action center (ADR-0022) ----------------------------
  test("action center: tabs with counts, search, pinned section, quick actions", async () => {
    // Seed two workers and generate notifications for one of them.
    const workerEmail = `worker14-${suffix}@test.local`;
    const workerPw = "Worker-Pass-1234!";
    const created = await page.request.post("/api/v1/admin/users", {
      data: { email: workerEmail, display_name: "P14 Worker", temporary_password: workerPw },
    });
    expect(created.status()).toBe(201);
    const worker = (await (
      await page.request.get(`/api/v1/admin/users?q=${encodeURIComponent(workerEmail)}`)
    ).json()) as { items: { id: string; email: string }[] };
    const workerId = worker.items.find((u) => u.email === workerEmail)!.id;
    // Assignment and mentions require team membership.
    const joined = await page.request.post(`/api/v1/teams/${teamId}/members`, {
      data: { email: workerEmail, role: "member" },
    });
    expect(joined.status()).toBe(201);

    const t1 = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: "P14 notify task" },
    });
    const t1Id = ((await t1.json()) as { id: string }).id;

    // Assignment notification.
    const assigned = await page.request.patch(`/api/v1/tasks/${t1Id}`, {
      data: { assignee_id: workerId },
    });
    expect(assigned.status()).toBe(200);
    // Mention notification.
    const commented = await page.request.post(`/api/v1/tasks/${t1Id}/comments`, {
      data: { body: `ping @${workerEmail} please review`, guest_visible: false },
    });
    expect(commented.status()).toBe(201);

    const workerContext = await context.browser()!.newContext();
    const wp = await workerContext.newPage();
    await wp.goto("/login");
    await wp.getByLabel("Email").fill(workerEmail);
    await wp.getByLabel("Password").fill(workerPw);
    await wp.getByTestId("login-submit").click();
    await expect(wp.getByRole("heading", { name: "My Tasks" })).toBeVisible();

    // Pin the first notification via the API before opening the bell.
    const notes = (await (
      await wp.request.get("/api/v1/notifications")
    ).json()) as { items: { id: string; event: string }[] };
    expect(notes.items.length).toBeGreaterThanOrEqual(2);
    await wp.request.post(`/api/v1/notifications/${notes.items[0].id}/pin`, {
      data: { pinned: true },
    });

    await wp.getByTestId("notification-bell").click();
    // Tabs with counts: mentions tab carries the mention.
    await expect(wp.getByTestId("notification-tab-mentions")).toContainText("1");
    // Pinned section floats above the time groups.
    await expect(wp.getByTestId("notification-list")).toContainText("Pinned");
    await expect(wp.getByTestId("notification-list")).toContainText("Today");
    // Search narrows the list.
    await wp.getByTestId("notification-search").fill("mentioned");
    await expect(wp.getByTestId("notification-list")).toContainText("mentioned you on");
    await expect(wp.getByTestId("notification-list")).not.toContainText("assigned you a task");
    // Quick action on a mention: Reply deep-links to the discussion anchor.
    const quick = wp.locator("[data-testid^=quick-]").first();
    await quick.click();
    await expect(wp).toHaveURL(new RegExp(`/tasks/${t1Id}`));

    await workerContext.close();
  });

  // -- WS6: dependency engine (ADR-0023) --------------------------------------
  test("dependencies: typed picker, edge cards, graph modal, close-with-blockers override", async () => {
    const a = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: `P14 dep A ${suffix}`, due_date: "2026-03-01" },
    });
    const aId = ((await a.json()) as { id: string }).id;
    const b = await page.request.post(`/api/v1/projects/${projectId}/tasks`, {
      data: { title: `P14 dep B ${suffix}`, due_date: "2026-02-01" },
    });
    const bId = ((await b.json()) as { id: string }).id;

    // Push A through the workflow to in_review so the close journey runs.
    // Backlog->todo and submit are assignee-only, and the admin (creator =
    // reporter) cannot self-assign — the worker from the WS5 test is a
    // member and takes the assignee seat.
    const workerEmail = `worker14-${suffix}@test.local`;
    const members = (await (
      await page.request.get(`/api/v1/teams/${teamId}/members`)
    ).json()) as { items: { user: { id: string; email: string } }[] };
    const workerId = members.items.find((m) => m.user.email === workerEmail)!.user.id;
    const workerContext = await context.browser()!.newContext();
    const wp2 = await workerContext.newPage();
    await wp2.goto("/login");
    await wp2.getByLabel("Email").fill(workerEmail);
    await wp2.getByLabel("Password").fill("Worker-Pass-1234!");
    await wp2.getByTestId("login-submit").click();
    await expect(wp2.getByRole("heading", { name: "My Tasks" })).toBeVisible();
    // The admin (creator) can edit the task; the worker then drives the
    // assignee-only transitions.
    const assign = await page.request.patch(`/api/v1/tasks/${aId}`, {
      data: { assignee_id: workerId },
    });
    expect(assign.status()).toBe(200);
    // Assignee moves it through resolved; only the reporter (admin, the
    // creator) submits the resolved work for review.
    for (const to of ["todo", "in_progress", "resolved"]) {
      const r = await wp2.request.post(`/api/v1/tasks/${aId}/transition`, {
        data: { to },
      });
      expect(r.status()).toBe(200);
    }
    await workerContext.close();
    const submit = await page.request.post(`/api/v1/tasks/${aId}/transition`, {
      data: { to: "in_review" },
    });
    expect(submit.status()).toBe(200);

    await page.goto(`/tasks/${aId}`);
    await expect(page.getByTestId("dependencies-panel")).toBeVisible();

    // Searchable command-menu picker with type chips; auto-schedule opt-in.
    await page.getByTestId("add-dependency").click();
    await page.getByTestId("dependency-task-picker-search").fill(`P14 dep B`);
    await page.getByRole("option").filter({ hasText: `P14 dep B ${suffix}` }).click();
    await page.getByTestId("dep-type-blocks").click();
    await expect(page.getByTestId("auto-schedule-toggle")).toBeVisible();
    await page.getByTestId("auto-schedule-toggle").check();
    await page.getByTestId("dependency-submit").click();

    // The edge card appears with the type badge.
    const card = page.getByTestId(`dep-${bId}`);
    await expect(card).toBeVisible();
    await expect(card).toContainText("Blocks");

    // Graph modal: the blocks subgraph renders as SVG.
    await page.getByTestId("show-dependency-graph").click();
    await expect(page.getByTestId("dependency-graph-modal")).toBeVisible();
    await expect(page.getByTestId("dependency-graph-svg")).toBeVisible();
    await page.getByRole("button", { name: "Close graph" }).click();

    // Close-with-blockers: A is blocked by open B → 409 → override modal.
    await page.getByTestId("accept-task").click();
    await page.getByTestId("confirm-accept").click();
    const blockersDialog = page.getByTestId("blockers-override-form");
    await expect(blockersDialog).toBeVisible();
    // Empty reason is rejected inline.
    await page.getByTestId("blockers-override-submit").click();
    await expect(page.getByText("An override reason is required")).toBeVisible();
    await page.getByTestId("blockers-reason-input").fill("Dependency is tracked separately; shipping this now.");
    await page.getByTestId("blockers-override-submit").click();
    await expect(page.getByTestId("status-control")).toHaveAttribute("data-status", "closed");

    // The audit trail recorded the override (task activity).
    const activity = (await (
      await page.request.get(`/api/v1/tasks/${aId}/activity`)
    ).json()) as { items: { event: string }[] };
    expect(
      activity.items.some((i) => i.event === "task_status_changed"),
      "close recorded in activity",
    ).toBe(true);
  });
});
