# ADR-0001: Rust + Axum for the backend

Date: 2026-10-28 · Status: accepted

## Context

A self-hosted team platform needs predictable latency under concurrent use,
strong memory-safety guarantees for a security-sensitive product, a single
static binary for painless self-hosting, and an async HTTP ecosystem with
first-class middleware support.

## Decision

Rust (edition 2024) with Tokio and Axum; tower/tower-http for middleware;
SQLx for database access.

## Consequences

- Single static binary per target; containers can be minimal and non-root.
- Axum's extractor model keeps handlers thin and typsafe; tower layers give
  body limits, timeouts, CORS without custom code.
- Cost: slower iteration than GC languages and a three-driver SQLx build —
  accepted for the operational wins.
