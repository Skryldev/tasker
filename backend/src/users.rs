//! User entity and persistence.

use serde::Serialize;
use sqlx::Row;

use crate::db::{Args, Conn, map_unique_violation};
use crate::error::AppError;
use crate::util;

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub username: Option<String>,
    pub bio: Option<String>,
    pub timezone: Option<String>,
    /// Forward-looking capacity unit for the Capacity view (1-100, default 10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity_limit: Option<i64>,
    pub created_at: i64,
}

const USER_COLUMNS: &str =
    "id, email, display_name, username, bio, timezone, capacity_limit, created_at";

/// The default and bounds for the per-user capacity limit.
pub const CAPACITY_DEFAULT: i64 = 10;
pub const CAPACITY_MIN: i64 = 1;
pub const CAPACITY_MAX: i64 = 1000;

/// Clamp a requested capacity limit into the allowed range.
#[must_use]
pub fn clamp_capacity(raw: i64) -> i64 {
    raw.clamp(CAPACITY_MIN, CAPACITY_MAX)
}

/// Reduced user shape embedded in task/comment responses. Hover-card data
/// (username, bio) is fetched separately via the batch user-info endpoint so
/// embedded payloads stay small.
#[derive(Debug, Clone, Serialize)]
pub struct UserBrief {
    pub id: String,
    pub display_name: String,
    pub email: String,
}

fn row_to_user(row: &sqlx::any::AnyRow) -> Result<User, AppError> {
    Ok(User {
        id: row.try_get("id")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        username: row.try_get("username")?,
        bio: row.try_get("bio")?,
        timezone: row.try_get("timezone")?,
        capacity_limit: Some(
            row.try_get::<i64, _>("capacity_limit")
                .unwrap_or(CAPACITY_DEFAULT),
        ),
        created_at: row.try_get("created_at")?,
    })
}

/// Look up an **active** user by email with their password hash.
/// `is_active` is filtered in SQL rather than decoded: boolean columns have
/// different runtime types per engine (PG bool vs MySQL/SQLite integer).
/// Callers treat `None` uniformly as "no usable account for this email".
pub async fn find_active_by_email(
    conn: &mut Conn<'_>,
    email: &str,
) -> Result<Option<(User, String)>, AppError> {
    let mut args = Args::new();
    args.s(email.trim().to_lowercase());
    let row = conn
        .fetch_optional(
            "SELECT id, email, display_name, username, bio, timezone, created_at, password_hash \
             FROM users WHERE email = ? AND is_active = TRUE",
            args,
        )
        .await?;
    row.map(|r| {
        Ok((
            User {
                id: r.try_get("id")?,
                email: r.try_get("email")?,
                display_name: r.try_get("display_name")?,
                username: r.try_get("username")?,
                bio: r.try_get("bio")?,
                timezone: r.try_get("timezone")?,
                capacity_limit: Some(
                    r.try_get::<i64, _>("capacity_limit")
                        .unwrap_or(CAPACITY_DEFAULT),
                ),
                created_at: r.try_get("created_at")?,
            },
            r.try_get("password_hash")?,
        ))
    })
    .transpose()
}

/// Validate inputs and create a user (public registration and admin
/// provisioning both route through here so the rules live in one place).
pub async fn create_validated(
    conn: &mut Conn<'_>,
    email: &str,
    display_name: &str,
    password: &str,
) -> Result<User, AppError> {
    let mut field_errors = Vec::new();
    let email = email.trim().to_lowercase();
    if let Err(message) = crate::util::validate_email(&email) {
        field_errors.push(crate::error::FieldError::new("email", message));
    }
    let display_name = display_name.trim();
    if display_name.is_empty() || display_name.chars().count() > 120 {
        field_errors.push(crate::error::FieldError::new(
            "display_name",
            "display name must be 1-120 characters",
        ));
    }
    if let Err(message) = crate::security::password::validate(password) {
        field_errors.push(crate::error::FieldError::new("password", message));
    }
    if !field_errors.is_empty() {
        return Err(AppError::Validation(field_errors));
    }
    let password_hash = crate::security::password::hash(password).map_err(AppError::internal)?;
    insert(
        conn,
        &NewUser {
            email: &email,
            display_name,
            password_hash: &password_hash,
        },
    )
    .await
}

/// Active-only variant of [`find_by_id`] used by authentication paths.
pub async fn find_active_by_id(conn: &mut Conn<'_>, id: &str) -> Result<Option<User>, AppError> {
    let mut args = Args::new();
    args.s(id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ? AND is_active = TRUE"),
            args,
        )
        .await?;
    row.map(|r| row_to_user(&r)).transpose()
}

pub async fn find_by_id(conn: &mut Conn<'_>, id: &str) -> Result<Option<User>, AppError> {
    let mut args = Args::new();
    args.s(id);
    let row = conn
        .fetch_optional(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
            args,
        )
        .await?;
    row.map(|r| row_to_user(&r)).transpose()
}

