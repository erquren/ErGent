use ergent_protocol::{ToolEvent, ToolEventKind as K, ToolState as S, ToolStatus};

pub fn reduce(previous: Option<&ToolStatus>, event: &ToolEvent) -> Option<ToolStatus> {
    if let Some(old) = previous {
        if event.occurred_at < old.occurred_at {
            return None;
        }
        if event.run_id != old.run_id || event.provider != old.provider {
            if event.kind != K::SessionStarted {
                return None;
            }
        } else {
            if event.kind == K::SessionStarted {
                return None;
            }
            if matches!(old.state, S::Stopped | S::Failed) {
                return None;
            }
            let notify_only_next = event.kind == K::TurnCompleted
                && old.state == S::Completed
                && event.turn_id != old.turn_id;
            if event.kind == K::TurnStarted && event.turn_id == old.turn_id {
                return None;
            }
            if !notify_only_next
                && !matches!(
                    event.kind,
                    K::TurnStarted | K::SessionStopped | K::SessionFailed
                )
            {
                if old.turn_id.is_some() && event.turn_id != old.turn_id {
                    return None;
                }
                // Late tool callbacks must not revive a finished turn.
                if matches!(old.state, S::Completed | S::Interrupted)
                    && matches!(
                        event.kind,
                        K::Activity | K::ApprovalRequested | K::TurnCompleted | K::TurnInterrupted
                    )
                {
                    return None;
                }
            }
        }
    } else if event.kind != K::SessionStarted {
        return None;
    }
    let state = match event.kind {
        K::SessionStarted => S::Idle,
        K::TurnStarted | K::Activity => S::Running,
        K::ApprovalRequested => S::WaitingForApproval,
        K::TurnCompleted => S::Completed,
        K::TurnInterrupted => S::Interrupted,
        K::SessionStopped => S::Stopped,
        K::SessionFailed => S::Failed,
    };
    if previous.is_some_and(|old| {
        event.kind == K::Activity && old.state == state && old.turn_id == event.turn_id
    }) {
        return None;
    }
    Some(ToolStatus {
        provider: event.provider.clone(),
        run_id: event.run_id.clone(),
        turn_id: event.turn_id.clone(),
        state,
        event_id: event.event_id.clone(),
        occurred_at: event.occurred_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: K, turn: Option<&str>, time: u64) -> ToolEvent {
        ToolEvent {
            version: 1,
            event_id: format!("event-{time}"),
            provider: "codex".into(),
            run_id: "run".into(),
            turn_id: turn.map(str::to_owned),
            kind,
            occurred_at: time,
        }
    }
    #[test]
    fn rejects_late_and_foreign_events_and_accepts_new_turns() {
        let idle = reduce(None, &event(K::SessionStarted, None, 1)).unwrap();
        let running = reduce(Some(&idle), &event(K::TurnStarted, Some("turn-1"), 2)).unwrap();
        let waiting = reduce(
            Some(&running),
            &event(K::ApprovalRequested, Some("turn-1"), 3),
        )
        .unwrap();
        assert_eq!(waiting.state, S::WaitingForApproval);
        let done = reduce(Some(&waiting), &event(K::TurnCompleted, Some("turn-1"), 4)).unwrap();
        assert!(reduce(Some(&done), &event(K::Activity, Some("turn-1"), 5)).is_none());
        let next = reduce(Some(&done), &event(K::TurnStarted, Some("turn-2"), 6)).unwrap();
        assert!(reduce(Some(&next), &event(K::TurnCompleted, Some("turn-1"), 7)).is_none());
        let mut foreign = event(K::TurnCompleted, Some("turn-2"), 8);
        foreign.run_id = "other".into();
        assert!(reduce(Some(&next), &foreign).is_none());
        assert!(reduce(Some(&next), &event(K::TurnCompleted, Some("turn-2"), 2)).is_none());
        let stopped = reduce(Some(&next), &event(K::SessionStopped, None, 9)).unwrap();
        assert!(reduce(Some(&stopped), &event(K::TurnStarted, Some("turn-3"), 10)).is_none());
    }
}
