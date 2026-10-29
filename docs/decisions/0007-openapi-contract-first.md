# ADR-0007: OpenAPI 3.1 contract-first with generated TypeScript types

Date: 2026-10-28 · Status: accepted

## Context

The frontend and backend must not drift. Hand-maintained duplicate types
always rot; a generated client keeps the contract authoritative.

## Decision

`api/openapi.yaml` is written first and is authoritative. From it:
`openapi.json` (served by the backend) and frontend TypeScript types via
`openapi-typescript`. The frontend API client is a thin typed wrapper over
fetch that maps problems to `ApiError`. CI lints the document (Redocly) and
behavioral suites assert representative success/validation/authn/authz
responses against the documented shapes.

## Rejected alternatives

- Full client generation (e.g. `@hey-api/openapi-ts`): heavier generated
  runtime; the thin hand-written wrapper over generated types gives the same
  type safety with less machinery. Revisit if endpoint count grows.
- Rust-first codegen (utoipa): would make Rust types authoritative and the
  contract generated — weaker as a cross-team source of truth for Phase 1.

## Consequences

- Contract changes are deliberate: edit YAML → regenerate JSON + types →
  implement.
- Type safety covers payloads/responses; path strings in the client are a
  thin convention layer checked by the E2E suite.
