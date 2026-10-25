//! The "take over" flag and the gate that makes it safe.
//!
//! One side drives the page at a time: the agent (default) or the user. A bare flag is not
//! enough: an agent action that was admitted a moment before the user took over would keep
//! issuing CDP commands afterwards (several awaits later), interleaving with the user's input.
//!
//! So every input path holds a *turn* for its whole duration:
//! * `agent_enter()` / `user_enter()` check the flag **under** a shared read guard and return a
//!   turn that holds it. Multi-step actions call `AgentTurn::checkpoint()` between CDP commands
//!   and stop as soon as ownership has been revoked.
//! * `set_user_control()` first bumps the epoch (which makes every in-flight agent checkpoint
//!   fail), then takes the write side of the gate, which waits until all admitted turns have
//!   finished, flips the flag, and only then returns. New turns queue behind the pending writer.
//!
//! When `set_user_control(true)` returns, no agent action is running or can still dispatch.
//! In the real product this lives in the daemon, next to the browser.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::{RwLock, RwLockReadGuard};

#[derive(Default)]
pub struct Control {
    user_has_control: AtomicBool,
    /// Bumped at the start of every ownership change; turns remember the one they entered under.
    epoch: AtomicU64,
    gate: RwLock<()>,
}

/// An admitted agent action. Hold it for the whole action.
pub struct AgentTurn<'a> {
    _guard: RwLockReadGuard<'a, ()>,
    control: &'a Control,
    epoch: u64,
}

impl AgentTurn<'_> {
    /// Call between CDP commands. `Err` means the user took over: stop (after releasing anything
    /// the action itself pressed).
    pub fn checkpoint(&self) -> Result<(), String> {
        if self.control.epoch.load(Ordering::SeqCst) == self.epoch {
            Ok(())
        } else {
            Err("agent input cancelled: the user took over".into())
        }
    }
}

/// An admitted user input (mouse, key, IME, navigation).
pub struct UserTurn<'a>(#[allow(dead_code)] RwLockReadGuard<'a, ()>);

impl Control {
    pub fn user_has_control(&self) -> bool {
        self.user_has_control.load(Ordering::SeqCst)
    }

    /// Admits an agent action, or refuses it while the user has control.
    pub async fn agent_enter(&self) -> Result<AgentTurn<'_>, String> {
        let guard = self.gate.read().await;
        if self.user_has_control() {
            return Err("agent input paused: the user has taken over".into());
        }
        Ok(AgentTurn { _guard: guard, control: self, epoch: self.epoch.load(Ordering::SeqCst) })
    }

    /// Admits a user input, or refuses it while the agent is driving.
    pub async fn user_enter(&self) -> Result<UserTurn<'_>, String> {
        let guard = self.gate.read().await;
        if self.user_has_control() { Ok(UserTurn(guard)) } else { Err("the agent is driving; take over first".into()) }
    }

    /// Switches the driver. Completes only after every turn admitted under the old owner has
    /// finished or been cancelled at its next checkpoint.
    pub async fn set_user_control(&self, on: bool) {
        self.epoch.fetch_add(1, Ordering::SeqCst); // revoke: running agent turns stop at their next checkpoint
        let _drained = self.gate.write().await; // wait for admitted turns, block new ones
        self.user_has_control.store(on, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn exactly_one_side_may_drive() {
        let c = Control::default();
        assert!(c.agent_enter().await.is_ok());
        assert!(c.user_enter().await.is_err());
        c.set_user_control(true).await;
        assert!(c.agent_enter().await.is_err());
        assert!(c.user_enter().await.is_ok());
        c.set_user_control(false).await;
        assert!(c.agent_enter().await.is_ok());
        assert!(c.user_enter().await.is_err());
    }

    /// The bug this guards against: an agent action admitted before the take-over kept running
    /// (and issuing commands) after `set_user_control(true)` returned.
    #[tokio::test]
    async fn take_over_waits_for_the_running_agent_action_and_cancels_it() {
        let c = Arc::new(Control::default());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let steps = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));

        let agent = tokio::spawn({
            let (c, steps) = (c.clone(), steps.clone());
            async move {
                let turn = c.agent_enter().await.unwrap();
                started_tx.send(()).unwrap();
                steps.lock().unwrap().push("step 1");
                loop {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    if turn.checkpoint().is_err() {
                        steps.lock().unwrap().push("cancelled");
                        return; // the turn (and its read guard) is released here
                    }
                    steps.lock().unwrap().push("step n");
                }
            }
        });
        started_rx.await.unwrap();
        c.set_user_control(true).await;
        // The take-over has returned: the agent must already be finished, not still running.
        assert!(agent.is_finished(), "take over returned while the agent action was still in flight");
        let n = steps.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(steps.lock().unwrap().len(), n, "agent kept dispatching after the take-over");
        assert_eq!(*steps.lock().unwrap().last().unwrap(), "cancelled");
        assert!(c.user_has_control());
    }

    #[tokio::test]
    async fn new_agent_work_queued_during_a_take_over_is_refused() {
        let c = Arc::new(Control::default());
        let turn = c.agent_enter().await.unwrap();
        let switch = tokio::spawn({
            let c = c.clone();
            async move { c.set_user_control(true).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!switch.is_finished(), "must wait for the admitted turn");
        // A late agent call queues behind the writer; once the switch completes it is refused.
        let late = tokio::spawn({
            let c = c.clone();
            async move { c.agent_enter().await.map(|_| ()) }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(turn.checkpoint().is_err());
        drop(turn);
        switch.await.unwrap();
        assert!(late.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn hand_back_waits_for_user_input_in_flight() {
        let c = Arc::new(Control::default());
        c.set_user_control(true).await;
        let turn = c.user_enter().await.unwrap();
        let back = tokio::spawn({
            let c = c.clone();
            async move { c.set_user_control(false).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!back.is_finished(), "hand-back must wait for the in-flight user input");
        drop(turn);
        back.await.unwrap();
        assert!(!c.user_has_control());
    }
}
