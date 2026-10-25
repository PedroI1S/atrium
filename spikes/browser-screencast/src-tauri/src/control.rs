//! The "take over" flag.
//!
//! One flag decides who may drive the page: the agent (default) or the user. Every input
//! command goes through `require_user` or `require_agent`, so a stray agent call cannot
//! interleave with a user who is typing a password, and the user's clicks cannot fight a
//! running agent. In the real product this lives in the daemon, next to the browser.

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct Control {
    user_has_control: AtomicBool,
}

impl Control {
    pub fn set_user_control(&self, on: bool) {
        self.user_has_control.store(on, Ordering::SeqCst);
    }

    pub fn user_has_control(&self) -> bool {
        self.user_has_control.load(Ordering::SeqCst)
    }

    /// Input that comes from the pane (mouse, wheel, keyboard, IME, nav bar).
    pub fn require_user(&self) -> Result<(), String> {
        if self.user_has_control() { Ok(()) } else { Err("the agent is driving; take over first".into()) }
    }

    /// Input that comes from the agent (the browser MCP tools, later).
    pub fn require_agent(&self) -> Result<(), String> {
        if self.user_has_control() { Err("agent input paused: the user has taken over".into()) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_side_may_drive() {
        let c = Control::default();
        assert!(c.require_agent().is_ok());
        assert!(c.require_user().is_err());
        c.set_user_control(true);
        assert!(c.require_agent().is_err());
        assert!(c.require_user().is_ok());
        c.set_user_control(false);
        assert!(c.require_agent().is_ok());
        assert!(c.require_user().is_err());
    }
}
