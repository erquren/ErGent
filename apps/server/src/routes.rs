use crate::{
    auth,
    storage::{hash, now, secret},
    AgentCommand, ApiError, App,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use ergent_protocol::{CreateTerminal, Event, MachineInfo};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

type Reply = Result<Json<Value>, ApiError>;
impl App {
    pub async fn owned_machine(&self, user: &str, id: &str) -> Result<(), ApiError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM machines WHERE id=? AND owner_id=? AND revoked=0",
        )
        .bind(id)
        .bind(user)
        .fetch_one(&self.db)
        .await?;
        if count == 0 {
            return Err(ApiError::not_found());
        }
        Ok(())
    }
    pub async fn inventory(&self, owner: &str) -> Result<Event, ApiError> {
        let rows = sqlx::query("SELECT * FROM machines WHERE owner_id=? ORDER BY rowid")
            .bind(owner)
            .fetch_all(&self.db)
            .await?;
        let hub = self.hub.lock().await;
        let machines: Vec<_> = rows
            .iter()
            .map(|r| {
                let id: String = r.get("id");
                MachineInfo {
                    online: hub.agents.get(&id).is_some_and(|a| a.ready),
                    id,
                    name: r.get("name"),
                    os: r.get("os"),
                    arch: r.get("arch"),
                    revoked: r.get::<i64, _>("revoked") != 0,
                    default_cwd: r.get("default_cwd"),
                }
            })
            .collect();
        let mut sessions: Vec<_> = hub
            .sessions
            .values()
            .filter(|s| machines.iter().any(|m| m.id == s.machine_id && !m.revoked))
            .cloned()
            .collect();
        drop(hub);
        let preferences = sqlx::query("SELECT p.* FROM terminal_preferences p JOIN terminal_sessions s ON s.id=p.session_id JOIN machines m ON m.id=s.machine_id WHERE m.owner_id=? AND m.revoked=0")
            .bind(owner).fetch_all(&self.db).await?;
        for session in &mut sessions {
            // Presentation metadata belongs to the server, never the Agent.
            session.terminal_theme = None;
            if let Some(row) = preferences
                .iter()
                .find(|r| r.get::<String, _>("session_id") == session.id)
            {
                if let Some(title) = row.get::<Option<String>, _>("title") {
                    session.title = title;
                }
                session.terminal_theme = Some(row.get("theme"));
            }
        }
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Event::Inventory { machines, sessions })
    }
}
pub async fn ready(State(app): State<App>) -> Result<&'static str, ApiError> {
    sqlx::query("SELECT 1").execute(&app.db).await?;
    Ok("ok")
}
pub async fn machines(State(app): State<App>, headers: HeaderMap) -> Reply {
    let identity = auth::identity(&app, &headers, false).await?;
    if let Event::Inventory { machines, .. } = app.inventory(&identity.user).await? {
        Ok(Json(json!({"data":{"items":machines}})))
    } else {
        unreachable!()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameMachine {
    name: String,
}
pub async fn rename_machine(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RenameMachine>,
) -> Reply {
    let identity = auth::identity(&app, &headers, true).await?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_ARGUMENT",
            "设备名须为 1–120 个字符，且不能包含控制字符",
        ));
    }
    let result = sqlx::query("UPDATE machines SET name=? WHERE id=? AND owner_id=? AND revoked=0")
        .bind(name)
        .bind(&id)
        .bind(identity.user)
        .execute(&app.db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    app.notify_inventory().await;
    Ok(Json(json!({"data":{"id":id,"name":name}})))
}
pub async fn sessions(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Reply {
    let identity = auth::identity(&app, &headers, false).await?;
    app.owned_machine(&identity.user, &id).await?;
    let Event::Inventory { sessions, .. } = app.inventory(&identity.user).await? else {
        unreachable!()
    };
    let items: Vec<_> = sessions
        .into_iter()
        .filter(|s| s.machine_id == id)
        .collect();
    Ok(Json(json!({"data":{"items":items}})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSession {
    title: Option<String>,
    terminal_theme: Option<String>,
}
pub async fn update_session(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateSession>,
) -> Reply {
    let identity = auth::identity(&app, &headers, true).await?;
    let session = app
        .hub
        .lock()
        .await
        .sessions
        .get(&id)
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    app.owned_machine(&identity.user, &session.machine_id)
        .await?;
    let title = body.title.as_deref().map(str::trim);
    if title
        .is_some_and(|t| t.is_empty() || t.chars().count() > 120 || t.chars().any(char::is_control))
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_ARGUMENT",
            "终端名称须为 1–120 个字符，且不能包含控制字符",
        ));
    }
    let theme = body.terminal_theme.as_deref();
    if theme
        .is_some_and(|t| !["auto", "dark", "light", "dracula", "nord", "solarized"].contains(&t))
        || (title.is_none() && theme.is_none())
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_ARGUMENT",
            "请选择有效的终端名称或配色",
        ));
    }
    sqlx::query("INSERT INTO terminal_preferences(session_id,title,theme) VALUES(?,?,COALESCE(?,'auto')) ON CONFLICT(session_id) DO UPDATE SET title=COALESCE(?,terminal_preferences.title),theme=COALESCE(?,terminal_preferences.theme)")
        .bind(&id).bind(title).bind(theme).bind(title).bind(theme).execute(&app.db).await?;
    app.notify_inventory().await;
    Ok(Json(json!({"data":{"id":id}})))
}

