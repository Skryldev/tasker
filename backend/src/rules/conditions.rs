//! Typed condition DSL for the Rules Engine.
//!
//! A condition is a bounded, declarative tree over a fixed field registry.
//! There is no evaluation of arbitrary expressions, scripts, SQL fragments,
//! or user code: the AST is closed, strictly deserialized (unknown fields are
//! rejected), typed against the existing Tasker domain enums, and evaluated
//! deterministically with no side effects.
//!
//! JSON grammar (shape-keyed, like the rest of the DSL):
//!
//! ```json
//! {"all": [ <condition>, ... ]}
//! {"any": [ <condition>, ... ]}
//! {"not": <condition>}
//! {"field": "task.status", "op": "EQ", "value": "todo"}
//! {"field": "task.status", "op": "IN", "values": ["todo", "in_progress"]}
//! ```
//!
//! Comparison semantics are total two-valued logic (no SQL-style UNKNOWN):
//!
//! - `EQ`: field equals the literal; a `null` literal matches a missing
//!   field (`EQ null` ⇔ is-null).
//! - `NEQ`: negation of `EQ` (`NEQ null` ⇔ is-not-null).
//! - `IN`: a missing field never matches (`false`); lists reject `null`
//!   entries at validation time.
//! - `NOT_IN`: negation of `IN`; a missing field is not in any list
//!   (`true`).
//!
//! There are no implicit coercions: comparing a status field against a
//! UUID-shaped value is a validation error, not a silent `false`. Equality
//! within a closed enum domain (status/priority) is canonical string
//! equality — both sides are constrained to the same `tasks::STATUSES` /
//! `tasks::PRIORITIES` vocabulary (DB CHECK constraints guarantee stored
//! values are canonical), so no case or whitespace variance is possible.
//! There are no numeric fields in the initial registry, hence no numeric
//! comparison operators.

use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::FieldError;
use crate::tasks::{PRIORITIES, STATUSES};

use super::RuleError;
use super::limits;

// ---------------------------------------------------------------------------
// Field registry
// ---------------------------------------------------------------------------

/// A condition field. The registry is closed; unknown field names are
/// rejected at validation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    TaskStatus,
    TaskPriority,
    TaskAssigneeId,
    TaskReporterId,
    EventActorId,
    EventPreviousStatus,
    EventNewStatus,
    EventPreviousPriority,
    EventNewPriority,
}

impl Field {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "task.status" => Some(Self::TaskStatus),
            "task.priority" => Some(Self::TaskPriority),
            "task.assignee_id" => Some(Self::TaskAssigneeId),
            "task.reporter_id" => Some(Self::TaskReporterId),
            "event.actor_id" => Some(Self::EventActorId),
            "event.previous_status" => Some(Self::EventPreviousStatus),
            "event.new_status" => Some(Self::EventNewStatus),
            "event.previous_priority" => Some(Self::EventPreviousPriority),
            "event.new_priority" => Some(Self::EventNewPriority),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TaskStatus => "task.status",
            Self::TaskPriority => "task.priority",
            Self::TaskAssigneeId => "task.assignee_id",
            Self::TaskReporterId => "task.reporter_id",
            Self::EventActorId => "event.actor_id",
            Self::EventPreviousStatus => "event.previous_status",
            Self::EventNewStatus => "event.new_status",
            Self::EventPreviousPriority => "event.previous_priority",
            Self::EventNewPriority => "event.new_priority",
        }
    }

    /// The value domain a field draws from; operands are type-checked
    /// against it.
    fn kind(self) -> ValueKind {
        match self {
            Self::TaskStatus | Self::EventPreviousStatus | Self::EventNewStatus => {
                ValueKind::Status
            }
            Self::TaskPriority | Self::EventPreviousPriority | Self::EventNewPriority => {
                ValueKind::Priority
            }
            Self::TaskAssigneeId | Self::TaskReporterId | Self::EventActorId => ValueKind::Uuid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Status,
    Priority,
    Uuid,
}

// ---------------------------------------------------------------------------
// Typed AST
// ---------------------------------------------------------------------------

/// A validated operand value. Enum literals hold the canonical `&'static str`
/// from the domain arrays; UUID literals are normalized (lowercase,
/// hyphenated) so comparisons are deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scalar {
    Null,
    Status(&'static str),
    Priority(&'static str),
    Uuid(String),
}

impl Scalar {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Null => None,
            Self::Status(s) | Self::Priority(s) => Some(s),
            Self::Uuid(u) => Some(u),
        }
    }
}

