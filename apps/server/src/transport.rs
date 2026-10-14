use crate::{
    auth, deliver,
    storage::{hash, now},
    AgentLink, ApiError, App, Viewer,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{HeaderMap, StatusCode},
    response::Response,
};
use ergent_protocol::{
    inventory::{chunks, InventoryAssembler, TRANSFER_TIMEOUT_SECS},
    AgentCommand, AgentEvent, BrowserCommand, Event, SessionInfo, MAX_MESSAGE, SUBPROTOCOL,
};
use serde_json::json;
use sqlx::Row;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

fn protocol(headers: &HeaderMap) -> Result<(), ApiError> {
    if !headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|v| v.trim() == SUBPROTOCOL))
    {
        return Err(ApiError::new(
            StatusCode::UPGRADE_REQUIRED,
            "PROTOCOL_UNSUPPORTED",
            "不兼容的协议",
        ));
    }
    Ok(())
}
pub async fn agent_upgrade(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    protocol(&headers)?;
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(ApiError::unauthorized)?;
    let row = sqlx::query("SELECT id,owner_id FROM machines WHERE credential_hash=? AND revoked=0")
        .bind(hash(token))
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let machine: String = row.get("id");
    let owner: String = row.get("owner_id");
    Ok(ws
        .protocols([SUBPROTOCOL])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| agent_socket(app, socket, machine, owner)))
}
pub async fn browser_upgrade(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    protocol(&headers)?;
    auth::origin(&app, &headers)?;
    let identity = auth::identity(&app, &headers, false).await?;
    Ok(ws
        .protocols([SUBPROTOCOL])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| browser_socket(app, socket, identity.user, identity.token_hash)))
}
async fn send<T: serde::Serialize>(socket: &mut WebSocket, value: &T) -> Result<(), ()> {
    let data = serde_json::to_string(value).map_err(|_| ())?;
    if data.len() > MAX_MESSAGE {
        return Err(());
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        socket.send(Message::Text(data.into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}
async fn ping(socket: &mut WebSocket) -> Result<(), ()> {
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Ping(vec![].into())),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}
async fn agent_socket(app: App, mut socket: WebSocket, machine: String, owner: String) {
    let connection = Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel(64);
    let (stop, mut stopped) = watch::channel(false);
    if app.owned_machine(&owner, &machine).await.is_err() {
        return;
    }
    {
        let mut hub = app.hub.lock().await;
        if let Some(old) = hub.agents.insert(
            machine.clone(),
            AgentLink {
                connection: connection.clone(),
                owner: owner.clone(),
                tx,
                stop,
                ready: false,
            },
        ) {
            let _ = old.stop.send(true);
        }
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut last = Instant::now();
    let mut epoch = None;
    let mut inventory = InventoryAssembler::default();
    let mut inventory_started: Option<Instant> = None;
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            _ = heartbeat.tick() => { if inventory_started.is_some_and(|t|t.elapsed()>Duration::from_secs(TRANSFER_TIMEOUT_SECS)) || last.elapsed()>Duration::from_secs(45) || ping(&mut socket).await.is_err() { break; } }
            command = rx.recv() => { let Some(command)=command else { break }; if send(&mut socket,&command).await.is_err() { break; } }
            message = socket.recv() => {
                let Some(Ok(message))=message else { break }; last=Instant::now();
                let current = app.hub.lock().await.agents.get(&machine).is_some_and(|a| a.connection==connection);
                if !current { break; }
                if let Message::Text(text) = message {
                    let Ok(event)=serde_json::from_str::<AgentEvent>(&text) else { break };
                    let event = match event {
                        AgentEvent::InventoryChunk(chunk) => {
                            let started = inventory_started.get_or_insert_with(Instant::now);
                            if started.elapsed()>Duration::from_secs(TRANSFER_TIMEOUT_SECS) { break; }
                            match inventory.push(chunk) {
                                Ok(Some(bytes)) => {
                                    inventory_started = None;
                                    let Ok(event) = serde_json::from_slice::<AgentEvent>(&bytes) else { break };
                                    if !matches!(event, AgentEvent::Hello { .. } | AgentEvent::Inventory { .. }) { break; }
                                    event
                                }
                                Ok(None) => continue,
                                Err(_) => break,
                            }
                        }
                        // v2 inventories must use the bounded transfer, even when small.
                        AgentEvent::Hello { .. } | AgentEvent::Inventory { .. } => break,
                        event => { if inventory.in_progress() { break; } event }
                    };
                    match event {
                        AgentEvent::InventoryChunk(_) => unreachable!(),
                        AgentEvent::Hello { epoch: e, default_cwd, sessions } => {
                            if epoch.is_some() || e.is_empty() || e.len()>128 || default_cwd.len()>4096 { break; }
                            if reconcile(&app,&machine,&e,sessions,&connection).await.is_err() { break; }
                            if sqlx::query("UPDATE machines SET default_cwd=? WHERE id=? AND revoked=0").bind(default_cwd).bind(&machine).execute(&app.db).await.is_err() { break; }
                            epoch=Some(e);
                            if let Some(a)=app.hub.lock().await.agents.get_mut(&machine) { if a.connection==connection { a.ready=true; } }
                            if send(&mut socket,&AgentCommand::Welcome).await.is_err() { break; }
                            app.notify_inventory().await;
                        }
                        AgentEvent::Inventory { sessions } => {
                            let Some(e)=&epoch else { break }; if reconcile(&app,&machine,e,sessions,&connection).await.is_err() { break; } app.notify_inventory().await;
                        }
                        AgentEvent::Session { session } => {
                            if Some(&session.agent_epoch)!=epoch.as_ref() || session.machine_id!=machine { break; }
                            if save_session(&app,&session).await.is_err() { break; } app.notify_inventory().await;
                        }
                        AgentEvent::Operation { operation_id, session, error } => {
                            if epoch.is_none() { break; }
                            if record_operation(&app,&machine,epoch.as_deref().unwrap(),&operation_id,session,error).await.is_err() { break; }
                            app.notify_inventory().await;
                        }
                        AgentEvent::Viewer { viewer_id, event } => {
                            let hub=app.hub.lock().await;
                            if let Some(viewer)=hub.viewers.get(&viewer_id).filter(|v|v.owner==owner) {
                                let session=match &event { Event::Snapshot { session_id,.. }|Event::Output {session_id,..}|Event::Lease{session_id,..}=>Some(session_id),_=>None };
                                let allowed=match session { Some(s)=>viewer.session.as_ref()==Some(s)&&hub.sessions.get(s).is_some_and(|s|s.machine_id==machine),None=>matches!(event,Event::InputAck{..}|Event::Error{..}) };
                                if allowed { deliver(viewer,event); }
                            }
                        }
                    }
                } else if matches!(message, Message::Close(_)|Message::Binary(_)) { break; }
            }
        }
    }
    {
        let mut hub = app.hub.lock().await;
        if hub
            .agents
            .get(&machine)
            .is_some_and(|a| a.connection == connection)
        {
            hub.agents.remove(&machine);
        } else {
            // A replaced connection must not invalidate the replacement's operations.
            return;
        }
    }
    let _ = sqlx::query(
        "UPDATE operations SET status='unknown' WHERE machine_id=? AND status='pending'",
    )
    .bind(&machine)
    .execute(&app.db)
    .await;
    app.notify_inventory().await;
}
async fn record_operation(
    app: &App,
    machine: &str,
    epoch: &str,
    operation_id: &str,
    mut session: Option<SessionInfo>,
    error: Option<String>,
) -> Result<(), ApiError> {
    let row = sqlx::query("SELECT session_id,kind FROM operations WHERE id=? AND machine_id=?")
        .bind(operation_id)
        .bind(machine)
        .fetch_optional(&app.db)
        .await?;
    let Some(row) = row else {
        return Ok(());
    };
    if let Some(s) = session.take() {
        if s.id != row.get::<String, _>("session_id")
            || s.machine_id != machine
            || s.agent_epoch != epoch
        {
            return Err(ApiError::not_found());
        }
        session = Some(save_session(app, &s).await?);
    }
    let status = if error.is_some() {
        "failed"
    } else if row.get::<String, _>("kind") == "close" {
        if session
            .as_ref()
            .is_some_and(|s| matches!(s.lifecycle.as_str(), "terminating" | "exited"))
        {
            "accepted"
        } else {
            // An older Agent may reply with running. It has not confirmed the new
            // close contract, so do not report either acceptance or process exit.
            "unknown"
        }
    } else {
        "succeeded"
    };
    sqlx::query("UPDATE operations SET status=?,result=? WHERE id=?")
        .bind(status)
        .bind(json!({"session":session,"error":error}).to_string())
        .bind(operation_id)
        .execute(&app.db)
        .await?;
    Ok(())
}
fn preserve_lifecycle(current: &SessionInfo, incoming: &mut SessionInfo) {
    if current.agent_epoch != incoming.agent_epoch {
        return;
    }
    incoming.output_truncated |= current.output_truncated;
    // Lifecycle and final-output markers are monotonic even if a delayed close
    // acknowledgement or inventory arrives after the authoritative wait event.
    if matches!(current.lifecycle.as_str(), "exited" | "lost")
        || (current.lifecycle == "terminating" && incoming.lifecycle == "running")
    {
        incoming.lifecycle = current.lifecycle.clone();
        incoming.exit_code = current.exit_code;
    }
}
async fn save_session(app: &App, incoming: &SessionInfo) -> Result<SessionInfo, ApiError> {
    let mut session = incoming.clone();
    // Use the same SQLite-writer -> hub ordering as inventory reconciliation and
    // operation creation. Guarding and persisting must observe one current state.
    let mut transaction = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let mut hub = app.hub.lock().await;
    if let Some(current) = hub.sessions.get(&session.id) {
        if current.machine_id != session.machine_id {
            return Err(ApiError::not_found());
        }
        preserve_lifecycle(current, &mut session);
    }
    let data = serde_json::to_string(&session).map_err(|_| ApiError::internal())?;
    sqlx::query("INSERT INTO terminal_sessions VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE machine_id=excluded.machine_id")
        .bind(&session.id).bind(&session.machine_id).bind(data).execute(&mut *transaction).await?;
    transaction.commit().await?;
    hub.sessions.insert(session.id.clone(), session.clone());
    Ok(session)
}
async fn reconcile(
    app: &App,
    machine: &str,
    epoch: &str,
    sessions: Vec<SessionInfo>,
    connection: &str,
) -> Result<(), ApiError> {
    let mut ids = HashSet::with_capacity(sessions.len());
    if sessions
        .iter()
        .any(|s| s.machine_id != machine || s.agent_epoch != epoch || !ids.insert(s.id.clone()))
    {
        return Err(ApiError::not_found());
    }
    // Match start_operation's lock order: SQLite writer first, then hub. A deferred
    // transaction would not acquire the writer until after hub, causing an inversion.
    let mut transaction = app.db.begin_with("BEGIN IMMEDIATE").await?;
    // Validate the complete list before changing anything. The hub lock also keeps a
    // replacement connection from publishing a newer inventory during this commit.
    let mut hub = app.hub.lock().await;
    if !hub
        .agents
        .get(machine)
        .is_some_and(|a| a.connection == connection)
        || sessions.iter().any(|s| {
            hub.sessions
                .get(&s.id)
                .is_some_and(|old| old.machine_id != machine)
        })
    {
        return Err(ApiError::not_found());
    }
    let mut updates: Vec<_> = hub
        .sessions
        .values()
        .filter(|s| {
            s.machine_id == machine
                && matches!(s.lifecycle.as_str(), "running" | "terminating")
                && (s.agent_epoch != epoch || !ids.contains(&s.id))
        })
        .cloned()
        .map(|mut s| {
            s.lifecycle = "lost".into();
            s
        })
        .collect();
    updates.extend(sessions.into_iter().map(|mut session| {
        if let Some(current) = hub
            .sessions
            .get(&session.id)
            .filter(|s| s.agent_epoch == session.agent_epoch)
        {
            preserve_lifecycle(current, &mut session);
        }
        session
    }));
    for session in &updates {
        let data = serde_json::to_string(session).map_err(|_| ApiError::internal())?;
        sqlx::query("INSERT INTO terminal_sessions VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE machine_id=excluded.machine_id")
            .bind(&session.id).bind(&session.machine_id).bind(data).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    for session in updates {
        hub.sessions.insert(session.id.clone(), session);
    }
    Ok(())
}

/// Chunk at the socket boundary so a large inventory occupies one bounded queue item,
/// never hundreds of queued fragments. No output or newer inventory can interleave.
async fn send_browser_event(
    socket: &mut WebSocket,
    event: &Event,
    transfer_id: &mut u64,
) -> Result<(), ()> {
    if matches!(event, Event::Inventory { .. }) {
        *transfer_id = transfer_id.checked_add(1).ok_or(())?;
        let parts = chunks(event, *transfer_id).map_err(|_| ())?;
        tokio::time::timeout(Duration::from_secs(TRANSFER_TIMEOUT_SECS), async {
            for chunk in parts {
                send(socket, &Event::InventoryChunk(chunk)).await?;
            }
            Ok(())
        })
        .await
        .map_err(|_| ())?
    } else {
        send(socket, event).await
    }
}
async fn browser_socket(app: App, mut socket: WebSocket, owner: String, auth_hash: String) {
    let viewer_id = Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel(64);
    let (stop, mut stopped) = watch::channel(false);
    {
        let mut hub = app.hub.lock().await;
        if hub.viewers.values().filter(|v| v.owner == owner).count() >= 8 {
            return;
        }
        hub.viewers.insert(
            viewer_id.clone(),
            Viewer {
                owner: owner.clone(),
                auth_hash: auth_hash.clone(),
                session: None,
                tx,
                stop,
            },
        );
    }
    let mut transfer_id = 0;
    if let Ok(event) = app.inventory(&owner).await {
        if send_browser_event(&mut socket, &event, &mut transfer_id)
            .await
            .is_err()
        {
            cleanup(&app, &viewer_id, &owner).await;
            return;
        }
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut last = Instant::now();
    loop {
        tokio::select! {
            _=stopped.changed()=>break,
            _=heartbeat.tick()=>{
                let valid=sqlx::query_scalar::<_,i64>("SELECT count(*) FROM web_sessions WHERE token_hash=? AND expires_at>?").bind(&auth_hash).bind(now()).fetch_one(&app.db).await.unwrap_or(0)>0;
                if !valid||last.elapsed()>Duration::from_secs(45)||ping(&mut socket).await.is_err(){break;}
            }
            event=rx.recv()=>{let Some(event)=event else{break};if send_browser_event(&mut socket,&event,&mut transfer_id).await.is_err(){break;}}
            message=socket.recv()=>{
                let Some(Ok(message))=message else{break};last=Instant::now();
                if let Message::Text(text)=message {
                    let Ok(command)=serde_json::from_str::<BrowserCommand>(&text) else{break};
                    if let Err(e)=route_browser(&app,&owner,&viewer_id,command).await{if send(&mut socket,&Event::error(e.code,e.message)).await.is_err(){break;}}
                }else if matches!(message,Message::Close(_)|Message::Binary(_)){break;}
            }
        }
    }
    cleanup(&app, &viewer_id, &owner).await;
}
async fn cleanup(app: &App, viewer: &str, owner: &str) {
    let mut hub = app.hub.lock().await;
    hub.viewers.remove(viewer);
    for agent in hub.agents.values().filter(|a| a.owner == owner) {
        if agent
            .tx
            .try_send(AgentCommand::ViewerGone {
                viewer_id: viewer.into(),
            })
            .is_err()
        {
            let _ = agent.stop.send(true);
        }
    }
}
async fn route_browser(
    app: &App,
    owner: &str,
    viewer: &str,
    command: BrowserCommand,
) -> Result<(), ApiError> {
    let sid = command.session_id().to_string();
    let machine = app
        .hub
        .lock()
        .await
        .sessions
        .get(&sid)
        .ok_or_else(ApiError::not_found)?
        .machine_id
        .clone();
    app.owned_machine(owner, &machine).await?;
    let mut hub = app.hub.lock().await;
    let agent = hub
        .agents
        .get(&machine)
        .filter(|a| a.ready)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "MACHINE_OFFLINE",
                "设备离线",
            )
        })?;
    let tx = agent.tx.clone();
    let active = hub.viewers.get(viewer).and_then(|v| v.session.clone());
    if matches!(command, BrowserCommand::Attach { .. }) {
        if let Some(old) = active.as_ref().filter(|s| **s != sid) {
            if let Some(a) = hub
                .sessions
                .get(old)
                .and_then(|s| hub.agents.get(&s.machine_id))
            {
                let _ = a.tx.try_send(AgentCommand::Browser {
                    viewer_id: viewer.into(),
                    command: BrowserCommand::Detach {
                        session_id: old.clone(),
                    },
                });
            }
        }
        if let Some(v) = hub.viewers.get_mut(viewer) {
            v.session = Some(sid.clone());
        }
    } else if active.as_ref() != Some(&sid) {
        return Err(ApiError::not_found());
    }
    if matches!(command, BrowserCommand::Detach { .. }) {
        if let Some(v) = hub.viewers.get_mut(viewer) {
            v.session = None;
        }
    }
    tx.try_send(AgentCommand::Browser {
        viewer_id: viewer.into(),
        command,
    })
    .map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "MACHINE_OFFLINE",
            "设备连接繁忙",
        )
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "close_tests.rs"]
mod close_tests;

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod inventory_tests;
