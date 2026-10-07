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
    AgentCommand, AgentEvent, BrowserCommand, Event, SessionInfo, MAX_MESSAGE, SUBPROTOCOL,
};
use serde_json::json;
use sqlx::Row;
use std::time::{Duration, Instant};
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
    loop {
        tokio::select! {
            _ = stopped.changed() => break,
            _ = heartbeat.tick() => { if last.elapsed()>Duration::from_secs(45) || ping(&mut socket).await.is_err() { break; } }
            command = rx.recv() => { let Some(command)=command else { break }; if send(&mut socket,&command).await.is_err() { break; } }
            message = socket.recv() => {
                let Some(Ok(message))=message else { break }; last=Instant::now();
                let current = app.hub.lock().await.agents.get(&machine).is_some_and(|a| a.connection==connection);
                if !current { break; }
                if let Message::Text(text) = message {
                    let Ok(event)=serde_json::from_str::<AgentEvent>(&text) else { break };
                    match event {
                        AgentEvent::Hello { epoch: e, default_cwd, sessions } => {
                            if epoch.is_some() || sessions.len()>1000 || default_cwd.len()>4096 { break; }
                            if reconcile(&app,&machine,&e,sessions).await.is_err() { break; }
                            if sqlx::query("UPDATE machines SET default_cwd=? WHERE id=? AND revoked=0").bind(default_cwd).bind(&machine).execute(&app.db).await.is_err() { break; }
                            epoch=Some(e);
                            if let Some(a)=app.hub.lock().await.agents.get_mut(&machine) { if a.connection==connection { a.ready=true; } }
                            if send(&mut socket,&AgentCommand::Welcome).await.is_err() { break; }
                            app.notify_inventory().await;
                        }
                        AgentEvent::Inventory { sessions } => {
                            let Some(e)=&epoch else { break }; if reconcile(&app,&machine,e,sessions).await.is_err() { break; } app.notify_inventory().await;
                        }
                        AgentEvent::Session { session } => {
                            if Some(&session.agent_epoch)!=epoch.as_ref() || session.machine_id!=machine { break; }
                            if save_session(&app,&session).await.is_err() { break; } app.notify_inventory().await;
                        }
                        AgentEvent::Operation { operation_id, session, error } => {
                            if epoch.is_none() { break; }
                            let row=sqlx::query("SELECT session_id FROM operations WHERE id=? AND machine_id=?").bind(&operation_id).bind(&machine).fetch_optional(&app.db).await;
                            let Ok(Some(row))=row else { continue };
                            if let Some(ref s)=session {
                                if s.id!=row.get::<String,_>("session_id") || s.machine_id!=machine || Some(&s.agent_epoch)!=epoch.as_ref() { break; }
                                if save_session(&app,s).await.is_err() { break; }
                            }
                            let status=if error.is_some(){"failed"}else{"succeeded"};
                            let _=sqlx::query("UPDATE operations SET status=?,result=? WHERE id=?").bind(status).bind(json!({"session":session,"error":error}).to_string()).bind(operation_id).execute(&app.db).await;
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
async fn save_session(app: &App, session: &SessionInfo) -> Result<(), ApiError> {
    // Existing IDs may never move to another device.
    {
        let hub = app.hub.lock().await;
        if hub
            .sessions
            .get(&session.id)
            .is_some_and(|s| s.machine_id != session.machine_id)
        {
            return Err(ApiError::not_found());
        }
    }
    let data = serde_json::to_string(session).map_err(|_| ApiError::internal())?;
    sqlx::query("INSERT INTO terminal_sessions VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE machine_id=excluded.machine_id").bind(&session.id).bind(&session.machine_id).bind(data).execute(&app.db).await?;
    app.hub
        .lock()
        .await
        .sessions
        .insert(session.id.clone(), session.clone());
    Ok(())
}
async fn reconcile(
    app: &App,
    machine: &str,
    epoch: &str,
    sessions: Vec<SessionInfo>,
) -> Result<(), ApiError> {
    if sessions.len() > 1000
        || sessions
            .iter()
            .any(|s| s.machine_id != machine || s.agent_epoch != epoch)
    {
        return Err(ApiError::not_found());
    }
    let lost: Vec<_> = app
        .hub
        .lock()
        .await
        .sessions
        .values()
        .filter(|s| {
            s.machine_id == machine
                && s.lifecycle == "running"
                && (s.agent_epoch != epoch || !sessions.iter().any(|n| n.id == s.id))
        })
        .cloned()
        .collect();
    for mut session in lost {
        session.lifecycle = "lost".into();
        save_session(app, &session).await?;
    }
    for session in sessions {
        save_session(app, &session).await?;
    }
    Ok(())
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
    if let Ok(event) = app.inventory(&owner).await {
        if send(&mut socket, &event).await.is_err() {
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
            event=rx.recv()=>{let Some(event)=event else{break};if send(&mut socket,&event).await.is_err(){break;}}
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