/// The right-hand side of a comparison: one value for `EQ`/`NEQ`, a
/// non-empty list for `IN`/`NOT_IN`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    One(Scalar),
    Many(Vec<Scalar>),
}

/// Comparison operators. No numeric operators exist because the initial
/// field registry has no numeric fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Neq,
    In,
    NotIn,
}

impl CmpOp {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "EQ",
            Self::Neq => "NEQ",
            Self::In => "IN",
            Self::NotIn => "NOT_IN",
        }
    }
}

/// A single field comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compare {
    pub field: Field,
    pub op: CmpOp,
    pub operand: Operand,
}

/// A validated condition tree. Constructed only through
/// [`Condition::from_json_str`], which enforces every structural and typing
/// limit before an instance can exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    Compare(Compare),
}

// ---------------------------------------------------------------------------
// Raw (strict) deserialization
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AllRaw {
    all: Vec<ConditionRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnyRaw {
    any: Vec<ConditionRaw>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotRaw {
    not: Box<ConditionRaw>,
}

/// Distinguishes an explicit JSON `null` operand from a missing key: serde
/// maps `null` to `None` for plain `Option<Value>` fields, which would
/// silently turn a missing operand into `EQ null`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawScalar {
    Null(()),
    Val(Value),
}

/// Deserialize the `value` key preserving explicit nulls: `Option<T>`
/// itself collapses JSON `null` to `None` before the inner type is consulted.
fn raw_scalar_from_option<'de, D>(deserializer: D) -> Result<Option<RawScalar>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(Some(match value {
        Value::Null => RawScalar::Null(()),
        other => RawScalar::Val(other),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareRaw {
    field: String,
    op: CmpOpRaw,
    #[serde(default, deserialize_with = "raw_scalar_from_option")]
    value: Option<RawScalar>,
    #[serde(default)]
    values: Option<Vec<Value>>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize)]
enum CmpOpRaw {
    #[serde(rename = "EQ")]
    Eq,
    #[serde(rename = "NEQ")]
    Neq,
    #[serde(rename = "IN")]
    In,
    #[serde(rename = "NOT_IN")]
    NotIn,
}

impl CmpOpRaw {
    fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "EQ",
            Self::Neq => "NEQ",
            Self::In => "IN",
            Self::NotIn => "NOT_IN",
        }
    }
}

/// Untagged: each variant's shape is its own discriminator; unknown keys
/// fail every variant (strict).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ConditionRaw {
    All(AllRaw),
    Any(AnyRaw),
    Not(NotRaw),
    Cmp(CompareRaw),
}

// ---------------------------------------------------------------------------
// Validation (raw -> typed)
// ---------------------------------------------------------------------------

fn find_status(s: &str) -> Option<&'static str> {
    STATUSES.iter().copied().find(|c| *c == s)
}

fn find_priority(s: &str) -> Option<&'static str> {
    PRIORITIES.iter().copied().find(|c| *c == s)
}

fn scalar_from_string(s: &str, kind: ValueKind) -> Result<Scalar, String> {
    match kind {
        ValueKind::Status => find_status(s)
            .map(Scalar::Status)
            .ok_or_else(|| format!("unknown status `{s}`; expected one of {STATUSES:?}")),
        ValueKind::Priority => find_priority(s)
            .map(Scalar::Priority)
            .ok_or_else(|| format!("unknown priority `{s}`; expected one of {PRIORITIES:?}")),
        ValueKind::Uuid => Uuid::parse_str(s)
            .map(|u| Scalar::Uuid(u.to_string()))
            .map_err(|_| format!("malformed UUID `{s}`")),
    }
}