pub async fn enrollment(State(app): State<App>, headers: HeaderMap) -> Reply {
    let identity = auth::identity(&app, &headers, true).await?;
    sqlx::query("DELETE FROM enrollments WHERE expires_at<=?")
        .bind(now())
        .execute(&app.db)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollments WHERE owner_id=?")
        .bind(&identity.user)
        .fetch_one(&app.db)
        .await?;
    if count >= 20 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "LIMIT_EXCEEDED",
            "注册凭据过多，请等待过期",
        ));
    }
    let token = secret();
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO enrollments(id,owner_id,token_hash,expires_at) VALUES(?,?,?,?)")
        .bind(&id)
        .bind(&identity.user)
        .bind(hash(&token))
        .bind(now() + 600)
        .execute(&app.db)
        .await?;
    Ok(Json(
        json!({"data":{"id":id,"token":token,"expiresAt":now()+600}}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Enroll {
    enrollment_token: String,
    name: String,
    os: String,
    arch: String,
}
pub async fn enroll(State(app): State<App>, Json(body): Json<Enroll>) -> Reply {
    if body.name.is_empty()
        || body.name.len() > 120
        || body.os.len() > 32
        || body.arch.len() > 32
        || body.enrollment_token.len() > 128
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "INVALID_ARGUMENT",
            "无效设备信息",
        ));
    }
    let mut tx = app.db.begin().await?;
    let row = sqlx::query("UPDATE enrollments SET consumed=1 WHERE token_hash=? AND consumed=0 AND expires_at>? RETURNING owner_id").bind(hash(&body.enrollment_token)).bind(now()).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::unauthorized)?;
    let owner: String = row.get("owner_id");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM machines WHERE owner_id=? AND revoked=0")
            .bind(&owner)
            .fetch_one(&mut *tx)
            .await?;
    if count >= 20 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "LIMIT_EXCEEDED",
            "最多接入 20 台设备",
        ));
    }
    let id = Uuid::new_v4().to_string();
    let token = secret();
    sqlx::query(
        "INSERT INTO machines(id,owner_id,name,os,arch,credential_hash) VALUES(?,?,?,?,?,?)",
    )
    .bind(&id)
    .bind(owner)
    .bind(body.name)
    .bind(body.os)
    .bind(body.arch)
    .bind(hash(&token))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    app.notify_inventory().await;
    Ok(Json(json!({"data":{"machineId":id,"secret":token}})))
}
pub async fn revoke(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Reply {
    let identity = auth::identity(&app, &headers, true).await?;
    app.owned_machine(&identity.user, &id).await?;
    sqlx::query("UPDATE machines SET revoked=1 WHERE id=?")
        .bind(&id)
        .execute(&app.db)
        .await?;
    {
        let mut hub = app.hub.lock().await;
        if let Some(agent) = hub.agents.remove(&id) {
            let _ = agent.stop.send(true);
        }
        for viewer in hub.viewers.values() {
            if viewer
                .session
                .as_ref()
                .and_then(|s| hub.sessions.get(s))
                .is_some_and(|s| s.machine_id == id)
            {
                let _ = viewer.stop.send(true);
            }
        }
    }
    app.notify_inventory().await;
    Ok(Json(json!({"data":{"revoked":true}})))
}
pub async fn create(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(config): Json<CreateTerminal>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let identity = auth::identity(&app, &headers, true).await?;
    app.owned_machine(&identity.user, &id).await?;
    config
        .validate()
        .map_err(|e| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "INVALID_ARGUMENT", e))?;
    start_operation(
        &app,
        &headers,
        &identity.user,
        &id,
        "create",
        None,
        Some(config),
    )
    .await
}
pub async fn close(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let identity = auth::identity(&app, &headers, true).await?;
    let machine = app
        .hub
        .lock()
        .await
        .sessions
        .get(&id)
        .ok_or_else(ApiError::not_found)?
        .machine_id
        .clone();
    app.owned_machine(&identity.user, &machine).await?;
    start_operation(
        &app,
        &headers,
        &identity.user,
        &machine,
        "close",
        Some(id),
        None,
    )
    .await
}
async fn start_operation(
    app: &App,
    headers: &HeaderMap,
    owner: &str,
    machine: &str,
    kind: &str,
    session: Option<String>,
    config: Option<CreateTerminal>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|k| Uuid::parse_str(k).is_ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "INVALID_ARGUMENT",
                "需要 UUID Idempotency-Key",
            )
        })?;
    let request_hash = hash(&json!([machine, session, config]).to_string());
    let id = Uuid::new_v4().to_string();
    let session_id = session.unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut tx = app.db.begin().await?;
    let inserted = sqlx::query("INSERT INTO operations VALUES(?,?,?,?,?,?,?,'pending',NULL,?) ON CONFLICT(owner_id,kind,idempotency_key) DO NOTHING")
        .bind(&id).bind(owner).bind(machine).bind(&session_id).bind(kind).bind(key).bind(&request_hash).bind(now()).execute(&mut *tx).await?.rows_affected()==1;
    let row =
        sqlx::query("SELECT * FROM operations WHERE owner_id=? AND kind=? AND idempotency_key=?")
            .bind(owner)
            .bind(kind)
            .bind(key)
            .fetch_one(&mut *tx)
            .await?;
    if row.get::<String, _>("request_hash") != request_hash {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "IDEMPOTENCY_CONFLICT",
            "同一幂等键不能使用不同参数",
        ));
    }
    if inserted {
        let sender = app
            .hub
            .lock()
            .await
            .agents
            .get(machine)
            .filter(|a| a.ready)
            .map(|a| a.tx.clone())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "MACHINE_OFFLINE",
                    "设备离线",
                )
            })?;
        let cmd = if let Some(config) = config {
            AgentCommand::Create {
                operation_id: id.clone(),
                session_id: session_id.clone(),
                config,
            }
        } else {
            AgentCommand::Close {
                operation_id: id.clone(),
                session_id: session_id.clone(),
            }
        };
        // Commit before sending; a lost result is unknown, never automatically replayed.
        tx.commit().await?;
        if sender.try_send(cmd).is_err() {
            sqlx::query("UPDATE operations SET status='failed',result=? WHERE id=?")
                .bind(json!({"error":"设备连接不可用"}).to_string())
                .bind(&id)
                .execute(&app.db)
                .await?;
        }
    } else {
        tx.commit().await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(
            json!({"data":{"operationId":row.get::<String,_>("id"),"sessionId":row.get::<String,_>("session_id"),"status":row.get::<String,_>("status")}}),
        ),
    ))
}
pub async fn operation(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Reply {
    let identity = auth::identity(&app, &headers, false).await?;
    sqlx::query(
        "UPDATE operations SET status='unknown' WHERE id=? AND owner_id=? AND status='pending' AND created_at<?",
    )
    .bind(&id)
    .bind(&identity.user)
    .bind(now() - 30)
    .execute(&app.db)
    .await?;
    let row = sqlx::query("SELECT * FROM operations WHERE id=? AND owner_id=?")
        .bind(&id)
        .bind(identity.user)
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let result: Option<String> = row.get("result");
    Ok(Json(
        json!({"data":{"id":id,"status":row.get::<String,_>("status"),"sessionId":row.get::<String,_>("session_id"),"result":result.and_then(|v| serde_json::from_str::<Value>(&v).ok())}}),
    ))
}
