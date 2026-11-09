# ADR-0019: List / Board / Timeline are routes, not tabs

Date: 2026-11-09 · Status: accepted

## Context

Since Phase 10, the three project views lived inside one page switched by
a `?view=` tab control. Consequences:

- **No deep links.** A user could not bookmark or share "the board for
  project X with these filters" in a way that survived re-animation of
  client state; the tab click never created a history entry, so Back left
  the project entirely instead of returning to the previous view.
- **One giant component.** ProjectPage carried the state, filters, and
  markup of all three surfaces; every view change re-rendered the others'
  hooks.
- **Timeline under the tab bar** could not get its own header controls
  without colliding with the list/board toolbar layout.

## Decision

1. **Three routes, one component.**
   `/teams/:teamId/projects/:projectId/{list,board,timeline}` (and the
   flat `/projects/:projectId/{...}` variants) all render ProjectPage,
   which derives the active view from the URL **path**. `?view=` remains
   readable as a legacy alias; nothing writes it anymore.
2. **Default view: board.** The board is the primary planning surface —
   list is triage, timeline is scheduling. `/projects/:projectId` (no
   suffix) renders board.
3. **Switcher = navigation.** The segmented control renders `Link`s to
   the sibling route, carrying the current query string (team, filters,
   swimlane). History, deep links, and Back/Forward all behave natively.
4. **Filters stay in the query string.** Already true since Phase 10;
   they survive view switches because the switch preserves `location.search`.
5. **Timeline redesign (Staff-level).** Per-task rows with a sticky task
   list (avatar, title, status dot); status-colored rounded bars with the
   assignee avatar, in-bar title, and closed-subtask progress segment;
   deadline diamonds; batch-fetched dependency arrows (one cached request
   per page of tasks — never N+1 inside render); day/week/month segmented
   zoom; assignee/priority/none grouping; row virtualization (fixed row
   height → index math on scroll); click → task page; drag → reschedule
   confirmation; reduced-motion respected.

## Consequences

- `?view=` bookkeeping is gone from ProjectPage's URL writes; the view is
  a path segment like every other resource selection in the app.
- A view switch now creates a history entry (intended: Back walks
  list → board).
- The timeline's dependency fan-out is bounded: one `listDependencies`
  call per task on the current page (≤100), deduped by React Query's key
  on the id list.
