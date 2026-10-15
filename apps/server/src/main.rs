mod auth;
mod login_guard;
mod origin;
mod routes;
mod setup;
mod storage;
mod transport;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use clap::{Parser, Subcommand, ValueEnum};
use ergent_protocol::{AgentCommand, Event, SessionInfo};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, watch, Mutex};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

#[derive(Parser)]
struct Cli {
    #[arg(
        long,
        env = "ERGENT_DATABASE",
        default_value = ".local/server.db",
        global = true
    )]
    database: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TlsMode {
    Direct,
    Proxy,
    DevHttp,
}

#[derive(Subcommand)]
enum Command {
    /// Rotate the first-account setup code; display only on the controlling terminal.
    SetupCode,
    /// Hidden password + confirmation in a terminal; single stdin value in pipelines.
    AdminCreate {
        #[arg(long, default_value = "admin")]
        username: String,
    },
    /// Clear an IP's login throttle and atomically record an audit event.
    LoginUnblock {
        ip: std::net::IpAddr,
        /// Operator label saved in the local audit trail.
        #[arg(long)]
        actor: String,
        #[arg(long)]
        reason: String,
    },
    Run {
        #[command(flatten)]
        login_policy: login_guard::Policy,
        #[arg(long, env = "ERGENT_LISTEN", default_value = "127.0.0.1:7777")]
        listen: std::net::SocketAddr,
        #[arg(long, env = "ERGENT_ORIGIN", default_value = "http://127.0.0.1:7777")]
        origin: String,
        /// Additional exact browser origins; no wildcard matching.
        #[arg(long, env = "ERGENT_ADDITIONAL_ORIGINS", value_delimiter = ',')]
        additional_origin: Vec<String>,
        /// Exact proxy peer IPs allowed to supply an overwritten X-Real-IP header.
        #[arg(long, env = "ERGENT_TRUSTED_PROXY", value_delimiter = ',')]
        trusted_proxy: Vec<std::net::IpAddr>,
        /// Allow HTTP on explicitly configured private IPv4 LAN origins.
        #[arg(long)]
        allow_insecure_lan: bool,
        /// Development opt-in: accept browser requests from any Origin.
        #[arg(long)]
        allow_any_origin: bool,
        #[arg(long, env = "ERGENT_WEB", default_value = "apps/web/dist")]
        web: PathBuf,
        /// Direct requires certificate files; proxy requires TLS termination upstream.
        #[arg(long, env = "ERGENT_TLS_MODE", value_enum)]
        tls_mode: Option<TlsMode>,
        #[arg(long, env = "ERGENT_TLS_CERT", requires = "tls_key")]
        tls_cert: Option<PathBuf>,
        #[arg(long, env = "ERGENT_TLS_KEY", requires = "tls_cert")]
        tls_key: Option<PathBuf>,
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
    login_policy: login_guard::Policy,
    connections: Connections,
}
/// Each pending/upgraded callback owns a receiver until its cleanup completes.
/// A sticky value prevents an upgrade racing shutdown from missing cancellation.
#[derive(Clone)]
struct Connections {
    shutdown: watch::Sender<bool>,
}
impl Default for Connections {
    fn default() -> Self {
        let (shutdown, _) = watch::channel(false);
        Self { shutdown }
    }
}
impl Connections {
    fn register(&self) -> Result<watch::Receiver<bool>, ApiError> {
        let receiver = self.shutdown.subscribe();
        if *receiver.borrow() {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "SERVER_STOPPING",
                "服务器正在关闭",
            ));
        }
        Ok(receiver)
    }
    fn stop(&self) {
        self.shutdown.send_replace(true);
    }
    async fn drained(&self) {
        self.shutdown.closed().await;
    }
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
        Command::SetupCode => setup::console_code(&db, true).await?,
        Command::AdminCreate { username } => {
            let password = ergent_cli_input::read_admin_password()?;
            auth::create_admin(&db, &username, &password).await?;
            println!("管理员已创建：{username}");
        }
        Command::LoginUnblock { ip, actor, reason } => {
            let audit_id = login_guard::unblock(&db, ip, &actor, &reason).await?;
            println!(
                "登录限制已清除：{}；审计记录：{audit_id}",
                login_guard::normalize(ip)
            );
        }
        Command::Run {
            login_policy,
            listen,
            origin,
            additional_origin,
            trusted_proxy,
            allow_insecure_lan,
            allow_any_origin,
            web,
            tls_mode,
            tls_cert,
            tls_key,
        } => {
            login_policy.validate()?;
            let parsed = origin::validate(&origin, allow_insecure_lan)?;
            let direct_tls = validate_tls_mode(
                tls_mode,
                tls_cert.is_some(),
                tls_key.is_some(),
                parsed.scheme(),
            )?;
            if tls_mode == Some(TlsMode::DevHttp) {
                tracing::warn!("开发 HTTP 模式：流量未经 TLS 加密，仅用于本机或受保护测试网络；不要暴露公网。首次网页设置仍要求双回环连接，Docker bridge 请使用 admin-create。");
            }
            let tls_config = if direct_tls {
                let cert =
                    tls_cert.ok_or_else(|| anyhow::anyhow!("direct 模式需要 TLS 证书文件"))?;
                let key = tls_key.ok_or_else(|| anyhow::anyhow!("direct 模式需要 TLS 私钥文件"))?;
                let _ = rustls::crypto::ring::default_provider().install_default();
                Some(axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?)
            } else {
                None
            };
            let mut allowed_origins = vec![origin.clone()];
            for extra in additional_origin {
                let other = origin::validate(&extra, allow_insecure_lan)?;
                anyhow::ensure!(
                    other.scheme() == parsed.scheme(),
                    "所有 origin 必须使用相同协议"
                );
                allowed_origins.push(extra);
            }
            setup::console_code(&db, false).await?;
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
                login_policy,
                connections: Connections::default(),
            };
            let api = Router::new()
                .route("/auth/csrf", get(auth::get_csrf))
                .route("/auth/login", post(auth::login))
                .route("/auth/setup", get(setup::status).post(setup::create))
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
            let connections = app.connections.clone();
            let mut http_shutdown = connections.shutdown.subscribe();
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
            let shutdown = shutdown_signal();
            let listener = tokio::net::TcpListener::bind(listen).await?;
            tracing::info!(%listen, %origin, "Ergent preview server ready");
            let serve = async move {
                let service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
                if let Some(config) = tls_config {
                    let handle = axum_server::Handle::new();
                    let server = axum_server::from_tcp_rustls(listener.into_std()?, config)?
                        .handle(handle.clone())
                        .serve(service);
                    tokio::pin!(server);
                    tokio::select! {
                        result = &mut server => result,
                        _ = http_shutdown.wait_for(|stopped| *stopped) => {
                            handle.graceful_shutdown(None);
                            server.await
                        }
                    }
                } else {
                    axum::serve(listener, service)
                        .with_graceful_shutdown(async move {
                            let _ = http_shutdown.wait_for(|stopped| *stopped).await;
                        })
                        .await
                }
            };
            tokio::pin!(serve);
            tokio::select! {
                result = &mut serve => result?,
                _ = shutdown => {
                    connections.stop();
                    // Axum's upgraded callbacks are detached from its HTTP tasks.
                    // Wait for both, including their database/Hub cleanup, before
                    // dropping Tokio. Bound even slow HTTP or WebSocket clients.
                    tokio::time::timeout(std::time::Duration::from_secs(15), async {
                        (&mut serve).await?;
                        connections.drained().await;
                        Ok::<_, std::io::Error>(())
                    })
                    .await
                    .map_err(|_| anyhow::anyhow!("Connection drain exceeded 15 seconds"))??;
                }
            }
        }
    }
    Ok(())
}

