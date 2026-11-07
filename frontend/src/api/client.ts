import type { components } from "./schema";

export type User = components["schemas"]["User"];
export type MfaRequired = components["schemas"]["MfaRequired"];
export type MfaSetup = components["schemas"]["MfaSetup"];
export type RecoveryCodes = components["schemas"]["RecoveryCodes"];
export type Team = components["schemas"]["Team"];
export type Member = components["schemas"]["Member"];
export type Label = components["schemas"]["Label"];
export type Project = components["schemas"]["Project"];
export type Task = components["schemas"]["Task"];
export type TaskCreate = components["schemas"]["TaskCreate"];
export type TaskUpdate = components["schemas"]["TaskUpdate"];
export type Comment = components["schemas"]["Comment"];
export type Activity = components["schemas"]["Activity"];
export type Notification = components["schemas"]["Notification"];
export type DependencyEdge = components["schemas"]["DependencyEdge"];
export type DependencyList = components["schemas"]["DependencyList"];
export type Workload = components["schemas"]["Workload"];
export type TaskTemplate = components["schemas"]["TaskTemplate"];
export type TemplateUpsert = components["schemas"]["TemplateUpsert"];
export type ProjectGuest = components["schemas"]["ProjectGuest"];
export type Goal = components["schemas"]["Goal"];
export type GoalDetail = components["schemas"]["GoalDetail"];
export type KeyResult = components["schemas"]["KeyResult"];
export type GoalActivity = components["schemas"]["GoalActivity"];
export type GoalStatus = NonNullable<Goal["status"]>;
export type KrMetricType = NonNullable<KeyResult["metric_type"]>;
export type Resolution = NonNullable<Task["resolution"]>;
export type Board = components["schemas"]["Board"];
export type BoardColumn = NonNullable<Board["columns"]>[number];
export type PaginatedTasks = components["schemas"]["PaginatedTasks"];
export type PaginatedMyTasks = components["schemas"]["PaginatedMyTasks"];
export type NotificationPrefs = components["schemas"]["NotificationPrefs"];
export type MyWorkSummary = components["schemas"]["MyWorkSummary"];
export type SessionInfo = components["schemas"]["SessionInfo"];
export type UserInfo = components["schemas"]["UserInfo"];
export type TeamGuest = components["schemas"]["TeamGuest"];
export type CapacityResponse = components["schemas"]["CapacityResponse"];
export type CapacityMember = components["schemas"]["CapacityMember"];
export type TrashProject = components["schemas"]["TrashProject"];

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
  login(email: string, password: string): Promise<User | MfaRequired> {
    return request("post", "/auth/login", { body: { email, password } }) as Promise<
      User | MfaRequired
    >;
  },
  mfaSetup(): Promise<MfaSetup> {
    return request("post", "/auth/2fa/setup") as Promise<MfaSetup>;
  },
  mfaEnable(code: string): Promise<RecoveryCodes> {
    return request("post", "/auth/2fa/enable", { body: { code } }) as Promise<RecoveryCodes>;
  },
  mfaVerify(mfaToken: string, code: string): Promise<User> {
    return request("post", "/auth/2fa/verify", {
      body: { mfa_token: mfaToken, code },
    }) as Promise<User>;
  },
  mfaDisable(currentPassword: string): Promise<void> {
    return request("post", "/auth/2fa/disable", {
      body: { current_password: currentPassword },
    }) as Promise<void>;
  },
  mfaRecoveryCodes(currentPassword: string): Promise<RecoveryCodes> {
    return request("post", "/auth/2fa/recovery-codes", {
      body: { current_password: currentPassword },
    }) as Promise<RecoveryCodes>;
  },
  logout(): Promise<void> {
    return request("post", "/auth/logout") as Promise<void>;
  },
  me(): Promise<User> {
    return request("get", "/me") as Promise<User>;
  },
  updateProfile(body: {
    display_name?: string;
    username?: string | null;
    bio?: string | null;
    timezone?: string | null;
    avatar?: string | null;
    notification_prefs?: Partial<NotificationPrefs>;
    capacity_limit?: number;
  }): Promise<User> {
    return request("patch", "/me", { body }) as Promise<User>;
  },
  mySummary(): Promise<MyWorkSummary> {
    return request("get", "/me/summary") as Promise<MyWorkSummary>;
  },
  listSessions(): Promise<{ items: SessionInfo[] }> {
    return request("get", "/me/sessions") as Promise<{ items: SessionInfo[] }>;
  },
  revokeSession(sessionId: string): Promise<void> {
    return request("delete", `/me/sessions/${sessionId}`) as Promise<void>;
  },
  listUsers(ids: string[]): Promise<{ items: UserInfo[] }> {
    return request("get", "/users", { query: { ids: ids.join(",") } }) as Promise<{
      items: UserInfo[];
    }>;
  },
  userAvatarUrl(userId: string): string {
    return `/api/v1/users/${userId}/avatar`;
  },
  teamAvatarUrl(teamId: string): string {
    return `/api/v1/teams/${teamId}/avatar`;
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
  updateTeam(
    teamId: string,
    body: {
      name?: string;
      description?: string;
      timezone?: string | null;
      avatar?: string | null;
    },
  ): Promise<Team> {
    return request("patch", "/teams/{team_id}".replace("{team_id}", teamId), {
      body,
    }) as Promise<Team>;
  },
  archiveTeam(teamId: string): Promise<Team> {
    return request("post", `/teams/${teamId}/archive`) as Promise<Team>;
  },
  unarchiveTeam(teamId: string): Promise<Team> {
    return request("post", `/teams/${teamId}/unarchive`) as Promise<Team>;
  },
  deleteTeam(
    teamId: string,
    confirmName: string,
    currentPassword: string,
  ): Promise<void> {
    return request("delete", `/teams/${teamId}`, {
      body: { confirm_name: confirmName, current_password: currentPassword },
    }) as Promise<void>;
  },
  teamGuests(teamId: string): Promise<{ items: TeamGuest[] }> {
    return request("get", `/teams/${teamId}/guests`) as Promise<{
      items: TeamGuest[];
    }>;
  },
  revokeTeamGuest(teamId: string, userId: string): Promise<void> {
    return request("delete", `/teams/${teamId}/guests/${userId}`) as Promise<void>;
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
  deleteProject(projectId: string): Promise<void> {
    return request("delete", `/projects/${projectId}`) as Promise<void>;
  },
  restoreProject(projectId: string): Promise<Project> {
    return request("post", `/projects/${projectId}/restore`) as Promise<Project>;
  },
  purgeProject(
    projectId: string,
    confirmName: string,
    currentPassword: string,
  ): Promise<void> {
    return request("delete", `/projects/${projectId}/purge`, {
      body: { confirm_name: confirmName, current_password: currentPassword },
    }) as Promise<void>;
  },
  projectTrash(teamId: string): Promise<{ items: TrashProject[] }> {
    return request("get", `/teams/${teamId}/projects/trash`) as Promise<{
      items: TrashProject[];
    }>;
  },
  capacity(
    teamId: string,
    window: "week" | "month",
  ): Promise<CapacityResponse> {
    return request("get", `/teams/${teamId}/capacity`, {
      query: { window },
    }) as Promise<CapacityResponse>;
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
  transitionTask(taskId: string, to: string, reason?: string): Promise<Task> {
    return request("post", `/tasks/${taskId}/transition`, {
      body: reason !== undefined ? { to, reason } : { to },
    }) as Promise<Task>;
  },
  listDependencies(taskId: string): Promise<DependencyList> {
    return request("get", `/tasks/${taskId}/dependencies`) as Promise<DependencyList>;
  },
  createDependency(taskId: string, predecessorTaskId: string): Promise<DependencyEdge> {
    return request("post", `/tasks/${taskId}/dependencies`, {
      body: { predecessor_task_id: predecessorTaskId },
    }) as Promise<DependencyEdge>;
  },
  removeDependency(taskId: string, predecessorTaskId: string): Promise<void> {
    return request(
      "delete",
      `/tasks/${taskId}/dependencies/${predecessorTaskId}`,
    ) as Promise<void>;
  },
  workload(
    teamId: string,
    query: { project_id?: string; due_before?: string; due_after?: string },
  ): Promise<Workload> {
    return request("get", `/teams/${teamId}/workload`, { query }) as Promise<Workload>;
  },
  listTemplates(projectId: string): Promise<{ items: TaskTemplate[] }> {
    return request(
      "get",
      `/projects/${projectId}/templates`,
    ) as Promise<{ items: TaskTemplate[] }>;
  },
  getTemplate(templateId: string): Promise<TaskTemplate> {
    return request("get", `/templates/${templateId}`) as Promise<TaskTemplate>;
  },
  createTemplate(
    projectId: string,
    body: TemplateUpsert,
  ): Promise<TaskTemplate> {
    return request("post", `/projects/${projectId}/templates`, {
      body,
    }) as Promise<TaskTemplate>;
  },
  updateTemplate(templateId: string, body: TemplateUpsert): Promise<TaskTemplate> {
    return request("patch", `/templates/${templateId}`, { body }) as Promise<TaskTemplate>;
  },
  deleteTemplate(templateId: string): Promise<void> {
    return request("delete", `/templates/${templateId}`) as Promise<void>;
  },
  listGuests(projectId: string): Promise<{ items: ProjectGuest[] }> {
    return request(
      "get",
      `/projects/${projectId}/guests`,
    ) as Promise<{ items: ProjectGuest[] }>;
  },
  grantGuest(projectId: string, email: string): Promise<ProjectGuest> {
    return request("post", `/projects/${projectId}/guests`, {
      body: { email },
    }) as Promise<ProjectGuest>;
  },
  revokeGuest(projectId: string, userId: string): Promise<void> {
    return request("delete", `/projects/${projectId}/guests/${userId}`) as Promise<void>;
  },
  myProjects(): Promise<{ items: Project[] }> {
    return request("get", "/me/projects") as Promise<{ items: Project[] }>;
  },
  listGoals(
    teamId: string,
    status?: string,
  ): Promise<{ items: Goal[]; page: number; page_size: number; total: number }> {
    return request("get", `/teams/${teamId}/goals`, {
      query: { status: status || undefined },
    }) as Promise<{ items: Goal[]; page: number; page_size: number; total: number }>;
  },
  getGoal(goalId: string): Promise<GoalDetail> {
    return request("get", `/goals/${goalId}`) as Promise<GoalDetail>;
  },
  createGoal(
    teamId: string,
    body: {
      title: string;
      description?: string;
      owner_id?: string;
      start_date?: string;
      due_date?: string;
    },
  ): Promise<Goal> {
    return request("post", `/teams/${teamId}/goals`, { body }) as Promise<Goal>;
  },
  updateGoal(
    goalId: string,
    body: {
      title?: string;
      description?: string;
      owner_id?: string;
      start_date?: string;
      due_date?: string;
    },
  ): Promise<Goal> {
    return request("patch", `/goals/${goalId}`, { body }) as Promise<Goal>;
  },
  deleteGoal(goalId: string): Promise<void> {
    return request("delete", `/goals/${goalId}`) as Promise<void>;
  },
  transitionGoal(goalId: string, status: string): Promise<Goal> {
    return request("post", `/goals/${goalId}/status`, {
      body: { status },
    }) as Promise<Goal>;
  },
  goalActivity(goalId: string): Promise<{ items: GoalActivity[] }> {
    return request("get", `/goals/${goalId}/activity`) as Promise<{ items: GoalActivity[] }>;
  },
  listKeyResults(goalId: string): Promise<{ items: KeyResult[] }> {
    return request("get", `/goals/${goalId}/key-results`) as Promise<{ items: KeyResult[] }>;
  },
  createKeyResult(
    goalId: string,
    body: {
      title: string;
      description?: string;
      metric_type: string;
      start_value?: number;
      current_value?: number;
      target_value?: number;
      unit?: string;
    },
  ): Promise<KeyResult> {
    return request("post", `/goals/${goalId}/key-results`, { body }) as Promise<KeyResult>;
  },
  updateKeyResult(
    keyResultId: string,
    body: {
      title?: string;
      description?: string;
      metric_type?: string;
      start_value?: number;
      current_value?: number;
      target_value?: number;
      unit?: string;
    },
  ): Promise<KeyResult> {
    return request("patch", `/key-results/${keyResultId}`, { body }) as Promise<KeyResult>;
  },
  deleteKeyResult(keyResultId: string): Promise<void> {
    return request("delete", `/key-results/${keyResultId}`) as Promise<void>;
  },
  listGoalProjects(
    goalId: string,
  ): Promise<{ items: { id: string; name: string; status: string }[] }> {
    return request(
      "get",
      `/goals/${goalId}/projects`,
    ) as Promise<{ items: { id: string; name: string; status: string }[] }>;
  },
  associateProjectWithGoal(goalId: string, projectId: string): Promise<void> {
    return request("post", `/goals/${goalId}/projects`, {
      body: { project_id: projectId },
    }) as Promise<void>;
  },
  removeProjectFromGoal(goalId: string, projectId: string): Promise<void> {
    return request("delete", `/goals/${goalId}/projects/${projectId}`) as Promise<void>;
  },
  listGoalTasks(
    goalId: string,
  ): Promise<{ items: { id: string; title: string; status: string }[] }> {
    return request(
      "get",
      `/goals/${goalId}/tasks`,
    ) as Promise<{ items: { id: string; title: string; status: string }[] }>;
  },
  associateTaskWithGoal(goalId: string, taskId: string): Promise<void> {
    return request("post", `/goals/${goalId}/tasks`, {
      body: { task_id: taskId },
    }) as Promise<void>;
  },
  removeTaskFromGoal(goalId: string, taskId: string): Promise<void> {
    return request("delete", `/goals/${goalId}/tasks/${taskId}`) as Promise<void>;
  },
  board(projectId: string, pageSize?: number): Promise<Board> {
    return request("get", "/projects/{project_id}/board".replace("{project_id}", projectId), {
      query: { page_size: pageSize },
    }) as Promise<Board>;
  },
  myTasks(query: Record<string, string | number | undefined>): Promise<PaginatedMyTasks> {
    return request("get", "/me/tasks", { query }) as Promise<PaginatedMyTasks>;
  },

  // notifications --------------------------------------------------------------
  listNotifications(
    unread: boolean,
    page?: number,
    pageSize?: number,
  ): Promise<{ items: Notification[]; page: number; page_size: number; total: number }> {
    return request("get", "/notifications", {
      query: { unread: unread || undefined, page, page_size: pageSize },
    }) as Promise<{ items: Notification[]; page: number; page_size: number; total: number }>;
  },
  unreadCount(): Promise<{ count: number }> {
    return request("get", "/notifications/unread-count") as Promise<{ count: number }>;
  },
  markNotificationRead(id: string): Promise<void> {
    return request("post", `/notifications/${id}/read`) as Promise<void>;
  },
  markAllNotificationsRead(): Promise<void> {
    return request("post", "/notifications/read-all") as Promise<void>;
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
  createComment(
    taskId: string,
    body: string,
    parentCommentId?: string,
  ): Promise<Comment> {
    return request("post", "/tasks/{task_id}/comments".replace("{task_id}", taskId), {
      body: { body, parent_comment_id: parentCommentId || undefined },
    }) as Promise<Comment>;
  },
  setTaskPosition(taskId: string, beforeTaskId?: string): Promise<void> {
    return request("patch", `/tasks/${taskId}/position`, {
      body: { before_task_id: beforeTaskId || undefined },
    }) as Promise<void>;
  },
  createTaskFromTemplate(
    templateId: string,
    overrides: components["schemas"]["FromTemplateRequest"] = {},
  ): Promise<Task> {
    return request("post", `/tasks/from-template/${templateId}`, {
      body: overrides,
    }) as Promise<Task>;
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
