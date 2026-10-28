import type { components } from "./schema";

export type User = components["schemas"]["User"];
export type Team = components["schemas"]["Team"];
export type Member = components["schemas"]["Member"];
export type Label = components["schemas"]["Label"];
export type Project = components["schemas"]["Project"];
export type Task = components["schemas"]["Task"];
export type TaskCreate = components["schemas"]["TaskCreate"];
export type TaskUpdate = components["schemas"]["TaskUpdate"];
export type Comment = components["schemas"]["Comment"];
export type Activity = components["schemas"]["Activity"];
export type Board = components["schemas"]["Board"];
export type BoardColumn = NonNullable<Board["columns"]>[number];
export type PaginatedTasks = components["schemas"]["PaginatedTasks"];
export type PaginatedMyTasks = components["schemas"]["PaginatedMyTasks"];

/** RFC 9457 problem details, as returned by every non-2xx response. */
export interface Problem {
  type?: string;
  title?: string;
  status?: number;
  detail?: string;
  errors?: { field: string; message: string }[];
}

export class ApiError extends Error {
  readonly status: number;
  readonly problem: Problem;

  constructor(status: number, problem: Problem) {
    super(problem.detail ?? problem.title ?? `Request failed (${status})`);
    this.name = "ApiError";
    this.status = status;
    this.problem = problem;
  }

  /** Field messages joined for inline form feedback. */
  fieldMessages(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const e of this.problem.errors ?? []) {
      out[e.field] = e.message;
    }
    return out;
  }
}

const BASE = "/api/v1";

async function request(
  method: string,
  path: string,
  options: { body?: unknown; query?: Record<string, string | number | boolean | undefined> } = {},
): Promise<unknown> {
  const url = new URL(BASE + path, window.location.origin);
  for (const [k, v] of Object.entries(options.query ?? {})) {
    if (v !== undefined && v !== "") url.searchParams.set(k, String(v));
  }
  const response = await fetch(url.pathname + url.search, {
    method: method.toUpperCase(),
    headers: options.body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
    credentials: "same-origin",
  });
  if (response.status === 204) return undefined;
  const contentType = response.headers.get("content-type") ?? "";
  if (!response.ok) {
    const problem = contentType.includes("json")
      ? ((await response.json()) as Problem)
      : { status: response.status, title: response.statusText };
    throw new ApiError(response.status, problem);
  }
  if (contentType.includes("json")) return (await response.json()) as unknown;
  return undefined;
}

/**
 * Contract-derived API surface. Each method maps to one OpenAPI operation;
 * payload and response types come from the generated schema.
 */
