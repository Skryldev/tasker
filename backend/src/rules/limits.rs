//! Centralized Rules Engine limits.
//!
//! Every limit is a **character count** (Unicode scalar values), matching the
//! repo's validation convention (comment body, display name). Validators
//! measure the *serialized* form where a limit concerns serialized data, so
//! JSON escaping is included by construction. Database columns are sized in
//! migration `0009` from these constants: `MySQL` `VARCHAR(n)` reserves 4 bytes
//! per character under utf8mb4, so `n = char_limit` always fits (worst case
//! `4 × limit + 2` length-prefix bytes; assessed per table in the Task 2
//! report).

/// Maximum rule name length.
pub const RULE_NAME_MAX_CHARS: usize = 120;
/// Maximum rule description length.
pub const RULE_DESCRIPTION_MAX_CHARS: usize = 500;
/// Maximum serialized condition JSON length.
pub const CONDITION_JSON_MAX_CHARS: usize = 4_000;
/// Maximum serialized actions JSON length.
pub const ACTIONS_JSON_MAX_CHARS: usize = 2_000;
/// Maximum serialized previous/new event-state JSON length (each).
pub const EVENT_STATE_JSON_MAX_CHARS: usize = 500;
/// Maximum serialized planned-actions summary length.
pub const PLANNED_ACTIONS_JSON_MAX_CHARS: usize = 1_000;
/// Maximum serialized outbox payload length.
pub const OUTBOX_PAYLOAD_MAX_CHARS: usize = 4_000;
/// Maximum automation-rendered comment/message body length.
pub const AUTOMATION_BODY_MAX_CHARS: usize = 2_000;
/// Maximum opaque connection/destination reference length.
pub const OPAQUE_REF_MAX_CHARS: usize = 64;
/// Maximum error-category length (bounded enum names).
pub const ERROR_CATEGORY_MAX_CHARS: usize = 40;
/// Maximum sanitized error-detail length.
pub const ERROR_DETAIL_MAX_CHARS: usize = 500;
/// Maximum condition tree depth (root = depth 1).
pub const CONDITION_MAX_DEPTH: usize = 10;
/// Maximum condition tree nodes.
pub const CONDITION_MAX_NODES: usize = 100;
/// Maximum actions per rule.
pub const ACTIONS_MAX: usize = 5;
/// Maximum values in a single `IN`/`NOT_IN` operand list.
pub const CONDITION_IN_VALUES_MAX: usize = 16;
/// Maximum explicit user ids in one notification action.
pub const NOTIFICATION_USERS_MAX: usize = 10;
