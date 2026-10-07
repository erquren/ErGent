use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use base64::Engine;
use ergent_protocol::{ToolEvent, ToolEventKind as K};
use ergent_terminal::TerminalManager;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub async fn listen(manager: Arc<TerminalManager>) -> Result<tokio::task::JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    manager.set_event_url(format!("http://{}/v1/tool-events", listener.local_addr()?));
    let router = Router::new()
        .route("/v1/tool-events", post(ingest))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(manager);
    Ok(tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    }))
}
async fn ingest(
    State(manager): State<Arc<TerminalManager>>,
    headers: HeaderMap,
    Json(event): Json<ToolEvent>,
) -> StatusCode {
    if headers.contains_key("origin") {
        return StatusCode::FORBIDDEN;
    }
    let terminal = headers
        .get("x-ergent-terminal")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    match manager.report_tool_event(terminal, token, event) {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::FORBIDDEN,
    }
}
fn event(kind: K, turn_id: Option<String>) -> Result<ToolEvent> {
    event_for_run(kind, turn_id, std::env::var("ERGENT_TOOL_RUN_ID")?)
}
fn event_for_run(kind: K, turn_id: Option<String>, run_id: String) -> Result<ToolEvent> {
    Ok(ToolEvent {
        version: 1,
        event_id: Uuid::new_v4().to_string(),
        provider: "codex".into(),
        run_id,
        turn_id,
        kind,
        occurred_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64,
    })
}
async fn report(event: &ToolEvent) -> Result<()> {
    event.validate().map_err(anyhow::Error::msg)?;
    let url = url::Url::parse(
        &std::env::var("ERGENT_EVENTS_URL").context("请在新建的 Ergent 终端中运行")?,
    )?;
    anyhow::ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.path() == "/v1/tool-events"
            && url.username().is_empty()
            && url.password().is_none(),
        "事件上报仅支持 Agent 本机回环地址"
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(800))
        .build()?;
    client
        .post(url)
        .bearer_auth(std::env::var("ERGENT_EVENT_TOKEN")?)
        .header("x-ergent-terminal", std::env::var("ERGENT_TERMINAL_ID")?)
        .json(event)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}
fn stdin_json() -> Result<Value> {
    let mut data = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut data)?;
    anyhow::ensure!(data.len() <= 1024 * 1024, "hook payload too large");
    Ok(serde_json::from_slice(&data)?)
}
pub async fn generic_report() -> Result<()> {
    report(&serde_json::from_value::<ToolEvent>(stdin_json()?)?).await
}
fn map_codex(value: &Value, notification: bool) -> Option<(K, Option<String>)> {
    // Subagent/tool output and prompts are never forwarded.
    if value.get("agent_id").is_some() {
        return None;
    }
    let (kind, key) = if notification {
        if value.get("type")?.as_str()? != "agent-turn-complete" {
            return None;
        }
        (K::TurnCompleted, "turn-id")
    } else {
        let kind = match value.get("hook_event_name")?.as_str()? {
            "UserPromptSubmit" => K::TurnStarted,
            "PreToolUse" | "PostToolUse" => K::Activity,
            "PermissionRequest" => K::ApprovalRequested,
            "Interrupt" => K::TurnInterrupted,
            _ => return None,
        };
        (kind, "turn_id")
    };
    // Turn correlation is mandatory: never allow an uncorrelated callback to change another turn.
    let turn = value.get(key)?.as_str()?.to_owned();
    Some((kind, Some(turn)))
}
pub async fn codex_hook(payload: Option<String>) {
    let notification = payload.is_some();
    // Outside an Ergent wrapper this installed hook is intentionally a no-op.
    if std::env::var("ERGENT_TOOL_RUN_ID").is_ok() {
        let parsed = match payload {
            Some(s) => serde_json::from_str(&s).map_err(anyhow::Error::from),
            None => stdin_json(),
        };
        if let Ok(value) = parsed {
            if let Some((kind, turn)) = map_codex(&value, notification) {
                if let Ok(e) = event(kind, turn) {
                    let _ = report(&e).await;
                }
            }
        }
    }
    // Hooks must never approve, reject, block or add model context.
    if !notification {
        println!("{{}}");
    }
}