export const api = {
  // auth ---------------------------------------------------------------
  login(email: string, password: string): Promise<User> {
    return request("post", "/auth/login", { body: { email, password } }) as Promise<User>;
  },
  logout(): Promise<void> {
    return request("post", "/auth/logout") as Promise<void>;
  },
  me(): Promise<User> {
    return request("get", "/me") as Promise<User>;
  },
  changePassword(currentPassword: string, newPassword: string): Promise<void> {
    return request("put", "/me/password", {
      body: { current_password: currentPassword, new_password: newPassword },
    }) as Promise<void>;
  },

  // teams ---------------------------------------------------------------
  listTeams(): Promise<{ items: Team[] }> {
    return request("get", "/teams") as Promise<{ items: Team[] }>;
  },
  createTeam(name: string, description?: string): Promise<Team> {
    return request("post", "/teams", { body: { name, description } }) as Promise<Team>;
  },
  getTeam(teamId: string): Promise<Team> {
    return request("get", "/teams/{team_id}".replace("{team_id}", teamId)) as Promise<Team>;
  },
  updateTeam(teamId: string, body: { name?: string; description?: string }): Promise<Team> {
    return request("patch", "/teams/{team_id}".replace("{team_id}", teamId), {
      body,
    }) as Promise<Team>;
  },
  listMembers(teamId: string): Promise<{ items: Member[] }> {
    return request("get", "/teams/{team_id}/members".replace("{team_id}", teamId)) as Promise<{
      items: Member[];
    }>;
  },
  addMember(
    teamId: string,
    body: {
      email: string;
      display_name?: string;
      role: string;
      temp_password?: string;
    },
  ): Promise<Member> {
    return request("post", "/teams/{team_id}/members".replace("{team_id}", teamId), {
      body,
    }) as Promise<Member>;
  },
  changeMemberRole(teamId: string, userId: string, role: string): Promise<Member> {
    return request("patch", `/teams/${teamId}/members/${userId}`, {
      body: { role },
    }) as Promise<Member>;
  },
  removeMember(teamId: string, userId: string): Promise<void> {
    return request("delete", `/teams/${teamId}/members/${userId}`) as Promise<void>;
  },
  listLabels(teamId: string): Promise<{ items: Label[] }> {
    return request("get", "/teams/{team_id}/labels".replace("{team_id}", teamId)) as Promise<{
      items: Label[];
    }>;
  },
  createLabel(teamId: string, name: string, color?: string): Promise<Label> {
    return request("post", "/teams/{team_id}/labels".replace("{team_id}", teamId), {
      body: { name, color },
    }) as Promise<Label>;
  },

  // projects -------------------------------------------------------------
  listProjects(teamId: string, includeArchived = false): Promise<{ items: Project[] }> {
    return request("get", "/teams/{team_id}/projects".replace("{team_id}", teamId), {
      query: { include_archived: includeArchived || undefined },
    }) as Promise<{ items: Project[] }>;
  },
  createProject(teamId: string, body: { name: string; description?: string }): Promise<Project> {
    return request("post", "/teams/{team_id}/projects".replace("{team_id}", teamId), {
      body,
    }) as Promise<Project>;
  },
  getProject(projectId: string): Promise<Project> {
    return request("get", "/projects/{project_id}".replace("{project_id}", projectId)) as Promise<Project>;
  },
  updateProject(
    projectId: string,
    body: { name?: string; description?: string },
  ): Promise<Project> {
    return request("patch", "/projects/{project_id}".replace("{project_id}", projectId), {
      body,
    }) as Promise<Project>;
  },
  setProjectArchived(projectId: string, archived: boolean): Promise<Project> {
    const path = archived ? `/projects/${projectId}/archive` : `/projects/${projectId}/unarchive`;
    return request("post", path) as Promise<Project>;
  },

  // tasks ---------------------------------------------------------------
  listTasks(
    projectId: string,
    query: Record<string, string | number | undefined>,
  ): Promise<PaginatedTasks> {
    return request("get", "/projects/{project_id}/tasks".replace("{project_id}", projectId), {
      query,
    }) as Promise<PaginatedTasks>;
  },
  createTask(projectId: string, body: TaskCreate): Promise<Task> {
    return request("post", "/projects/{project_id}/tasks".replace("{project_id}", projectId), {
      body,
    }) as Promise<Task>;
  },
  getTask(taskId: string): Promise<Task> {
    return request("get", "/tasks/{task_id}".replace("{task_id}", taskId)) as Promise<Task>;
  },
  updateTask(taskId: string, body: TaskUpdate): Promise<Task> {
    return request("patch", "/tasks/{task_id}".replace("{task_id}", taskId), {
      body,
    }) as Promise<Task>;
  },
  deleteTask(taskId: string): Promise<void> {
    return request("delete", "/tasks/{task_id}".replace("{task_id}", taskId)) as Promise<void>;
  },
  createSubtask(taskId: string, body: TaskCreate): Promise<Task> {
    return request("post", "/tasks/{task_id}/subtasks".replace("{task_id}", taskId), {
      body,
    }) as Promise<Task>;
  },
  board(projectId: string, pageSize?: number): Promise<Board> {
    return request("get", "/projects/{project_id}/board".replace("{project_id}", projectId), {
      query: { page_size: pageSize },
    }) as Promise<Board>;
  },
  myTasks(query: Record<string, string | number | undefined>): Promise<PaginatedMyTasks> {
    return request("get", "/me/tasks", { query }) as Promise<PaginatedMyTasks>;
  },

  // comments & activity ---------------------------------------------------
  listComments(
    taskId: string,
    page?: number,
    pageSize?: number,
  ): Promise<{ items: Comment[]; total: number }> {
    return request("get", "/tasks/{task_id}/comments".replace("{task_id}", taskId), {
      query: { page, page_size: pageSize },
    }) as Promise<{ items: Comment[]; total: number }>;
  },
  createComment(taskId: string, body: string): Promise<Comment> {
    return request("post", "/tasks/{task_id}/comments".replace("{task_id}", taskId), {
      body: { body },
    }) as Promise<Comment>;
  },
  updateComment(commentId: string, body: string): Promise<Comment> {
    return request("patch", "/comments/{comment_id}".replace("{comment_id}", commentId), {
      body: { body },
    }) as Promise<Comment>;
  },
  deleteComment(commentId: string): Promise<void> {
    return request("delete", "/comments/{comment_id}".replace("{comment_id}", commentId)) as Promise<void>;
  },
  listActivity(taskId: string): Promise<{ items: Activity[] }> {
    return request("get", "/tasks/{task_id}/activity".replace("{task_id}", taskId)) as Promise<{
      items: Activity[];
    }>;
  },
};
