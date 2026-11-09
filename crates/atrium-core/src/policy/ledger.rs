//! The approval ledger (contract §5, D-046): the single authority over
//! approvals and questions. It owns every state transition; the store only
//! persists them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use atrium_agents::{ApprovalTicket, BoxFuture, ToolCheck};
use atrium_protocol::agent::ApprovalDecision;
use atrium_protocol::limits::MAX_PENDING_REQUESTS;
use atrium_protocol::policy::{RememberScope, Rule, RuleDecision, RuleId, RuleOrigin, RuleScope};
use atrium_protocol::thread::ThreadId;
use sha2::{Digest, Sha256};

use super::{Ledger, input_sha256};
use crate::{ApprovalLedger, Binding, LedgerError, RequestKey, RequestStore};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The error for a transition that found `found` instead of what it needed.
fn refused(found: Option<&str>) -> LedgerError {
    match found {
        None => LedgerError::Unknown,
        Some("expired" | "cancelled") => LedgerError::Expired,
        Some(_) => LedgerError::AlreadyResolved,
    }
}

impl Ledger {
    pub fn new(store: Arc<dyn RequestStore>) -> Self {
        let salt = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Self {
            store,
            open_lock: tokio::sync::Mutex::new(()),
            counter: AtomicU64::new(1),
            salt,
        }
    }

