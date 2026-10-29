//! User entity and persistence.

use serde::Serialize;
use sqlx::Row;

use crate::db::{Args, Conn};
use crate::error::AppError;
use crate::util;

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub created_at: i64,
}

/// Reduced user shape embedded in task/comment responses.
#[derive(Debug, Clone, Serialize)]
pub struct UserBrief {
    pub id: String,
    pub display_name: String,
    pub email: String,
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
            "SELECT id, email, display_name, password_hash, created_at \
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

pub async fn find_by_id(conn: &mut Conn<'_>, id: &str) -> Result<Option<User>, AppError> {
    let mut args = Args::new();
    args.s(id);
    let row = conn
        .fetch_optional(
            "SELECT id, email, display_name, created_at FROM users WHERE id = ?",
            args,
        )
        .await?;
    row.map(|r| {
        Ok(User {
            id: r.try_get("id")?,
            email: r.try_get("email")?,
            display_name: r.try_get("display_name")?,
            created_at: r.try_get("created_at")?,
        })
    })
    .transpose()
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
        return Err(crate::db::map_unique_violation(
            e,
            "a user with this email already exists".into(),
        ));
    }
    let row = conn
        .fetch_optional(
            "SELECT id, email, display_name, created_at FROM users WHERE id = ?",
            args2,
        )
        .await?
        .ok_or_else(|| AppError::internal("user insert did not persist"))?;
    Ok(User {
        id: row.try_get("id")?,
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        created_at: row.try_get("created_at")?,
    })
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