fn scalar_from_value(raw: &Value, kind: ValueKind) -> Result<Scalar, String> {
    match raw {
        Value::Null => Ok(Scalar::Null),
        Value::String(s) => scalar_from_string(s, kind),
        _ => Err("operand must be a string or null".to_string()),
    }
}

fn build_compare(raw: &CompareRaw) -> Result<Compare, FieldError> {
    let fe = |message: String| FieldError::new("condition", message);
    let field = Field::parse(&raw.field)
        .ok_or_else(|| fe(format!("unknown condition field `{}`", raw.field)))?;
    let op_name = raw.op.as_str();
    let operand = match raw.op {
        CmpOpRaw::Eq | CmpOpRaw::Neq => {
            if raw.values.is_some() {
                return Err(fe(format!(
                    "operator {op_name} takes `value`, not `values`"
                )));
            }
            let value = raw
                .value
                .as_ref()
                .ok_or_else(|| fe(format!("operator {op_name} requires a `value` operand")))?;
            let scalar = match value {
                RawScalar::Null(()) => Scalar::Null,
                RawScalar::Val(v) => scalar_from_value(v, field.kind()).map_err(fe)?,
            };
            Operand::One(scalar)
        }
        CmpOpRaw::In | CmpOpRaw::NotIn => {
            if raw.value.is_some() {
                return Err(fe(format!(
                    "operator {op_name} takes `values`, not `value`"
                )));
            }
            let values = raw
                .values
                .as_ref()
                .ok_or_else(|| fe(format!("operator {op_name} requires a `values` operand")))?;
            if values.is_empty() {
                return Err(fe(format!(
                    "operator {op_name} requires a non-empty `values` list"
                )));
            }
            if values.len() > limits::CONDITION_IN_VALUES_MAX {
                return Err(fe(format!(
                    "`values` list exceeds the maximum of {} entries",
                    limits::CONDITION_IN_VALUES_MAX
                )));
            }
            let mut list = Vec::with_capacity(values.len());
            for v in values {
                match v {
                    Value::Null => {
                        return Err(fe("`values` entries must not be null".to_string()));
                    }
                    Value::String(s) => {
                        list.push(scalar_from_string(s, field.kind()).map_err(fe)?);
                    }
                    _ => {
                        return Err(fe("`values` entries must be strings".to_string()));
                    }
                }
            }
            Operand::Many(list)
        }
    };
    Ok(Compare {
        field,
        op: match raw.op {
            CmpOpRaw::Eq => CmpOp::Eq,
            CmpOpRaw::Neq => CmpOp::Neq,
            CmpOpRaw::In => CmpOp::In,
            CmpOpRaw::NotIn => CmpOp::NotIn,
        },
        operand,
    })
}

