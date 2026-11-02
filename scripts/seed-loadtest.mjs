// Seed a dataset for the load test: admin login, team, project, N tasks.
// Usage: node scripts/seed-loadtest.mjs <base-url> <count>
const [base = "http://localhost:8095", count = "500"] = process.argv.slice(2);
const N = Number(count);

async function jf(method, path, body, cookie, attempt = 0) {
  const res = await fetch(base + path, {
    method,
    headers: {
      ...(body ? { "Content-Type": "application/json" } : {}),
      ...(cookie ? { Cookie: cookie } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  // SQLite write contention surfaces as 503; retry briefly.
  if (res.status === 503 && attempt < 20) {
    await new Promise((r) => setTimeout(r, 150 * (attempt + 1)));
    return jf(method, path, body, cookie, attempt + 1);
  }
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status}: ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : {};
}

const login = await jf("POST", "/api/v1/auth/login", {
  email: "load@tasker.local",
  password: "load-admin-pass-1",
});
const cookie = login.headers?.get?.("set-cookie");
// Node fetch cannot read Set-Cookie from .json(); do a raw login instead:
const raw = await fetch(base + "/api/v1/auth/login", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ email: "load@tasker.local", password: "load-admin-pass-1" }),
});
const setCookie = raw.headers.get("set-cookie") ?? "";
const session = setCookie.split(";")[0];

const team = await jf("POST", "/api/v1/teams", { name: "Load Team" }, session);
const project = await jf("POST", `/api/v1/teams/${team.id}/projects`, { name: "Load Project" }, session);

const statuses = ["backlog", "todo", "in_progress", "resolved", "in_review", "closed"];
const priorities = ["low", "medium", "high", "urgent"];
let created = 0;
const CONCURRENCY = 25;
async function worker(queue) {
  while (queue.length > 0) {
    const i = queue.pop();
    await jf(
      "POST",
      `/api/v1/projects/${project.id}/tasks`,
      {
        title: `Load task ${i} about searchable fixture number ${i}`,
        priority: priorities[i % priorities.length],
        status: statuses[i % statuses.length],
      },
      session,
    );
    created++;
    if (created % 100 === 0) console.log(`  seeded ${created}/${N}`);
  }
}
const queue = Array.from({ length: N }, (_, i) => i);
await Promise.all(Array.from({ length: CONCURRENCY }, () => worker(queue)));
console.log(JSON.stringify({ team: team.id, project: project.id, session }));
