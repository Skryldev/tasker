import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { api } from "@/api/client";
import type * as apiClientModule from "@/api/client";
import { ToastProvider } from "@/components/ui/toast";
import { WorkspaceProvider } from "@/lib/workspace";
import { TaskDialog } from "@/components/app/TaskDialog";

// The API client is network-bound; tests replace it entirely and inspect the
// payloads the form hands over — the actual serialized request body.
vi.mock("@/api/client", async (importOriginal) => {
  const original = (await importOriginal()) as typeof apiClientModule;
  return { ...original, api: { ...original.api, createTask: vi.fn() } };
});

function renderDialog(projectId: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <WorkspaceProvider>
          <ToastProvider>
            <TaskDialog open onOpenChange={() => undefined} teamId="t1" projectId={projectId} />
          </ToastProvider>
        </WorkspaceProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("TaskDialog calendar → API boundary", () => {
  beforeEach(() => {
    vi.mocked(api.createTask).mockReset();
    vi.mocked(api.createTask).mockResolvedValue({
      id: "t-new",
      team_id: "t1",
      project_id: "p1",
      title: "x",
      status: "backlog",
      priority: "medium",
      position: 0,
      labels: [],
      created_at: 0,
      updated_at: 0,
    });
  });

  afterEach(cleanup);

  it("submitting a Persian-picked date sends the Gregorian ISO day", async () => {
    const user = userEvent.setup();
    renderDialog("p1");
    await user.type(screen.getByTestId("task-title-input"), "Calendar task");

    // Pick 15 Aban 1405 in the Doran grid (= 2026-11-06 Gregorian).
    const dueField = screen.getByLabelText("Due date") as HTMLInputElement;
    await user.clear(dueField);
    await user.type(dueField, "1405/08/15");
    await user.tab();
    expect(dueField.value).toBe("۱۴۰۵/۰۸/۱۵");

    await user.click(screen.getByTestId("task-submit"));
    await waitFor(() => expect(api.createTask).toHaveBeenCalledOnce());
    const body = vi.mocked(api.createTask).mock.calls[0]![1];
    expect(body.due_date).toBe("2026-11-06");
  });

  it("leaving the date empty keeps the field absent from the payload", async () => {
    const user = userEvent.setup();
    renderDialog("p1");
    await user.type(screen.getByTestId("task-title-input"), "No date task");
    await user.click(screen.getByTestId("task-submit"));
    await waitFor(() => expect(api.createTask).toHaveBeenCalledOnce());
    const body = vi.mocked(api.createTask).mock.calls[0]![1];
    expect(body.due_date).toBeUndefined();
  });
});
