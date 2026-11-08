# ADR-0015: The task detail is a page, the panel is a quick view

Date: 2026-11-08 · Status: accepted

## Context

Since Phase 10.5 the task detail lived in a right-hand panel inside the
project board and My Tasks. For a task-management product the detail
surface is the primary workspace: the panel squeezed it to ~380 px,
metadata competed with content for vertical space, deep work (long
descriptions, threaded comments, checklists, attachments) required
scrolling inside a drawer, and the surface could not hold a stable URL or
browser history entry of its own.

## Decision

1. **Dedicated page.** `GET /teams/:teamId/projects/:projectId/tasks/:taskId`
   renders a Staff-level two-column page (content + metadata rail on
   desktop, stacked on mobile) with a sticky breadcrumb header
   (Team → Project → Task), inline title editing, the Phase 10.5 status
   control, and an overflow menu (edit fields, duplicate, copy link,
   delete). The legacy `/tasks/:taskId` route (notifications, copy-link)
   resolves team/project from the task and renders the same page.
2. **One shared body, two shells.** `TaskDetailBody` (description,
   resolution, subtasks, dependencies, checklist, attachments, threaded
   comments, activity) and `MetadataBand` (reporter/assignee/priority/due/
   deadline/labels chips) are exported components used verbatim by both
   the panel and the page. Behavior cannot drift between surfaces because
   there is only one implementation.
3. **The panel stays** as the fast navigation surface, gaining an
   **"Open full page"** action (also the title click on the page). This
   preserves the board's quick-triage flow while giving depth a real home.
4. **Navigation.** Back (`navigate(-1)`) returns to the originating
   view (list/board/timeline) — view state lives in the URL (`?view=`),
   so history-back restores it exactly.
5. **The page is a separate lazy chunk** (already-lazy route), sharing
   the detail components with the panel chunk; no eager-bundle impact.

## Consequences

- Deep links to tasks now carry team/project context; copy-link URLs are
  canonical.
- The panel and page must both keep their WS10 closed-task presentation —
  guaranteed by sharing the body, which takes `readOnly` once.
- Duplicate-task semantics: a copy is created in the same project with
  creation-legal status (closed/review states map to `todo`), and the user
  is navigated to it.