fn build_condition(
    raw: &ConditionRaw,
    depth: usize,
    nodes: &mut usize,
) -> Result<Condition, FieldError> {
    if depth > limits::CONDITION_MAX_DEPTH {
        return Err(FieldError::new(
            "condition",
            format!(
                "condition nesting exceeds the maximum depth of {}",
                limits::CONDITION_MAX_DEPTH
            ),
        ));
    }
    *nodes += 1;
    if *nodes > limits::CONDITION_MAX_NODES {
        return Err(FieldError::new(
            "condition",
            format!(
                "condition exceeds the maximum of {} nodes",
                limits::CONDITION_MAX_NODES
            ),
        ));
    }
    match raw {
        ConditionRaw::All(r) => {
            if r.all.is_empty() {
                return Err(FieldError::new(
                    "condition",
                    "`all` must contain at least one condition",
                ));
            }
            let inner = r
                .all
                .iter()
                .map(|c| build_condition(c, depth + 1, nodes))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Condition::All(inner))
        }
        ConditionRaw::Any(r) => {
            if r.any.is_empty() {
                return Err(FieldError::new(
                    "condition",
                    "`any` must contain at least one condition",
                ));
            }
            let inner = r
                .any
                .iter()
                .map(|c| build_condition(c, depth + 1, nodes))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Condition::Any(inner))
        }
        ConditionRaw::Not(r) => {
            let inner = build_condition(&r.not, depth + 1, nodes)?;
            Ok(Condition::Not(Box::new(inner)))
        }
        ConditionRaw::Cmp(r) => build_compare(r).map(Condition::Compare),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl Condition {
    /// Parse and fully validate a serialized condition.
    ///
    /// The serialized-size gate runs *before* parsing so oversized input can
    /// never reach the deserializer; `serde_json`'s default 128-level
    /// recursion limit additionally bounds parser stack depth for deep
    /// nesting, after which the tree limits take over.
    pub fn from_json_str(json: &str) -> Result<Self, RuleError> {
        if json.chars().count() > limits::CONDITION_JSON_MAX_CHARS {
            return Err(RuleError(vec![FieldError::new(
                "condition",
                format!(
                    "condition JSON exceeds the maximum serialized size of {} characters",
                    limits::CONDITION_JSON_MAX_CHARS
                ),
            )]));
        }
        let raw: ConditionRaw = serde_json::from_str(json)
            .map_err(|e| RuleError(vec![FieldError::new("condition", e.to_string())]))?;
        let mut nodes = 0usize;
        let condition = build_condition(&raw, 1, &mut nodes).map_err(RuleError::from)?;
        // The canonical (compact, key-ordered) form is stored at rest; it
        // must itself respect the cap. Canonical output is derived from the
        // same input that passed the gate, so this is a defensive check.
        let canonical_len = condition.to_json_str().chars().count();
        if canonical_len > limits::CONDITION_JSON_MAX_CHARS {
            return Err(RuleError(vec![FieldError::new(
                "condition",
                format!(
                    "canonical condition JSON exceeds the maximum serialized size of {} characters",
                    limits::CONDITION_JSON_MAX_CHARS
                ),
            )]));
        }
        Ok(condition)
    }

    /// Canonical JSON representation used for storage and rule versions.
    #[must_use]
    pub fn to_json_value(&self) -> Value {
        match self {
            Self::All(inner) => Value::Object(single("all", array_value(inner))),
            Self::Any(inner) => Value::Object(single("any", array_value(inner))),
            Self::Not(inner) => Value::Object(single("not", inner.to_json_value())),
            Self::Compare(c) => {
                let mut map = Map::new();
                map.insert("field".into(), Value::String(c.field.as_str().into()));
                map.insert("op".into(), Value::String(c.op.as_str().into()));
                match &c.operand {
                    Operand::One(scalar) => {
                        map.insert("value".into(), scalar_to_value(scalar));
                    }
                    Operand::Many(list) => {
                        map.insert(
                            "values".into(),
                            Value::Array(list.iter().map(scalar_to_value).collect()),
                        );
                    }
                }
                Value::Object(map)
            }
        }
    }

    #[must_use]
    pub fn to_json_str(&self) -> String {
        serde_json::to_string(&self.to_json_value())
            .expect("serializing a validated condition Value cannot fail")
    }

    /// Deterministic, side-effect-free evaluation. Infallible by design:
    /// every field/operator/value combination the validator admits has a
    /// defined boolean result (see the module docs for the semantics).
    #[must_use]
    pub fn evaluate(&self, ctx: &EvalContext<'_>) -> bool {
        match self {
            Self::All(inner) => inner.iter().all(|c| c.evaluate(ctx)),
            Self::Any(inner) => inner.iter().any(|c| c.evaluate(ctx)),
            Self::Not(inner) => !inner.evaluate(ctx),
            Self::Compare(c) => c.evaluate(ctx),
        }
    }
}

fn single(key: &str, value: Value) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert(key.into(), value);
    map
}

fn array_value(inner: &[Condition]) -> Value {
    Value::Array(inner.iter().map(Condition::to_json_value).collect())
}

