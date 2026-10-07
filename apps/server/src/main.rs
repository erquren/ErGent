mod auth;
mod login_guard;
mod origin;
mod routes;
mod storage;
mod transport;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use clap::{Parser, Subcommand};
use ergent_protocol::{AgentCommand, Event, SessionInfo};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use std::{collections::HashMap, io::Read, path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, watch, Mutex};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = ".local/server.db", global = true)]
    database: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Read the new password from stdin, not from the command line.
    AdminCreate {
        #[arg(long, default_value = "admin")]
        username: String,
    },
    /// Clear the persistent login failure count for an IP.
    LoginUnblock { ip: std::net::IpAddr },
    Run {
        #[arg(long, default_value = "127.0.0.1:7777")]
        listen: std::net::SocketAddr,
        #[arg(long, default_value = "http://127.0.0.1:7777")]
        origin: String,
        /// Additional exact browser origins; no wildcard matching.
        #[arg(long)]
        additional_origin: Vec<String>,
        /// Exact proxy peer IPs allowed to supply an overwritten X-Real-IP header.
        #[arg(long)]
        trusted_proxy: Vec<std::net::IpAddr>,
        /// Allow HTTP on explicitly configured private IPv4 LAN origins.
        #[arg(long)]
        allow_insecure_lan: bool,
        /// Development opt-in: accept browser requests from any Origin.
        #[arg(long)]
        allow_any_origin: bool,
        #[arg(long, default_value = "apps/web/dist")]
        web: PathBuf,
    },
}
#[derive(Clone)]
pub struct App {
    db: SqlitePool,
    origin: String,
    allowed_origins: Vec<String>,
    allow_any_origin: bool,
    hub: Arc<Mutex<Hub>>,
    trusted_proxies: Vec<std::net::IpAddr>,
    login_locks: Arc<[Mutex<()>; 64]>,
}
#[derive(Default)]
struct Hub {
    agents: HashMap<String, AgentLink>,
    viewers: HashMap<String, Viewer>,
    sessions: HashMap<String, SessionInfo>,
}
struct AgentLink {
    connection: String,
    owner: String,
    tx: mpsc::Sender<AgentCommand>,
    stop: watch::Sender<bool>,
    ready: bool,
}
struct Viewer {
    owner: String,
    auth_hash: String,
    session: Option<String>,
    tx: mpsc::Sender<Event>,
    stop: watch::Sender<bool>,
}
impl App {
    async fn close_auth(&self, token_hash: &str) {
        for v in self
            .hub
            .lock()
            .await
            .viewers
            .values()
            .filter(|v| v.auth_hash == token_hash)
        {
            let _ = v.stop.send(true);
        }
    }
    async fn notify_inventory(&self) {
        let owners: std::collections::HashSet<_> = self
            .hub
            .lock()
            .await
            .viewers
            .values()
            .map(|v| v.owner.clone())
            .collect();
        for owner in owners {
            if let Ok(event) = self.inventory(&owner).await {
                for viewer in self
                    .hub
                    .lock()
                    .await
                    .viewers
                    .values()
                    .filter(|v| v.owner == owner)
                {
                    deliver(viewer, event.clone());
                }
            }
        }
    }
}
fn deliver(viewer: &Viewer, event: Event) {
    if viewer.tx.try_send(event).is_err() {
        let _ = viewer.stop.send(true);
    }
}
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "UNAUTHENTICATED",
            "请登录或检查凭据",
        )
    }
    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", "资源不存在")
    }
    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "服务内部错误",
        )
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error=%e, "database request failed");
        Self::internal()
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error":{"code":self.code,"message":self.message,"retryable":self.status==StatusCode::SERVICE_UNAVAILABLE},"requestId":Uuid::new_v4().to_string()}))).into_response()
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let db = storage::open(&cli.database).await?;
    match cli.command {
        Command::AdminCreate { username } => {
            eprintln!("从标准输入读取密码（至少 12 字节），输入完成后 EOF：");
            let mut password = String::new();
            std::io::stdin().take(1025).read_to_string(&mut password)?;
            auth::create_admin(&db, &username, password.trim_end_matches(['\r', '\n'])).await?;
            println!("管理员已创建：{username}");
        }
        Command::LoginUnblock { ip } => {
            sqlx::query("DELETE FROM login_ip_failures WHERE ip=?")
                .bind(login_guard::normalize(ip).to_string())
                .execute(&db)
                .await?;
            println!("登录失败计数已清除：{ip}");
        }
        Command::Run {
            listen,
            origin,
            additional_origin,
            trusted_proxy,
            allow_insecure_lan,
            allow_any_origin,
            web,
        } => {
            let parsed = origin::validate(&origin, allow_insecure_lan)?;
            let mut allowed_origins = vec![origin.clone()];
            for extra in additional_origin {
                let other = origin::validate(&extra, allow_insecure_lan)?;
                anyhow::ensure!(
                    other.scheme() == parsed.scheme(),
                    "所有 origin 必须使用相同协议"
                );
                allowed_origins.push(extra);
            }
            let mut hub = Hub::default();
            for row in sqlx::query("SELECT data FROM terminal_sessions")
                .fetch_all(&db)
                .await?
            {
                let session: SessionInfo = serde_json::from_str(row.get("data"))?;
                hub.sessions.insert(session.id.clone(), session);
            }
            sqlx::query("UPDATE operations SET status='unknown' WHERE status='pending'")
                .execute(&db)
                .await?;
            let app = App {
                db,
                origin: origin.clone(),
                allowed_origins,
                allow_any_origin,
                hub: Arc::new(Mutex::new(hub)),
                trusted_proxies: trusted_proxy
                    .into_iter()
                    .map(login_guard::normalize)
                    .collect(),
                login_locks: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
            };
            let api = Router::new()
                .route("/auth/csrf", get(auth::get_csrf))
                .route("/auth/login", post(auth::login))
                .route("/auth/logout", post(auth::logout))
                .route("/auth/me", get(auth::me))
                .route("/machines", get(routes::machines))
                .route("/machines/{id}", patch(routes::rename_machine))
                .route("/enrollments", post(routes::enrollment))
                .route("/agent/enroll", post(routes::enroll))
                .route("/machines/{id}/revoke", post(routes::revoke))
                .route(
                    "/machines/{id}/sessions",
                    get(routes::sessions).post(routes::create),
                )
                .route("/sessions/{id}", patch(routes::update_session))
                .route("/sessions/{id}/close", post(routes::close))
                .route("/operations/{id}", get(routes::operation))
                .route("/ws/agent", get(transport::agent_upgrade))
                .route("/ws/browser", get(transport::browser_upgrade));
            let router = Router::new()
                .nest("/api/v1", api)
                .route("/health/live", get(|| async { "ok" }))
                .route("/health/ready", get(routes::ready))
                .fallback_service(
                    ServeDir::new(&web).not_found_service(ServeFile::new(web.join("index.html"))),
                )
                .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
                .layer(axum::middleware::from_fn(
                    |req: axum::extract::Request, next: axum::middleware::Next| async move {
                        let mut response = next.run(req).await;
                        response
                            .headers_mut()
                            .insert("cache-control", "no-store".parse().unwrap());
                        response
                            .headers_mut()
                            .insert("x-content-type-options", "nosniff".parse().unwrap());
                        response
                            .headers_mut()
                            .insert("x-frame-options", "DENY".parse().unwrap());
                        response
                    },
                ))
                .with_state(app);
            let listener = tokio::net::TcpListener::bind(listen).await?;
            tracing::info!(%listen, %origin, "Ergent preview server ready");
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        }
    }
    Ok(())
}
