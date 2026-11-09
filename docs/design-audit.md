# Phase 13 UI/UX Design Audit

Date: 2026-11-09 · Author: Staff engineering (agent), under the explicit
audit mandate (Phase 13 §11). Every change solves a concrete UX problem;
surfaces already at the bar are listed as unchanged with the reason.

## Method

Each page was walked against the Phase 13 design principles (restraint,
hierarchy, consistency, density, motion, accessibility, performance),
then changed only where a concrete problem existed. Bundle impact of
every addition was measured; all new surfaces are lazy-loaded.

## Changed (before → after, with the problem each change solves)

### 1. Left sidebar — collapsible (Bug 4)
- **Before**: fixed 240px column; no way to reclaim horizontal space on
  laptops; nav labels dominated small viewports.
- **After**: animated toggle (⌘/Ctrl+B or in-sidebar button) between a
  240px labeled rail and a 48px icon rail with `title` tooltips;
  `localStorage: tasker:sidebar-open` persistence; `aria-expanded` on
  the toggle; focus management (collapse → toggle, expand → first item).
- **Why**: a standard, discoverable density control; persisted so the
  choice survives reloads.
- **Motion**: width transition 200ms ease-out (compositor-cheap on a
  single column; a transform slide would require a clipping wrapper and
  clip the focus ring). Chevron rotates 200ms.

### 2. "Open full page" affordance (Bug 1)
- **Before**: buried in the three-dot overflow menu — undiscoverable.
- **After**: maximize-2 icon button in the panel header, tooltip
  "Open full page (⌘⇧O)", `aria-label="Open task as full page"`,
  ⌘/Ctrl+Shift+O shortcut, and a one-time 150ms pulse (localStorage
  flag, reduced-motion-safe).
- **Why**: the panel-to-page transition is the primary way into deep
  work; it must be visible without opening a menu.

### 3. Settings hub (Bug 5)
- **Before**: "Users" sat in the main sidebar next to My Tasks/Projects —
  instance administration mixed with daily navigation; profile lived at
  /account behind the avatar menu.
- **After**: a coherent `/settings` hub (⌘/Ctrl+, or the account menu)
  with a section rail: Account, Security (profile + 2FA + sessions via
  the existing AccountPage), Users (Owner-only). `/account` and `/users`
  redirect.
- **Why**: instance-wide administration is settings, not navigation;
  a single hub gives every account-level surface one address.

### 4. List / Board / Timeline as routes (Bug 6)
- **Before**: one page with a `?view=` tab control — no deep links, no
  history entries, one bloated component.
- **After**: `/teams/:teamId/projects/:projectId/{list,board,timeline}`
  routes; the switcher renders links (URL is the truth); filters ride the
  query string and survive switches; Back/Forward behave natively.

### 5. Timeline redesign (Bug 6)
- **Before**: grouped-by-assignee Gantt with generic primary-colored
  bars; no per-task rows, no progress, no deadlines, no dependencies; the
  whole grid re-rendered on scroll.
- **After**: per-task rows with a sticky task list (status dot, title,
  avatar); status-colored rounded bars with assignee avatar, in-bar
  title, closed-subtask progress segment; deadline diamonds; dependency
  arrows (batch-fetched, cached); day/week/month segmented zoom;
  assignee/priority/none grouping; row virtualization (fixed 44px rows →
  index math on scroll); click opens the task page; drag-reschedule with
  confirmation.
- **Why**: the timeline must carry the same information density as the
  board; progress/deadlines/dependencies make it a scheduling tool
  rather than a picture.

### 6. Guest badge (Bug 7)
- **After**: a warning-tinted "Guest" chip in the topbar whenever the
  active team context is a guest membership, with a title explaining the
  restriction.
- **Why**: restricted users must know their view is intentionally
  limited — silence reads as breakage.

### 7. Team switcher shows team avatars (Bug 8 companion)
- **Before**: initials-only tile even when a team avatar existed.
- **After**: the team's avatar (cache-busted) renders in the switcher;
  initials remain the fallback.

### 8. ProjectPage team-consistency guard (Bug 2 companion)
- A mismatched `?team=` is silently corrected to the project's own team
  (replace navigation) — the user never sees one team's frame around
  another team's data.

## Reviewed and left unchanged (with reasons)

- **Login page** — single-purpose, already minimal; visual baselines
  pin it.
- **My Tasks / Projects / Goals / Dashboard / Workload / Capacity** —
  rebuilt to the current bar in Phases 10–12 (compact rows, pastel
  chips, KPI tiles); no concrete problem found; regressions pinned by
  E2E + screenshots.
- **Task page (Phase 12.5)** — two-column layout, inline title edit,
  shared body; this phase only added the duplicate/overflow refinements
  it needed (Bug 3 panel hand-off).
- **Notifications dropdown** — Phase 12 groups/tabs/icon chips; still
  current.
- **Kanban board** — Phase 12 swimlanes, pastel chips, dashed drop
  highlight; unchanged.
- **All 16 themes** — contrast tokens were audited in Phase 10/12 and
  are unchanged; new surfaces use existing semantic tokens only
  (`bg-muted`, `text-muted-foreground`, `bg-warning/10`, status
  palette), so AA carries over.

## Remaining UX debt (documented, not scheduled)

- Long task lists in List view paginate at 25 — virtualization there
  would match the timeline's, but rows are variable-height (context
  line), so it needs measurement work.
- The command palette does not include the new Settings/section
  targets yet.
- Guest permission editor uses native `<select>` elements rather than
  the Radix Select for now (dialog-in-dialog focus order); a Radix
  pass is cosmetic follow-up.
- Mobile drawer content does not yet show the Guest badge (it renders
  in the topbar, which the drawer overlays).

## Screenshots

Visual baselines for the four highest-impact surfaces (light + dark)
are pinned by `e2e/quality.spec.ts` (login, kanban, list, task panel);
the Phase 13 changes intentionally refreshed the task-panel baseline
(new header affordance, breadcrumb) and any run of
`npx playwright test quality --update-snapshots` regenerates them.
New Phase 13 surfaces are asserted structurally in `e2e/phase13.spec.ts`
rather than pixel-pinned, since their final polish is still landing.
