import { useCallback } from "react";
import {
  useMutation,
  useQuery,
  useQueryClient,
  keepPreviousData,
} from "@tanstack/react-query";
import { api, ApiError, type Task, type TaskCreate, type TaskUpdate } from "./client";

export const queryKeys = {
  me: ["me"] as const,
  teams: ["teams"] as const,
  team: (id: string) => ["team", id] as const,
  members: (teamId: string) => ["members", teamId] as const,
  labels: (teamId: string) => ["labels", teamId] as const,
  projects: (teamId: string) => ["projects", teamId] as const,
  project: (id: string) => ["project", id] as const,
  tasks: (projectId: string, filters: unknown) => ["tasks", projectId, filters] as const,
  board: (projectId: string) => ["board", projectId] as const,
  task: (id: string) => ["task", id] as const,
  comments: (taskId: string) => ["comments", taskId] as const,
  activity: (taskId: string) => ["activity", taskId] as const,
  myTasks: (filters: unknown) => ["my-tasks", filters] as const,
};

export function useMe() {
  return useQuery({
    queryKey: queryKeys.me,
    queryFn: api.me,
    retry: (count, error) => error instanceof ApiError && error.status >= 500 && count < 2,
    staleTime: 60_000,
  });
}

export function useTeams() {
  return useQuery({ queryKey: queryKeys.teams, queryFn: api.listTeams });
}

export function useTeam(teamId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.team(teamId ?? ""),
    queryFn: () => api.getTeam(teamId!),
    enabled: Boolean(teamId),
  });
}

export function useMembers(teamId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.members(teamId ?? ""),
    queryFn: () => api.listMembers(teamId!),
    enabled: Boolean(teamId),
  });
}

export function useLabels(teamId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.labels(teamId ?? ""),
    queryFn: () => api.listLabels(teamId!),
    enabled: Boolean(teamId),
  });
}

export function useProjects(teamId: string | undefined, includeArchived = false) {
  return useQuery({
    queryKey: [...queryKeys.projects(teamId ?? ""), includeArchived],
    queryFn: () => api.listProjects(teamId!, includeArchived),
    enabled: Boolean(teamId),
  });
}

export function useProject(projectId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.project(projectId ?? ""),
    queryFn: () => api.getProject(projectId!),
    enabled: Boolean(projectId),
  });
}

export interface TaskFilters {
  status?: string;
  priority?: string;
  assignee_id?: string;
  label_id?: string;
  q?: string;
  sort?: string;
  order?: string;
  page?: number;
  page_size?: number;
}

function cleanFilters(f: TaskFilters): Record<string, string | number | undefined> {
  return {
    status: f.status,
    priority: f.priority,
    assignee_id: f.assignee_id,
    label_id: f.label_id,
    q: f.q,
    sort: f.sort,
    order: f.order,
    page: f.page && f.page > 1 ? f.page : undefined,
    page_size: f.page_size && f.page_size !== 25 ? f.page_size : undefined,
  };
}

export function useTasks(projectId: string | undefined, filters: TaskFilters) {
  return useQuery({
    queryKey: queryKeys.tasks(projectId ?? "", filters),
    queryFn: () => api.listTasks(projectId!, cleanFilters(filters)),
    enabled: Boolean(projectId),
    placeholderData: keepPreviousData,
  });
}

export function useBoard(projectId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.board(projectId ?? ""),
    queryFn: () => api.board(projectId!),
    enabled: Boolean(projectId),
  });
}

export function useMyTasks(filters: TaskFilters) {
  return useQuery({
    queryKey: queryKeys.myTasks(filters),
    queryFn: () => api.myTasks(cleanFilters(filters)),
    placeholderData: keepPreviousData,
  });
}

export function useTask(taskId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.task(taskId ?? ""),
    queryFn: () => api.getTask(taskId!),
    enabled: Boolean(taskId),
  });
}

export function useComments(taskId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.comments(taskId ?? ""),
    queryFn: () => api.listComments(taskId!),
    enabled: Boolean(taskId),
  });
}

export function useActivity(taskId: string | undefined) {
  return useQuery({
    queryKey: queryKeys.activity(taskId ?? ""),
    queryFn: () => api.listActivity(taskId!),
    enabled: Boolean(taskId),
  });
}

/** Invalidate every query that embeds tasks or counters. */
export function useInvalidateTasks() {
  const qc = useQueryClient();
  return useCallback(
    (projectId?: string) => {
      void qc.invalidateQueries({ queryKey: ["tasks"] });
      void qc.invalidateQueries({ queryKey: ["board"] });
      void qc.invalidateQueries({ queryKey: ["my-tasks"] });
      void qc.invalidateQueries({ queryKey: ["projects"] });
      void qc.invalidateQueries({ queryKey: ["activity"] });
      if (projectId) void qc.invalidateQueries({ queryKey: ["project", projectId] });
    },
    [qc],
  );
}

export function useCreateTask(projectId: string) {
  const invalidate = useInvalidateTasks();
  return useMutation({
    mutationFn: (body: TaskCreate) => api.createTask(projectId, body),
    onSuccess: () => invalidate(projectId),
  });
}

export function useCreateSubtask() {
  const qc = useQueryClient();
  const invalidate = useInvalidateTasks();
  return useMutation({
    mutationFn: ({ taskId, body }: { taskId: string; body: TaskCreate }) =>
      api.createSubtask(taskId, body),
    onSuccess: (_data, vars) => {
      invalidate();
      void qc.invalidateQueries({ queryKey: ["task", vars.taskId] });
    },
  });
}

export function useUpdateTask() {
  const qc = useQueryClient();
  const invalidate = useInvalidateTasks();
  return useMutation({
    mutationFn: ({ taskId, body }: { taskId: string; body: TaskUpdate }) =>
      api.updateTask(taskId, body),
    onSuccess: (task: Task) => {
      void qc.invalidateQueries({ queryKey: ["task", task.id] });
      invalidate(task.project_id);
    },
  });
}

export function useDeleteTask() {
  const invalidate = useInvalidateTasks();
  return useMutation({
    mutationFn: (taskId: string) => api.deleteTask(taskId),
    onSuccess: () => invalidate(),
  });
}

export function useCreateComment(taskId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: string) => api.createComment(taskId, body),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["comments", taskId] });
      void qc.invalidateQueries({ queryKey: ["activity", taskId] });
    },
  });
}

export function useUpdateComment(taskId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ commentId, body }: { commentId: string; body: string }) =>
      api.updateComment(commentId, body),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["comments", taskId] });
      void qc.invalidateQueries({ queryKey: ["activity", taskId] });
    },
  });
}

export function useDeleteComment(taskId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (commentId: string) => api.deleteComment(commentId),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["comments", taskId] });
      void qc.invalidateQueries({ queryKey: ["activity", taskId] });
    },
  });
}
