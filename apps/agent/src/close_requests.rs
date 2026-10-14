use anyhow::Result;
use ergent_protocol::{AgentEvent, SessionInfo};
use std::sync::Arc;
use tokio::{sync::Semaphore, task::JoinSet};

const MAX_PENDING: usize = 32;

pub fn limit() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(MAX_PENDING))
}

pub fn queue(
    tasks: &mut JoinSet<AgentEvent>,
    operation_id: String,
    limit: &Arc<Semaphore>,
    close: impl FnOnce() -> Result<SessionInfo> + Send + 'static,
) -> Option<AgentEvent> {
    // Signaling can block inside a platform PTY implementation. Do not await it in
    // the connection loop, and do not let repeated requests create unlimited work.
    let Ok(permit) = limit.clone().try_acquire_owned() else {
        return Some(AgentEvent::Operation {
            operation_id,
            session: None,
            error: Some("关闭请求过多，请稍后检查会话状态".into()),
        });
    };
    tasks.spawn_blocking(move || {
        let _permit = permit;
        match close() {
            Ok(session) => AgentEvent::Operation {
                operation_id,
                session: Some(session),
                error: None,
            },
            Err(error) => AgentEvent::Operation {
                operation_id,
                session: None,
                error: Some(error.to_string()),
            },
        }
    });
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[tokio::test]
    async fn stalled_close_does_not_block_other_requests_or_runtime() {
        let mut tasks = JoinSet::new();
        let limit = limit();
        let (release, blocked) = mpsc::channel::<()>();
        assert!(queue(&mut tasks, "blocked".into(), &limit, move || {
            let _ = blocked.recv();
            anyhow::bail!("released")
        })
        .is_none());
        assert!(queue(&mut tasks, "fast".into(), &limit, || anyhow::bail!(
            "fast failure"
        ))
        .is_none());
        let result = tokio::time::timeout(Duration::from_secs(2), tasks.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(result, AgentEvent::Operation { operation_id, error: Some(_), .. } if operation_id == "fast")
        );
        // The timer and connection's select branches remain schedulable.
        tokio::time::sleep(Duration::from_millis(1)).await;
        drop(release);
        tasks.join_next().await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn outstanding_closes_are_bounded() {
        let mut tasks = JoinSet::new();
        let limit = limit();
        let (release, blocked) = mpsc::channel::<()>();
        let blocked = std::sync::Arc::new(std::sync::Mutex::new(blocked));
        for n in 0..MAX_PENDING {
            let blocked = blocked.clone();
            assert!(queue(&mut tasks, n.to_string(), &limit, move || {
                let _ = blocked.lock().unwrap().recv();
                anyhow::bail!("released")
            })
            .is_none());
        }
        assert!(matches!(
            queue(&mut tasks, "overflow".into(), &limit, || panic!(
                "must not run"
            )),
            Some(AgentEvent::Operation { error: Some(_), .. })
        ));
        drop(release);
        while let Some(task) = tasks.join_next().await {
            task.unwrap();
        }
    }
}
