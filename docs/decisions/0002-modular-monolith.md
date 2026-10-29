# ADR-0002: Modular monolith deployment

Date: 2026-10-28 · Status: accepted

## Context

Phase 1 scope (teams → projects → tasks → comments/activity) is one bounded
context family with shared transactions. Startup self-hosters want one
container and one database, not an orchestration exercise.

## Decision

A single backend binary with strict internal module boundaries (transport →
services → portable persistence layer). No message bus, no microservices,
no DI framework.

## Consequences

- Transactional consistency across modules is trivial (one database
  transaction, e.g. task mutation + activity record).
- Module boundaries are enforced by import discipline rather than process
  walls; if a future phase needs extraction (e.g. search service), the
  service functions and portable query layer are the seams.
- Scaling is vertical + replica-count; horizontal write scaling is an
  explicit non-goal for Phase 1.