    fn new_request_id(&self) -> String {
        format!(
            "req_{:x}_{}",
            self.salt,
            self.counter.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// The stale / unknown-thread check shared by every entry point.
    async fn check_incarnation(
        &self,
        key_thread: &ThreadId,
        incarnation: u32,
    ) -> Result<(), LedgerError> {
        match self.store.current_incarnation(key_thread).await {
            Ok(None) => Err(LedgerError::Unknown),
            Ok(Some(cur)) if incarnation < cur => Err(LedgerError::StaleIncarnation),
            Ok(Some(cur)) if incarnation > cur => Err(LedgerError::Unknown),
            Ok(Some(_)) => Ok(()),
            Err(_) => Err(LedgerError::Store),
        }
    }

    async fn open(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        binding: Option<Binding>,
        turn: Option<u32>,
    ) -> Result<ApprovalTicket, LedgerError> {
        self.check_incarnation(thread, incarnation).await?;
        // One opener at a time, so the limit can't be raced past.
        let _guard = self.open_lock.lock().await;
        let open = self
            .store
            .open_count(thread, incarnation)
            .await
            .map_err(|_| LedgerError::Store)?;
        if open >= MAX_PENDING_REQUESTS as u64 {
            return Err(LedgerError::TooMany);
        }
        let key = RequestKey {
            thread: thread.clone(),
            incarnation,
            request_id: self.new_request_id(),
        };
        let rememberable = binding
            .as_ref()
            .is_some_and(|b| b.remember_matcher.is_some());
        match &binding {
            Some(b) => self.store.open_approval(&key, b).await,
            None => self.store.open_question(&key, turn).await,
        }
        .map_err(|_| LedgerError::Store)?;
        Ok(ApprovalTicket {
            request_id: key.request_id,
            rememberable,
        })
    }

    fn rule_id(key: &RequestKey) -> RuleId {
        let mut h = Sha256::new();
        h.update(key.thread.0.as_bytes());
        h.update([0]);
        h.update(key.incarnation.to_le_bytes());
        h.update(key.request_id.as_bytes());
        let d = h.finalize();
        let hex: String = d.iter().take(10).map(|b| format!("{b:02x}")).collect();
        RuleId(format!("rule_{hex}"))
    }
}

impl ApprovalLedger for Ledger {
    fn open_approval(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        binding: Binding,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        let thread = thread.clone();
        Box::pin(async move { self.open(&thread, incarnation, Some(binding), None).await })
    }

    fn open_question(
        &self,
        thread: &ThreadId,
        incarnation: u32,
        turn: Option<u32>,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        let thread = thread.clone();
        Box::pin(async move { self.open(&thread, incarnation, None, turn).await })
    }

    fn resolve(
        &self,
        key: &RequestKey,
        decision: &ApprovalDecision,
        remember: Option<RememberScope>,
    ) -> BoxFuture<'_, Result<Option<RuleId>, LedgerError>> {
        let key = key.clone();
        let decision = decision.clone();
        Box::pin(async move {
            self.check_incarnation(&key.thread, key.incarnation).await?;
            match decision {
                ApprovalDecision::Deny { .. } => {
                    let found = self
                        .store
                        .transition(&key, &["pending"], "denied")
                        .await
                        .map_err(|_| LedgerError::Store)?;
                    match found.as_deref() {
                        Some("pending") => Ok(None),
                        other => Err(refused(other)),
                    }
                }
                ApprovalDecision::Allow => {
                    let rule = match remember {
                        None => None,
                        Some(scope) => {
                            let found = self
                                .store
                                .binding(&key)
                                .await
                                .map_err(|_| LedgerError::Store)?;
                            let Some((status, binding)) = found else {
                                return Err(LedgerError::Unknown);
                            };
                            if status != "pending" {
                                return Err(refused(Some(&status)));
                            }
                            binding.remember_matcher.map(|matcher| Rule {
                                id: Self::rule_id(&key),
                                scope: match scope {
                                    RememberScope::Thread => RuleScope::Thread {
                                        thread_id: key.thread.clone(),
                                    },
                                    RememberScope::Home => RuleScope::Home,
                                    RememberScope::Global => RuleScope::Global,
                                },
                                tool_kind: binding.tool_kind,
                                tool_name: Some(binding.tool_name.clone()),
                                matcher,
                                path_scope: None,
                                decision: RuleDecision::Allow,
                                origin: RuleOrigin::Approval {
                                    request_id: key.request_id.clone(),
                                },
                                created_at_ms: now_ms(),
                            })
                        }
                    };
                    let id = rule.as_ref().map(|r| r.id.clone());
                    let found = self
                        .store
                        .approve_with_rule(&key, rule)
                        .await
                        .map_err(|_| LedgerError::Store)?;
                    match found.as_deref() {
                        Some("pending") => Ok(id),
                        other => Err(refused(other)),
                    }
                }
            }
        })
    }

    fn answer(&self, key: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        let key = key.clone();
        Box::pin(async move {
            self.check_incarnation(&key.thread, key.incarnation).await?;
            let found = self
                .store
                .transition(&key, &["pending"], "answered")
                .await
                .map_err(|_| LedgerError::Store)?;
            match found.as_deref() {
                Some("pending") => Ok(()),
                other => Err(refused(other)),
            }
        })
    }

    fn redeem(
        &self,
        key: &RequestKey,
        check: &ToolCheck,
    ) -> BoxFuture<'_, Result<(), LedgerError>> {
        let key = key.clone();
        let check = check.clone();
        Box::pin(async move {
            self.check_incarnation(&key.thread, key.incarnation).await?;
            let found = self
                .store
                .binding(&key)
                .await
                .map_err(|_| LedgerError::Store)?;
            let Some((status, binding)) = found else {
                return Err(LedgerError::Unknown);
            };
            match status.as_str() {
                "approved" => {}
                // not resolved yet: nothing to redeem
                "pending" => return Err(LedgerError::Unknown),
                other => return Err(refused(Some(other))),
            }
            if check.turn != binding.turn
                || check.name != binding.tool_name
                || check.kind != binding.tool_kind
                || input_sha256(&check) != binding.input_sha256
            {
                return Err(LedgerError::Mismatch);
            }
            // The compare-and-set is the single point where exactly one
            // caller wins.
            let found = self
                .store
                .transition(&key, &["approved"], "redeemed")
                .await
                .map_err(|_| LedgerError::Store)?;
            match found.as_deref() {
                Some("approved") => Ok(()),
                other => Err(refused(other)),
            }
        })
    }

    fn withdraw(&self, key: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        let key = key.clone();
        Box::pin(async move {
            let found = self
                .store
                .transition(&key, &["pending", "approved"], "cancelled")
                .await
                .map_err(|_| LedgerError::Store)?;
            match found.as_deref() {
                Some("pending" | "approved") => Ok(()),
                other => Err(refused(other)),
            }
        })
    }

    fn expire_turn(&self, thread: &ThreadId, incarnation: u32, turn: u32) -> BoxFuture<'_, u64> {
        let thread = thread.clone();
        Box::pin(async move {
            self.store
                .expire(Some(&thread), Some(incarnation), Some(turn))
                .await
                .unwrap_or(0)
        })
    }

    fn expire_incarnation(&self, thread: &ThreadId, incarnation: u32) -> BoxFuture<'_, u64> {
        let thread = thread.clone();
        Box::pin(async move {
            self.store
                .expire(Some(&thread), Some(incarnation), None)
                .await
                .unwrap_or(0)
        })
    }

    fn expire_all(&self) -> BoxFuture<'_, u64> {
        Box::pin(async move { self.store.expire(None, None, None).await.unwrap_or(0) })
    }
}