fn scalar_to_value(scalar: &Scalar) -> Value {
    match scalar {
        Scalar::Null => Value::Null,
        Scalar::Status(s) | Scalar::Priority(s) => Value::String((*s).into()),
        Scalar::Uuid(u) => Value::String(u.clone()),
    }
}

impl Compare {
    fn scalar_matches(scalar: &Scalar, value: Option<&str>) -> bool {
        match scalar {
            Scalar::Null => value.is_none(),
            Scalar::Status(s) | Scalar::Priority(s) => value.is_some_and(|v| v == *s),
            Scalar::Uuid(u) => value.is_some_and(|v| v == u),
        }
    }

    fn evaluate(&self, ctx: &EvalContext<'_>) -> bool {
        let value = ctx.field_value(self.field);
        match &self.operand {
            Operand::One(scalar) => {
                let matched = Self::scalar_matches(scalar, value);
                match self.op {
                    CmpOp::Eq => matched,
                    CmpOp::Neq => !matched,
                    // IN/NOT_IN with a single-value operand is unreachable
                    // (the validator enforces the operand shape per op).
                    CmpOp::In | CmpOp::NotIn => false,
                }
            }
            Operand::Many(list) => {
                let contains = value.is_some_and(|v| {
                    list.iter()
                        .any(|scalar| scalar.as_str().is_some_and(|s| s == v))
                });
                match self.op {
                    CmpOp::In => contains,
                    CmpOp::NotIn => !contains,
                    // EQ/NEQ with a list operand is unreachable.
                    CmpOp::Eq | CmpOp::Neq => false,
                }
            }
        }
    }
}

/// The immutable facts a rule evaluation observes. All strings are canonical
/// database values; optional fields are `None` when absent (nullable column
/// or not applicable to the event's trigger type).
#[derive(Debug, Clone, Copy)]
pub struct EvalContext<'a> {
    pub task_status: &'a str,
    pub task_priority: &'a str,
    pub task_assignee_id: Option<&'a str>,
    pub task_reporter_id: Option<&'a str>,
    pub event_actor_id: Option<&'a str>,
    pub event_previous_status: Option<&'a str>,
    pub event_new_status: Option<&'a str>,
    pub event_previous_priority: Option<&'a str>,
    pub event_new_priority: Option<&'a str>,
    pub event_trigger: super::TriggerType,
    pub event_origin: super::EventOrigin,
    pub event_causing_execution_id: Option<&'a str>,
}

