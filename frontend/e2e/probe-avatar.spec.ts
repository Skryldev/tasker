import { test, expect } from "@playwright/test";

test("probe avatar img on task page", async ({ page }) => {
  page.on("response", (res) => {
    if (res.url().includes("/avatar")) {
      console.log("AVATAR:", res.status(), res.url().slice(0, 90));
    }
  });
  await page.goto("/login");
  await page.getByLabel("Email").fill("admin@tasker.local");
  await page.getByLabel("Password").fill("Admin-Pass-12345!");
  await page.getByTestId("login-submit").click();
  await page.waitForURL(/^http:\/\/localhost:8091\/$/);

  const team = await page.request.post("/api/v1/teams", { data: { name: `Av ${Date.now()}` } });
  const teamId = ((await team.json()) as { id: string }).id;
  const proj = await page.request.post(`/api/v1/teams/${teamId}/projects`, { data: { name: "Av proj" } });
  const projId = ((await proj.json()) as { id: string }).id;
  const task = await page.request.post(`/api/v1/projects/${projId}/tasks`, { data: { title: "Av task" } });
  const taskId = ((await task.json()) as { id: string }).id;

  const png =
    "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAAMElEQVR4nO3BMQEAAADCoPVLb0o/8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA4GvkQEAAT4FA6oAAAAASUVORK5CYII=";
  await page.goto("/account");
  await page.locator("#avatar-upload").setInputFiles({
    name: "a.png",
    mimeType: "image/png",
    buffer: Buffer.from(png, "base64"),
  });
  const accountImg = await page.evaluate(() =>
    document.querySelector('img[src*="/users/"]')?.getAttribute("src") ?? "none",
  );
  console.log("ACCOUNT-IMG:", accountImg);

  await page.goto(`/teams/${teamId}/projects/${projId}/tasks/${taskId}`);
  await page.getByTestId("task-metadata").waitFor();
  const railHtml = await page.evaluate(() => {
    const rail = document.querySelector('[data-testid="task-page-rail"]');
    return rail ? rail.innerHTML.slice(0, 600) : "no rail";
  });
  console.log("RAIL-HTML:", railHtml);
});
