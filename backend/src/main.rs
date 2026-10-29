//! Tasker backend entry point.
//!
//! Subcommands:
//! - `serve` (default): run the HTTP server.
//! - `bootstrap-admin`: create the first administrator account (see docs/deployment.md).

use std::io::Write as _;

use clap::{Parser, Subcommand};
use sqlx::Row;
use tasker::config::Config;
use tasker::db::Database;
use tasker::error::AppError;
use tasker::http::build_router;
use tasker::security::password;
use tasker::state::AppState;
use tasker::users;
use tasker::util;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(
    name = "tasker",
    version,
    about = "Self-hosted team work management platform"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server (default).
    Serve,
    /// Create the initial administrator account and team-owner membership.
    /// Idempotent: refuses cleanly when an administrator already exists.
    /// Credentials come from --email/--password, `TASKER_ADMIN_EMAIL`/
    /// `TASKER_ADMIN_PASSWORD`, or interactive prompts; never from defaults.
    BootstrapAdmin {
        #[arg(long, env = "TASKER_ADMIN_EMAIL")]
        email: Option<String>,
        #[arg(long, env = "TASKER_ADMIN_PASSWORD", hide_env_values = true)]
        password: Option<String>,
        /// Display name (defaults to the email local part).
        #[arg(long, env = "TASKER_ADMIN_DISPLAY_NAME")]
        display_name: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing();

    let config = Config::from_env().unwrap_or_else(|e| {
        eprintln!("configuration error: {e}");
        std::process::exit(2);
    });

    let code = match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => run_server(config).await,
        Command::BootstrapAdmin {
            email,
            password,
            display_name,
        } => run_bootstrap(config, email, password, display_name).await,
    };
    std::process::exit(code);
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn"));
    let json_format =
        std::env::var("TASKER_LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("json"));
    let registry = tracing_subscriber::registry().with(filter);
    if json_format {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
}

async fn run_server(config: Config) -> i32 {
    tracing::info!(
        environment = if config.environment.is_production() {
            "production"
        } else {
            "development"
        },
        "starting tasker"
    );

    let db = match Database::connect(&config).await {
        Ok(db) => db,
        Err(e) => {
            tracing::error!(error = %e, "database initialization failed");
            eprintln!("database initialization failed: {e}");
            return 1;
        }
    };

    tracing::info!(driver = db.driver().name(), "running database migrations");
    if let Err(e) = db.migrate().await {
        tracing::error!(error = %e, "migration failed");
        eprintln!("database migration failed: {e}");
        return 1;
    }

    let state = AppState::new(db, config.clone());
    let router = build_router(state.clone());
    let addr = format!("{}:{}", config.http_host, config.http_port);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, %addr, "cannot bind listener");
            eprintln!("cannot bind {addr}: {e}");
            return 1;
        }
    };
    tracing::info!(%addr, "tasker listening");

    let app_state = state.clone();
    let shutdown = async move {
        wait_for_shutdown().await;
        tracing::info!("shutdown signal received; draining connections");
        // Give in-flight requests a bounded grace period via axum's graceful
        // shutdown, then close the pool.
        let _ = &app_state;
    };

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await;

    // Close database resources cleanly after the server stops accepting.
    state.db.close().await;

    match result {
        Ok(()) => {
            tracing::info!("server stopped cleanly");
            0
        }
        Err(e) => {
            tracing::error!(error = %e, "server error");
            1
        }
    }
}

async fn wait_for_shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

async fn run_bootstrap(
    config: Config,
    email_arg: Option<String>,
    password_arg: Option<String>,
    display_name_arg: Option<String>,
) -> i32 {
    let db = match Database::connect(&config).await {
        Ok(db) => db,
        Err(e) => {
            eprintln!("database initialization failed: {e}");
            return 1;
        }
    };
    if let Err(e) = db.migrate().await {
        eprintln!("database migration failed: {e}");
        return 1;
    }

    let email = match email_arg {
        Some(e) => e,
        None => match prompt("Administrator email: ") {
            Ok(e) => e,
            Err(_) => return 2,
        },
    };
    if let Err(message) = util::validate_email(&email) {
        eprintln!("invalid email: {message}");
        return 2;
    }

    let password = match password_arg {
        Some(p) => p,
        None => match rpassword::prompt_password("Administrator password: ") {
            Ok(p) => p,
            Err(_) => return 2,
        },
    };
    if let Err(message) = password::validate(&password) {
        eprintln!("invalid password: {message}");
        return 2;
    }

    let display_name = display_name_arg.unwrap_or_else(|| {
        email
            .split('@')
            .next()
            .unwrap_or("Administrator")
            .to_string()
    });

    let mut conn = match db.begin().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cannot open database transaction: {e}");
            return 1;
        }
    };

    // Bootstrap is first-setup only: refuse as soon as any user exists
    // (members are provisioned through team settings afterwards).
    let user_count = match count_users(&mut conn).await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("cannot check existing users: {e}");
            return 1;
        }
    };
    if user_count > 0 {
        eprintln!(
            "users already exist; bootstrap refuses to run again. \
             Add members from team settings instead."
        );
        return 3;
    }
    if user_count_check_by_email(&mut conn, &email).await {
        eprintln!("a user with this email already exists; bootstrap cannot proceed");
        return 3;
    }

    let password_hash = match password::hash(&password) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("password hashing failed: {e}");
            return 1;
        }
    };

    let user = match users::insert(
        &mut conn,
        &users::NewUser {
            email: &email,
            display_name: &display_name,
            password_hash: &password_hash,
        },
    )
    .await
    {
        Ok(u) => u,
        Err(AppError::Conflict(m)) => {
            eprintln!("cannot create administrator: {m}");
            return 3;
        }
        Err(e) => {
            eprintln!("cannot create administrator: {e}");
            return 1;
        }
    };
    if let Err(e) = conn.commit().await {
        eprintln!("cannot commit administrator creation: {e}");
        return 1;
    }

    println!("Administrator created:");
    println!("  id:           {}", user.id);
    println!("  email:        {}", user.email);
    println!("  display name: {}", user.display_name);
    println!();
    println!("Next steps: log in, create a team, and invite members from team settings.");
    0
}

fn prompt(message: &str) -> Result<String, std::io::Error> {
    print!("{message}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

async fn user_count_check_by_email(conn: &mut tasker::db::Conn<'_>, email: &str) -> bool {
    let mut args = tasker::db::Args::new();
    args.s(email.trim().to_lowercase());
    conn.fetch_optional("SELECT id FROM users WHERE email = ?", args)
        .await
        .is_ok_and(|r| r.is_some())
}

async fn count_users(conn: &mut tasker::db::Conn<'_>) -> Result<i64, AppError> {
    let row = conn
        .fetch_optional("SELECT COUNT(*) AS n FROM users", tasker::db::Args::new())
        .await?;
    Ok(row.map_or(0, |r| r.try_get::<i64, _>("n").unwrap_or(0)))
}