/// The stored avatar data URL for a user, if any. Never serialized into
/// JSON responses; served as image bytes by `GET /users/{user_id}/avatar`.
pub async fn find_avatar(conn: &mut Conn<'_>, id: &str) -> Result<Option<String>, AppError> {
    // MySQL stores the data URL as MEDIUMTEXT, which the wire protocol
    // reports as a BLOB-family type that sqlx Any cannot decode as String
    // (the same trap migration 0008 fixed for the MFA payload columns).
    // CONVERT yields a VARCHAR on the wire (Text kind), decodable as String.
    let sql = match conn.driver() {
        crate::db::Driver::MySql => {
            "SELECT CONVERT(avatar USING utf8mb4) AS avatar FROM users WHERE id = ?"
        }
        _ => "SELECT avatar FROM users WHERE id = ?",
    };
    let mut args = Args::new();
    args.s(id);
    let row = conn.fetch_optional(sql, args).await?;
    // NULL maps to None; a *type* error propagates so storage defects cannot
    // masquerade as a missing avatar (404).
    let raw = match row {
        Some(r) => r
            .try_get::<Option<String>, _>("avatar")
            .map_err(AppError::from)?,
        None => None,
    };
    Ok(raw)
}

pub struct NewUser<'a> {
    pub email: &'a str,
    pub display_name: &'a str,
    pub password_hash: &'a str,
}

pub async fn insert(conn: &mut Conn<'_>, user: &NewUser<'_>) -> Result<User, AppError> {
    let id = util::new_id();
    let now = util::now_ms();
    let mut args = Args::new();
    args.s(&id)
        .s(user.email.trim().to_lowercase())
        .s(user.display_name)
        .s(user.password_hash)
        .b(true)
        .i(now)
        .i(now);
    let mut args2 = Args::new();
    args2.s(&id);
    let result = conn
        .execute(
            "INSERT INTO users (id, email, display_name, password_hash, is_active, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            args,
        )
        .await;
    if let Err(e) = result {
        return Err(map_unique_violation(
            e,
            "a user with this email already exists".into(),
        ));
    }
    let row = conn
        .fetch_optional(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"),
            args2,
        )
        .await?
        .ok_or_else(|| AppError::internal("user insert did not persist"))?;
    row_to_user(&row)
}

/// Fetch a batch of users for embedding into responses (bounded by caller).
pub async fn find_many_brief(
    conn: &mut Conn<'_>,
    ids: &[String],
) -> Result<std::collections::HashMap<String, UserBrief>, AppError> {
    let mut map = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!("SELECT id, email, display_name FROM users WHERE id IN ({placeholders})");
    let mut args = Args::new();
    for id in ids {
        args.s(id.clone());
    }
    for row in conn.fetch_all(&sql, args).await? {
        let id: String = row.try_get("id")?;
        map.insert(
            id,
            UserBrief {
                id: row.try_get("id")?,
                display_name: row.try_get("display_name")?,
                email: row.try_get("email")?,
            },
        );
    }
    Ok(map)
}

/// A validated profile change. `None` means "field not present in the
/// request"; `Some(None)` clears a nullable field; `Some(Some(v))` sets it.
pub struct ProfileUpdate<'a> {
    pub display_name: Option<&'a str>,
    pub username: Option<Option<String>>,
    pub bio: Option<Option<String>>,
    pub timezone: Option<Option<String>>,
    pub avatar: Option<Option<String>>,
    pub notification_prefs: Option<String>,
    pub capacity_limit: Option<i64>,
}

/// Persist a profile change. All values must be validated by the caller;
/// username uniqueness is enforced by the unique index and mapped to a 409.
pub async fn update_profile(
    conn: &mut Conn<'_>,
    user_id: &str,
    update: &ProfileUpdate<'_>,
) -> Result<(), AppError> {
    let mut sets: Vec<&str> = Vec::new();
    let mut args = Args::new();

    if let Some(name) = update.display_name {
        sets.push("display_name = ?");
        args.s(name);
    }
    if let Some(value) = &update.username {
        sets.push("username = ?");
        args.opt_s(value.clone());
    }
    if let Some(value) = &update.bio {
        sets.push("bio = ?");
        args.opt_s(value.clone());
    }
    if let Some(value) = &update.timezone {
        sets.push("timezone = ?");
        args.opt_s(value.clone());
    }
    if let Some(value) = &update.avatar {
        sets.push("avatar = ?");
        args.opt_s(value.clone());
    }
    if let Some(json) = &update.notification_prefs {
        sets.push("notification_prefs = ?");
        args.s(json.clone());
    }
    if let Some(cap) = update.capacity_limit {
        sets.push("capacity_limit = ?");
        args.i(clamp_capacity(cap));
    }
    if sets.is_empty() {
        return Ok(());
    }
    sets.push("updated_at = ?");
    args.i(util::now_ms());
    args.s(user_id);

    let sql = format!("UPDATE users SET {} WHERE id = ?", sets.join(", "));
    if let Err(e) = conn.execute(&sql, args).await {
        return Err(map_unique_violation(
            e,
            "this username is already taken".into(),
        ));
    }
    Ok(())
}

pub async fn update_password(
    conn: &mut Conn<'_>,
    user_id: &str,
    password_hash: &str,
) -> Result<(), AppError> {
    let mut args = Args::new();
    args.s(password_hash).i(util::now_ms()).s(user_id);
    conn.execute(
        "UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?",
        args,
    )
    .await?;
    Ok(())
}
