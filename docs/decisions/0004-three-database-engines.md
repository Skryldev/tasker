# ADR-0004: Three supported database engines

Date: 2026-10-28 · Status: accepted

## Context

Self-hosted audiences split three ways: startups that already run PostgreSQL,
shops standardized on MySQL, and individuals who want a single-binary SQLite
deployment with no database service.

## Decision

Support all three as first-class configuration choices. PostgreSQL is the
recommended default for team deployments; SQLite is fully functional for
lightweight installs (with documented write-concurrency limits); MySQL 8.x
protocol is supported via the portable dialect.

## Consequences

- Every schema change costs three migration files; the portable-SQL ruleset
  (ADR-0003) keeps that affordable.
- Verification honesty: MySQL-protocol testing ran on MariaDB 11.4.5 (local
  availability); MySQL 8.x itself remains to be exercised — tracked in the
  implementation report.