pub async fn launch_codex(args: Vec<String>) -> Result<()> {
    let run_id = Uuid::new_v4().to_string();
    let started = event_for_run(K::SessionStarted, None, run_id.clone())?;
    report(&started)
        .await
        .context("无法关联此终端，请确认 Agent 已更新，并新建终端")?;
    let exe = std::env::current_exe()?;
    let notify = serde_json::to_string(&vec![
        exe.to_string_lossy().to_string(),
        "codex-notify".into(),
    ])?;
    // JSON string arrays are valid TOML arrays. Only this invocation's notify setting is replaced.
    // Ctrl-C reaches Codex through the PTY process group; keep the wrapper alive to observe its exit.
    let interrupts = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });
    let result = tokio::process::Command::new("codex")
        .arg("-c")
        .arg(format!("notify={notify}"))
        .args(args)
        .env("ERGENT_TOOL_RUN_ID", &run_id)
        .status()
        .await;
    interrupts.abort();
    let kind = if result.as_ref().is_ok_and(|s| s.success()) {
        K::SessionStopped
    } else {
        K::SessionFailed
    };
    if let Ok(e) = event_for_run(kind, None, run_id) {
        let _ = report(&e).await;
    }
    let status = result.context("无法启动 codex，请先安装 Codex CLI 并确认 PATH")?;
    if !status.success() {
        bail!("Codex 已退出：{status}");
    }
    Ok(())
}

pub fn setup_codex() -> Result<()> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|p| PathBuf::from(p).join(".codex"))
        })
        .context("找不到 Codex 配置目录")?;
    std::fs::create_dir_all(&home)?;
    let path = home.join("hooks.json");
    let original = if path.exists() {
        Some(std::fs::read(&path)?)
    } else {
        None
    };
    let mut config: Value = match &original {
        Some(data) => serde_json::from_slice(data)?,
        None => json!({"hooks":{}}),
    };
    let root = config.as_object_mut().context("hooks.json 必须为对象")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks 必须为对象")?;
    let exe = std::env::current_exe()?.to_string_lossy().to_string();
    let command = if cfg!(windows) {
        let script = format!("& '{}' codex-hook", exe.replace('\'', "''"));
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!(
            "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    } else {
        format!("'{}' codex-hook", exe.replace('\'', "'\\''"))
    };
    for name in [
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Interrupt",
    ] {
        let groups = hooks
            .entry(name)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("hook event 必须为数组")?;
        let handler = json!({"type":"command","command":command,"timeout":2});
        if !groups.iter().any(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hs| hs.contains(&handler))
        }) {
            groups.push(json!({"hooks":[handler]}));
        }
    }
    let data = serde_json::to_vec_pretty(&config)?;
    if original.as_deref() == Some(data.as_slice()) {
        println!("Ergent hooks 已配置。在 Codex /hooks 中确认已信任。");
        return Ok(());
    }
    if let Some(original) = original {
        let backup = home.join(format!("hooks.json.ergent-backup-{}", Uuid::new_v4()));
        write_private(&backup, &original)?;
        println!("已备份原 hooks：{}", backup.display());
    }
    // Exclusive temporary file + replace; preserve unrelated hook entries.
    let temporary = home.join(format!(".ergent-hooks-{}.tmp", Uuid::new_v4()));
    write_private(&temporary, &data)?;
    std::fs::rename(&temporary, &path)?;
    println!("已写入 {}。在 Codex /hooks 中审阅并信任新增 hooks，然后在 Ergent 新终端中运行 ergent-agent codex。",path.display());
    Ok(())
}

fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_documented_events_and_ignores_stop_and_subagents() {
        assert_eq!(
            map_codex(
                &json!({"hook_event_name":"PermissionRequest","turn_id":"t","prompt":"private"}),
                false
            ),
            Some((K::ApprovalRequested, Some("t".into())))
        );
        assert_eq!(
            map_codex(
                &json!({"type":"agent-turn-complete","turn-id":"t","last-assistant-message":"private"}),
                true
            ),
            Some((K::TurnCompleted, Some("t".into())))
        );
        assert!(map_codex(&json!({"hook_event_name":"Stop","turn_id":"t"}), false).is_none());
        assert!(map_codex(
            &json!({"hook_event_name":"UserPromptSubmit","turn_id":"t","agent_id":"child"}),
            false
        )
        .is_none());
        assert!(map_codex(&json!({"hook_event_name":"UserPromptSubmit"}), false).is_none());
    }
}