impl EvalContext<'_> {
    fn field_value(&self, field: Field) -> Option<&str> {
        match field {
            Field::TaskStatus => Some(self.task_status),
            Field::TaskPriority => Some(self.task_priority),
            Field::TaskAssigneeId => self.task_assignee_id,
            Field::TaskReporterId => self.task_reporter_id,
            Field::EventActorId => self.event_actor_id,
            Field::EventPreviousStatus => self.event_previous_status,
            Field::EventNewStatus => self.event_new_status,
            Field::EventPreviousPriority => self.event_previous_priority,
            Field::EventNewPriority => self.event_new_priority,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> EvalContext<'static> {
        EvalContext {
            task_status: "todo",
            task_priority: "high",
            task_assignee_id: Some("00000000-0000-0000-0000-00000000000a"),
            task_reporter_id: None,
            event_actor_id: Some("00000000-0000-0000-0000-00000000000b"),
            event_previous_status: Some("backlog"),
            event_new_status: Some("todo"),
            event_previous_priority: Some("medium"),
            event_new_priority: Some("high"),
            event_trigger: super::super::TriggerType::TaskStatusChanged,
            event_origin: super::super::EventOrigin::User,
            event_causing_execution_id: None,
        }
    }

    fn parse(json: &str) -> Condition {
        Condition::from_json_str(json).expect("valid condition")
    }

    fn leaf(field: &str, value: &str) -> String {
        format!("{{\"field\":\"{field}\",\"op\":\"EQ\",\"value\":\"{value}\"}}")
    }

    fn nested_all(depth: usize) -> String {
        let mut s = String::new();
        for _ in 0..depth {
            s.push_str("{\"all\":[");
        }
        s.push_str(&leaf("task.status", "todo"));
        for _ in 0..depth {
            s.push_str("]}");
        }
        s
    }

    // -- parsing and strictness ---------------------------------------------

    #[test]
    fn parses_simple_equality() {
        let c = parse(&leaf("task.status", "todo"));
        assert!(c.evaluate(&ctx()));
    }

    #[test]
    fn rejects_unknown_field() {
        let err = Condition::from_json_str(&leaf("task.title", "x")).unwrap_err();
        assert!(err.0[0].message.contains("unknown condition field"));
    }

    #[test]
    fn rejects_unknown_operator() {
        let json = "{\"field\":\"task.status\",\"op\":\"LIKE\",\"value\":\"todo\"}";
        assert!(Condition::from_json_str(json).is_err());
    }

    #[test]
    fn rejects_unknown_keys_in_every_variant() {
        for json in [
            "{\"field\":\"task.status\",\"op\":\"EQ\",\"value\":\"todo\",\"junk\":1}",
            "{\"all\":[],\"junk\":1}",
            "{\"any\":[],\"junk\":1}",
            "{\"not\":{\"field\":\"task.status\",\"op\":\"EQ\",\"value\":\"todo\"},\"junk\":1}",
        ] {
            assert!(Condition::from_json_str(json).is_err(), "{json}");
        }
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(Condition::from_json_str("{\"field\":").is_err());
    }

    // -- typing --------------------------------------------------------------

    #[test]
    fn rejects_status_field_with_unknown_status_value() {
        let json = leaf("task.status", "archived");
        let err = Condition::from_json_str(&json).unwrap_err();
        assert!(err.0[0].message.contains("unknown status"));
    }

    #[test]
    fn rejects_priority_field_with_unknown_priority_value() {
        // Phase 12 WS2 made "critical" a valid priority level; "catastrophic"
        // remains outside the closed vocabulary.
        let json = leaf("task.priority", "catastrophic");
        assert!(
            Condition::from_json_str(&json).unwrap_err().0[0]
                .message
                .contains("unknown priority")
        );
    }

    #[test]
    fn rejects_uuid_field_with_non_uuid_value() {
        let json = leaf("task.assignee_id", "low");
        assert!(
            Condition::from_json_str(&json).unwrap_err().0[0]
                .message
                .contains("malformed UUID")
        );
    }

    #[test]
    fn rejects_non_string_operand() {
        let json = "{\"field\":\"task.status\",\"op\":\"EQ\",\"value\":42}";
        assert!(Condition::from_json_str(json).is_err());
    }

    #[test]
    fn normalizes_uuid_literals() {
        let json = leaf("task.assignee_id", "00000000-0000-0000-0000-00000000000A");
        let c = parse(&json);
        // Canonical form lowercases; comparison against the stored id hits.
        assert!(
            c.to_json_str()
                .contains("00000000-0000-0000-0000-00000000000a")
        );
        assert!(c.evaluate(&ctx()));
    }

    #[test]
    fn rejects_operand_shape_violations() {
        for json in [
            // EQ with a list operand
            "{\"field\":\"task.status\",\"op\":\"EQ\",\"values\":[\"todo\"]}",
            // IN with a single operand
            "{\"field\":\"task.status\",\"op\":\"IN\",\"value\":\"todo\"}",
            // EQ without any operand
            "{\"field\":\"task.status\",\"op\":\"EQ\"}",
            // IN without operands
            "{\"field\":\"task.status\",\"op\":\"IN\"}",
        ] {
            assert!(Condition::from_json_str(json).is_err(), "{json}");
        }
    }

    // -- null and IN semantics ----------------------------------------------

    #[test]
    fn null_semantics_for_assignee_field() {
        let ctx = ctx();
        // assignee_id is Some in ctx, reporter_id is None.
        assert!(
            parse("{\"field\":\"task.assignee_id\",\"op\":\"NEQ\",\"value\":null}").evaluate(&ctx)
        );
        assert!(
            !parse("{\"field\":\"task.assignee_id\",\"op\":\"EQ\",\"value\":null}").evaluate(&ctx)
        );
        // Missing field: EQ null matches, NEQ null does not.
        assert!(
            parse("{\"field\":\"task.reporter_id\",\"op\":\"EQ\",\"value\":null}").evaluate(&ctx)
        );
        assert!(
            !parse("{\"field\":\"task.reporter_id\",\"op\":\"NEQ\",\"value\":null}").evaluate(&ctx)
        );
        // IN on a missing field is false; NOT_IN on a missing field is true.
        let in_list = "{\"field\":\"task.reporter_id\",\"op\":\"IN\",\"values\":[\"00000000-0000-0000-0000-00000000000a\"]}";
        let not_in_list = in_list.replace("\"IN\"", "\"NOT_IN\"");
        assert!(!parse(in_list).evaluate(&ctx));
        assert!(parse(&not_in_list).evaluate(&ctx));
    }

    #[test]
    fn in_and_not_in_over_lists() {
        let ctx = ctx();
        let json =
            "{\"field\":\"task.status\",\"op\":\"IN\",\"values\":[\"todo\",\"in_progress\"]}";
        assert!(parse(json).evaluate(&ctx));
        assert!(!parse(&json.replace("\"IN\"", "\"NOT_IN\"")).evaluate(&ctx));
    }

    #[test]
    fn rejects_null_entries_and_empty_or_oversized_lists() {
        assert!(
            Condition::from_json_str("{\"field\":\"task.status\",\"op\":\"IN\",\"values\":[]}")
                .is_err()
        );
        assert!(
            Condition::from_json_str("{\"field\":\"task.status\",\"op\":\"IN\",\"values\":[null]}")
                .is_err()
        );
        let values: Vec<String> = (0..=16).map(|_| "\"todo\"".to_string()).collect();
        let json = format!(
            "{{\"field\":\"task.status\",\"op\":\"IN\",\"values\":[{}]}}",
            values.join(",")
        );
        assert!(Condition::from_json_str(&json).is_err());
    }

    // -- structural limits ---------------------------------------------------

    #[test]
    fn enforces_depth_limit() {
        let ok = nested_all(limits::CONDITION_MAX_DEPTH - 1);
        assert!(Condition::from_json_str(&ok).is_ok());
        let too_deep = nested_all(limits::CONDITION_MAX_DEPTH);
        let err = Condition::from_json_str(&too_deep).unwrap_err();
        assert!(err.0[0].message.contains("maximum depth"));
    }

    #[test]
    fn parses_large_but_bounded_trees() {
        // The serialized-size cap (4000 chars) binds before the node cap for
        // this grammar: the smallest legal leaf is ~48 chars, so ~80 leaves
        // fit but 100 do not. The node cap remains defense-in-depth and is
        // exercised directly in enforces_node_limit.
        let leaves: Vec<String> = (0..80).map(|_| leaf("task.status", "todo")).collect();
        let json = format!("{{\"any\":[{}]}}", leaves.join(","));
        assert!(json.chars().count() <= limits::CONDITION_JSON_MAX_CHARS);
        assert!(Condition::from_json_str(&json).is_ok());
    }

    #[test]
    fn enforces_node_limit() {
        // Drive build_condition directly: 100 nodes pass, 101 fail.
        let raw_leaf = || {
            ConditionRaw::Cmp(CompareRaw {
                field: "task.status".into(),
                op: CmpOpRaw::Eq,
                value: Some(RawScalar::Val(Value::String("todo".into()))),
                values: None,
            })
        };
        let build_raw = |count: usize| {
            ConditionRaw::Any(AnyRaw {
                any: (0..count).map(|_| raw_leaf()).collect(),
            })
        };
        let mut nodes = 0usize;
        let ok = build_condition(&build_raw(limits::CONDITION_MAX_NODES - 1), 1, &mut nodes);
        assert!(ok.is_ok());
        let mut nodes = 0usize;
        let err = build_condition(&build_raw(limits::CONDITION_MAX_NODES), 1, &mut nodes)
            .expect_err("exceeds node cap");
        assert!(err.message.contains("maximum of 100 nodes"));
    }

    #[test]
    fn rejects_empty_groups() {
        for json in ["{\"all\":[]}", "{\"any\":[]}"] {
            assert!(Condition::from_json_str(json).is_err(), "{json}");
        }
    }

    #[test]
    fn oversized_input_is_rejected_before_parsing() {
        let json = format!(
            "{{\"field\":\"task.status\",\"op\":\"EQ\",\"value\":\"{}\"}}",
            "a".repeat(limits::CONDITION_JSON_MAX_CHARS)
        );
        let err = Condition::from_json_str(&json).unwrap_err();
        assert!(err.0[0].message.contains("maximum serialized size"));
    }

    // -- evaluation over the full field registry ------------------------------

    #[test]
    fn evaluates_every_registered_field() {
        let ctx = ctx();
        // (field, the context's actual value, an alternative same-kind value
        // that must not match). task.reporter_id is None in ctx and is
        // covered by the null-semantics test.
        let assignee = "00000000-0000-0000-0000-00000000000a";
        let actor = "00000000-0000-0000-0000-00000000000b";
        let cases: Vec<(&str, &str, &str)> = vec![
            ("task.status", "todo", "in_progress"),
            ("task.priority", "high", "low"),
            ("task.assignee_id", assignee, actor),
            ("event.actor_id", actor, assignee),
            ("event.previous_status", "backlog", "todo"),
            ("event.new_status", "todo", "backlog"),
            ("event.previous_priority", "medium", "high"),
            ("event.new_priority", "high", "medium"),
        ];
        for (field, hit, miss) in cases {
            assert!(parse(&leaf(field, hit)).evaluate(&ctx), "{field} EQ hit");
            assert!(!parse(&leaf(field, miss)).evaluate(&ctx), "{field} EQ miss");
        }
    }

    #[test]
    fn logical_operators_compose() {
        let ctx = ctx();
        let status_todo = leaf("task.status", "todo");
        let priority_low = leaf("task.priority", "low");
        let all = format!("{{\"all\":[{status_todo},{priority_low}]}}");
        let any = format!("{{\"any\":[{status_todo},{priority_low}]}}");
        let not = format!("{{\"not\":{priority_low}}}");
        assert!(!parse(&all).evaluate(&ctx));
        assert!(parse(&any).evaluate(&ctx));
        // ctx.priority is `high`; NOT(low) is true.
        assert!(parse(&not).evaluate(&ctx));
        let not_not = format!("{{\"not\":{not}}}");
        assert!(!parse(&not_not).evaluate(&ctx));
    }

    // -- determinism and canonical form ---------------------------------------

    #[test]
    fn evaluation_is_deterministic() {
        let ctx = ctx();
        let json = format!(
            "{{\"all\":[{},{{\"any\":[{},{}]}}]}}",
            leaf("task.status", "todo"),
            leaf("task.priority", "high"),
            leaf("task.assignee_id", "00000000-0000-0000-0000-00000000000a"),
        );
        let c = parse(&json);
        assert_eq!(c.evaluate(&ctx), c.evaluate(&ctx));
    }

    #[test]
    fn canonical_serialization_round_trips() {
        let json = format!(
            "{{\"all\":[ {{\"any\":[{},{}] }}, {{\"not\":{}}}, {} ]}}",
            leaf("task.status", "todo"),
            leaf("task.priority", "low"),
            leaf("task.reporter_id", "00000000-0000-0000-0000-00000000000c"),
            "{\"field\":\"task.status\",\"op\":\"IN\",\"values\":[\"todo\",\"in_progress\"]}",
        );
        let c = parse(&json);
        let canonical = c.to_json_str();
        let reparsed = Condition::from_json_str(&canonical).expect("canonical reparses");
        assert_eq!(c, reparsed);
        // Canonical form is compact: strictly shorter than the spaced input.
        assert!(canonical.chars().count() < json.chars().count());
    }
}