fn validate_tls_mode(
    mode: Option<TlsMode>,
    cert: bool,
    key: bool,
    scheme: &str,
) -> anyhow::Result<bool> {
    anyhow::ensure!(cert == key, "TLS 证书和私钥必须同时配置");
    let direct = mode.map_or(cert, |mode| mode == TlsMode::Direct);
    if direct {
        anyhow::ensure!(cert && key, "direct 模式需要 TLS 证书与私钥文件");
        anyhow::ensure!(scheme == "https", "direct 模式的 origin 必须使用 HTTPS");
    } else {
        anyhow::ensure!(!cert && !key, "非 direct 模式不能配置 TLS 证书");
        if mode == Some(TlsMode::Proxy) {
            anyhow::ensure!(
                scheme == "https",
                "proxy 模式需要外部 HTTPS origin；明文测试请显式使用 dev-http"
            );
        }
        if mode == Some(TlsMode::DevHttp) {
            anyhow::ensure!(scheme == "http", "dev-http 模式的 origin 必须使用 HTTP");
        }
    }
    Ok(direct)
}

/// Install Unix handlers before binding the listener (including the startup race).
fn shutdown_signal() -> impl std::future::Future<Output = ()> {
    #[cfg(unix)]
    let (mut interrupt, mut terminate) = (
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("failed to install interrupt signal handler"),
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install termination signal handler"),
    );
    async move {
        #[cfg(unix)]
        tokio::select! {
            _ = interrupt.recv() => {},
            _ = terminate.recv() => {},
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install interrupt signal handler");
        tracing::info!("Shutdown requested; draining connections");
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::{validate_tls_mode, Connections, TlsMode};
    use std::time::Duration;

    #[test]
    fn transport_modes_fail_closed_and_keep_native_defaults() {
        assert!(validate_tls_mode(Some(TlsMode::Direct), true, true, "https").unwrap());
        assert!(!validate_tls_mode(Some(TlsMode::Proxy), false, false, "https").unwrap());
        assert!(!validate_tls_mode(Some(TlsMode::DevHttp), false, false, "http").unwrap());
        assert!(!validate_tls_mode(None, false, false, "http").unwrap());
        assert!(!validate_tls_mode(None, false, false, "https").unwrap());
        assert!(validate_tls_mode(None, true, true, "https").unwrap());
        for (mode, cert, key, scheme) in [
            (Some(TlsMode::Direct), false, false, "https"),
            (Some(TlsMode::Direct), true, false, "https"),
            (Some(TlsMode::Direct), true, true, "http"),
            (Some(TlsMode::Proxy), true, true, "https"),
            (Some(TlsMode::Proxy), false, false, "http"),
            (Some(TlsMode::DevHttp), false, false, "https"),
            (Some(TlsMode::DevHttp), true, true, "http"),
            (None, false, true, "https"),
        ] {
            assert!(validate_tls_mode(mode, cert, key, scheme).is_err());
        }
    }

    #[tokio::test]
    async fn pending_upgrade_is_counted_and_observes_sticky_shutdown() {
        let connections = Connections::default();
        let mut pending = connections.register().unwrap();
        connections.stop();
        assert!(connections.register().is_err());
        assert!(*pending.wait_for(|stopped| *stopped).await.unwrap());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), connections.drained())
                .await
                .is_err()
        );
        drop(pending);
        tokio::time::timeout(Duration::from_secs(1), connections.drained())
            .await
            .unwrap();
    }
}
