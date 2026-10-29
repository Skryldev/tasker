# ADR-0006: Centralized capability matrix, no repository traits

Date: 2026-10-28 · Status: accepted

## Context

Authorization bugs cluster where role checks are scattered, and premature
trait abstraction (a generic `Repository<T>`) tends to add indirection
without removing SQL.

## Decision

- One `Role` enum (Member < Manager < Admin < Owner) with a single
  `can(Capability)` matrix in `authorization.rs`, unit-tested exhaustively.
  Services call `require_capability`; handlers never compare roles.
  Rank-based rules (owner-grant, last-owner protection) live inside the
  teams service transactions.
- Persistence uses the concrete `db::Conn` handle directly. No repository
  trait layer exists; SQL lives beside the module that owns the entity.

## Consequences

- Adding a capability is a one-place change with a test.
- Replacing the persistence layer (unlikely for a monolith) would be a
  broader refactor — accepted, since a speculative abstraction would be
  paid every day while the payoff is hypothetical.
- Team-owned resources 404 (not 403) for outsiders to avoid existence
  leaks; this convention is encoded in the services and asserted by tests.
