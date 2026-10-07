mod tool_events;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ergent_protocol::{AgentCommand, AgentEvent, Event, MAX_MESSAGE, SUBPROTOCOL};
use ergent_terminal::TerminalManager;
use fs4::fs_std::FileExt;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, protocol::WebSocketConfig, Message,
};
use uuid::Uuid;

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = ".local/agent.json", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Reads a one-time enrollment token from stdin.
    Enroll {
        #[arg(long)]
        server: String,
        #[arg(long, default_value = "My development machine")]
        name: String,
    },
    Run,
    Doctor,
    /// Launch Codex with per-invocation completion notifications (no config.toml edits).
    Codex {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Merge Ergent lifecycle hooks into the current user's Codex hooks.json.
    SetupCodex,
    /// Read a provider-neutral ToolEvent JSON object from stdin.
    ToolEvent,
    #[command(hide = true)]
    CodexHook,
    #[command(hide = true)]
    CodexNotify {
        payload: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    server: String,
    machine_id: String,
    secret: String,
}
fn server_url(server: &str) -> Result<url::Url> {
    let u = url::Url::parse(server)?;
    anyhow::ensure!(
        u.origin().ascii_serialization() == server,
        "server 必须是没有路径的 origin"
    );
    anyhow::ensure!(
        u.scheme() == "https"
            || (u.scheme() == "http"
                && u.host_str()
                    .is_some_and(|h| h == "127.0.0.1" || h == "localhost" || h == "[::1]")),
        "公网 Agent 连接必须使用 HTTPS"
    );
    Ok(u)
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Codex { args } => {
            tool_events::launch_codex(args).await?;
        }
        Command::SetupCodex => {
            tool_events::setup_codex()?;
        }
        Command::ToolEvent => {
            tool_events::generic_report().await?;
        }
        Command::CodexHook => {
            tool_events::codex_hook(None).await;
        }
        Command::CodexNotify { payload } => {
            tool_events::codex_hook(Some(payload)).await;
        }
        Command::Enroll { server, name } => {
            server_url(&server)?;
            anyhow::ensure!(
                !cli.config.exists(),
                "配置文件已存在；请使用不同 --config 路径注册其他设备"
            );
            eprintln!("从标准输入读取一次性注册 token，完成后 EOF：");
            let mut token = String::new();
            std::io::stdin().take(256).read_to_string(&mut token)?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let response=client.post(format!("{server}/api/v1/agent/enroll")).json(&json!({"enrollmentToken":token.trim(),"name":name,"os":std::env::consts::OS,"arch":std::env::consts::ARCH})).send().await?;
            if !response.status().is_success() {
                bail!("注册失败：HTTP {}", response.status());
            }
            let value: Value = response.json().await?;
            let config = Config {
                server,
                machine_id: value["data"]["machineId"]
                    .as_str()
                    .context("missing machineId")?
                    .into(),
                secret: value["data"]["secret"]
                    .as_str()
                    .context("missing credential")?
                    .into(),
            };
            if let Some(parent) = cli.config.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&cli.config)?;
            file.write_all(serde_json::to_string_pretty(&config)?.as_bytes())?;
            file.sync_all()?;
            println!(
                "已注册设备 {}，配置已保存至 {}",
                config.machine_id,
                cli.config.display()
            );
        }
        Command::Doctor => {
            let config: Config = serde_json::from_slice(&std::fs::read(&cli.config)?)?;
            server_url(&config.server)?;
            println!(
                "Agent {}\nOS: {} / {}\nServer: {}\nMachine: {}\nDefault cwd: {}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                config.server,
                config.machine_id,
                std::env::current_dir()?.display()
            );
        }
        Command::Run => {
            let config: Config =
                serde_json::from_slice(&std::fs::read(&cli.config).context("请先 enroll")?)?;
            server_url(&config.server)?;
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(cli.config.with_extension("lock"))?;
            lock.try_lock_exclusive()
                .context("同一配置的 Agent 已在运行")?;
            let epoch = Uuid::new_v4().to_string();
            // One manager/epoch for the daemon lifetime, not for an individual network connection.
            let manager = Arc::new(TerminalManager::new(
                config.machine_id.clone(),
                epoch.clone(),
            ));
            let event_listener = tool_events::listen(manager.clone()).await?;
            let mut delay = 1;
            loop {
                manager.reset_connections();
                let connection = connect(&config, &epoch, &manager);
                tokio::select! {
                    result=connection=>{if let Err(error)=result{tracing::warn!(%error,"连接中断，终端继续运行");}}
                    _=tokio::signal::ctrl_c()=>{tracing::info!("停止 Agent，关闭它创建的终端");break;}
                }
                let jitter = (Uuid::new_v4().as_u128() % 500) as u64;
                tokio::select! {_=tokio::time::sleep(Duration::from_millis(delay*1000+jitter))=>{},_=tokio::signal::ctrl_c()=>break}
                delay = (delay * 2).min(30);
            }
            event_listener.abort();
        }
    }
    Ok(())
}
async fn connect(config: &Config, epoch: &str, manager: &Arc<TerminalManager>) -> Result<()> {
    let mut url = server_url(&config.server)?;
    url.set_scheme(if config.server.starts_with("https:") {
        "wss"
    } else {
        "ws"
    })
    .map_err(|_| anyhow::anyhow!("URL"))?;
    url.set_path("/api/v1/ws/agent");
    let mut request = url.as_str().into_client_request()?;
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", config.secret).parse()?,
    );
    request
        .headers_mut()
        .insert("sec-websocket-protocol", SUBPROTOCOL.parse()?);
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let (mut ws, response) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async_with_config(request, Some(ws_config), false),
    )
    .await??;
    anyhow::ensure!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            == Some(SUBPROTOCOL),
        "协议不匹配"
    );
    let mut events = manager.events();
    let hello = AgentEvent::Hello {
        epoch: epoch.into(),
        default_cwd: std::env::current_dir()?.to_string_lossy().into(),
        sessions: manager.list(),
    };
    ws.send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await?;
    let mut last = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            event=events.recv()=>{
                // Lag means the stream has a gap. Reconnect and require a fresh snapshot instead of corrupting the screen.
                let event=event.context("输出队列落后，重新同步")?;
                tokio::time::timeout(Duration::from_secs(10),ws.send(Message::Text(serde_json::to_string(&event)?.into()))).await??;
            }
            _=tick.tick()=>{
                manager.expire_leases();if last.elapsed()>Duration::from_secs(45){bail!("服务端心跳超时");}
                tokio::time::timeout(Duration::from_secs(10),ws.send(Message::Ping(vec![].into()))).await??;
            }
            message=ws.next()=>{
                let Some(message)=message else{bail!("连接关闭")};last=Instant::now();
                match message?{
                    Message::Text(data)=>{
                        let command:AgentCommand=serde_json::from_str(&data)?;
                        let reply=match command{
                            AgentCommand::Welcome=>{tracing::info!("Agent 已连接，机器 {}",config.machine_id);None}
                            AgentCommand::Create{operation_id,session_id,config}=>{
                                let m=manager.clone();let result=tokio::task::spawn_blocking(move||m.create(&session_id,config)).await?;
                                Some(match result{Ok(session)=>AgentEvent::Operation{operation_id,session:Some(session),error:None},Err(e)=>AgentEvent::Operation{operation_id,session:None,error:Some(e.to_string())}})
                            }
                            AgentCommand::Close{operation_id,session_id}=>{
                                let m=manager.clone();let result=tokio::task::spawn_blocking(move||m.close(&session_id)).await?;
                                Some(match result{Ok(session)=>AgentEvent::Operation{operation_id,session:Some(session),error:None},Err(e)=>AgentEvent::Operation{operation_id,session:None,error:Some(e.to_string())}})
                            }
                            AgentCommand::Browser{viewer_id,command}=>{
                                let m=manager.clone();let v=viewer_id.clone();
                                let result=tokio::task::spawn_blocking(move||m.handle(&v,command)).await?;
                                result.err().map(|e|AgentEvent::Viewer{viewer_id,event:Event::error("TERMINAL_ERROR",e.to_string())})
                            }
                            AgentCommand::ViewerGone{viewer_id}=>{manager.disconnect(&viewer_id);None}
                        };
                        if let Some(reply)=reply{tokio::time::timeout(Duration::from_secs(10),ws.send(Message::Text(serde_json::to_string(&reply)?.into()))).await??;}
                    }
                    Message::Close(_)=>bail!("服务器关闭连接"),
                    Message::Binary(_)=>bail!("preview 不接受二进制控制帧"),
                    _=>{}
                }
            }
        }
    }
}
