# ADR-0022: Notification action center

Date: 2026-11-14 · Status: accepted · Extends
[ADR-0008](0008-acceptance-workflow-notifications.md)

## Context

The notification dropdown (Phase 12 WS12) was a flat reverse-chronological
feed with an All/Unread toggle. As volume grows, bursts (an assignee
working through a task) drown the signal, there is no way to keep a
notification visible across sessions, and triaging mentions versus review
requests means eyeballing every row.

## Decision

1. **Schema (migration 0021).** `notifications` gains `pinned` (boolean,
   default false), `pinned_at`, and `group_key` (nullable text, reserved
   for server-side batching — see §5). An index on `(recipient_id,
   pinned)` backs the pinned section.
2. **Pinning.** `POST /notifications/{id}/pin` toggles (or sets, with
   `{"pinned": bool}`) pin state for the recipient only. Pinned rows sort
   first in the list (`ORDER BY pinned DESC, pinned_at DESC, …`) and
   render in a **Pinned** section above the time groups. `?pinned=true`
   fetches only pinned rows.
3. **Tabs with counts.** `GET /notifications/counts` returns per-tab
   `{total, unread}` for `all`, `mentions` (`comment_mention`),
   `assigned` (`task_assigned`), and `activity` (every other task event).
   The dropdown renders All / Unread / Mentions / Assigned / Task Activity
   tabs with unread chips; Mentions/Assigned/Activity filter the loaded
   set client-side via the same event mapping.
4. **Search.** A client-side query filters loaded rows on summary and
   actor name. With pagination the search covers loaded pages only; the
   Load more control extends coverage (kept client-side to avoid a
   server-side LIKE over actor-joined text on every keystroke).
5. **Client-side smart batching.** Consecutive notifications from the
   same actor on the same task within **5 minutes** collapse into one
   card with a "+N" chip and a "Show N more" expander. Batching is
   recomputed within each time group so bursts never span headers.
   `group_key` exists so a future server-side batcher can precompute the
   key without another migration.
6. **Time groups.** Today / Yesterday / This Week / Earlier.
7. **Per-type identity.** Each event type has an icon + accent color,
   drawn as a 3px left color bar and a tinted icon chip — the same hue
   consistently marks a type across light and dark themes.
8. **Quick actions.** Every row carries Reply (mentions → task page
   `?focus=comments`), Reschedule (→ `?focus=dates`), View Task, and
   Pin/Unpin. The task page scrolls to the anchored section and flashes a
   ring, so the action lands the user on the exact surface.
9. **Load more.** 50 per page with an explicit Load more control; the
   eager shell (bell + badge) is unchanged and the action center stays
   code-split.

## Consequences

- No notification is ever hidden by batching — expanders reveal every
  follower row.
- Counts are server-computed so tab chips stay honest beyond the loaded
  page.
- Pin state is per recipient and never affects other users of the task.
- The list contract gains `pinned`/`pinned_at` fields; the generated
  schema regenerates with the Phase 14 OpenAPI pass.
