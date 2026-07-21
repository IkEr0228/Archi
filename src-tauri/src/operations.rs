use crate::models::ConflictDecision;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

struct PendingConflict {
    conflict_id: String,
    tx: mpsc::Sender<ConflictDecision>,
}

pub struct OperationState {
    pub cancelled: Arc<AtomicBool>,
    apply_policy: Mutex<Option<ConflictDecision>>,
    waiter: Mutex<Option<PendingConflict>>,
    /// Receiver half of the conflict decision channel, held until `recv_conflict_decision`.
    decision_rx: Mutex<Option<(String, mpsc::Receiver<ConflictDecision>)>>,
}

impl Default for OperationState {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            apply_policy: Mutex::new(None),
            waiter: Mutex::new(None),
            decision_rx: Mutex::new(None),
        }
    }
}

#[derive(Clone, Default)]
pub struct OperationRegistry(Arc<Mutex<HashMap<String, Arc<OperationState>>>>);

impl OperationRegistry {
    fn get_state(&self, id: &str) -> Result<Arc<OperationState>, String> {
        self.0
            .lock()
            .map_err(|_| "Operation registry is unavailable.".to_string())?
            .get(id)
            .cloned()
            .ok_or_else(|| format!("Unknown operation ID: {id}"))
    }

    pub fn start(&self, id: &str) -> Result<Arc<OperationState>, String> {
        if id.is_empty() {
            return Err("Operation ID is empty.".into());
        }

        let mut active = self
            .0
            .lock()
            .map_err(|_| "Operation registry is unavailable.")?;
        if active.contains_key(id) {
            return Err("Operation ID is already active.".into());
        }

        let state = Arc::new(OperationState::default());
        active.insert(id.into(), state.clone());
        Ok(state)
    }

    pub fn cancel(&self, id: &str) -> bool {
        let state = match self
            .0
            .lock()
            .ok()
            .and_then(|active| active.get(id).cloned())
        {
            Some(state) => state,
            None => return false,
        };

        state.cancelled.store(true, Ordering::Relaxed);
        if let Ok(mut waiter) = state.waiter.lock() {
            if let Some(pending) = waiter.take() {
                let _ = pending.tx.send(ConflictDecision::Cancel);
            }
        }
        true
    }

    pub fn finish(&self, id: &str) {
        if let Ok(mut active) = self.0.lock() {
            if let Some(state) = active.remove(id) {
                if let Ok(mut waiter) = state.waiter.lock() {
                    if let Some(pending) = waiter.take() {
                        let _ = pending.tx.send(ConflictDecision::Cancel);
                    }
                }
                if let Ok(mut decision_rx) = state.decision_rx.lock() {
                    let _ = decision_rx.take();
                }
            }
        }
    }

    /// Register a pending conflict waiter **before** emitting the UI event.
    /// Pair with [`recv_conflict_decision`] after emit.
    pub fn install_conflict_waiter(
        &self,
        operation_id: &str,
        conflict_id: &str,
    ) -> Result<(), String> {
        let state = self.get_state(operation_id)?;
        let (tx, rx) = mpsc::channel();

        {
            let mut decision_rx = state
                .decision_rx
                .lock()
                .map_err(|_| "Operation decision receiver is unavailable.")?;
            if decision_rx.is_some() {
                return Err("A conflict is already waiting for a decision.".into());
            }

            if state.cancelled.load(Ordering::Relaxed) {
                let _ = tx.send(ConflictDecision::Cancel);
                *decision_rx = Some((conflict_id.to_string(), rx));
                return Ok(());
            }
        }

        {
            let mut waiter = state
                .waiter
                .lock()
                .map_err(|_| "Operation waiter is unavailable.")?;
            if waiter.is_some() {
                return Err("A conflict is already waiting for a decision.".into());
            }
            *waiter = Some(PendingConflict {
                conflict_id: conflict_id.to_string(),
                tx,
            });
        }

        {
            let mut decision_rx = state
                .decision_rx
                .lock()
                .map_err(|_| "Operation decision receiver is unavailable.")?;
            *decision_rx = Some((conflict_id.to_string(), rx));
        }

        // Avoid a race where cancel landed after the first check but before the waiter was set.
        if state.cancelled.load(Ordering::Relaxed) {
            if let Ok(mut waiter) = state.waiter.lock() {
                if let Some(pending) = waiter.take() {
                    let _ = pending.tx.send(ConflictDecision::Cancel);
                }
            }
        }

        Ok(())
    }

    /// Block until a decision is delivered for a waiter installed via [`install_conflict_waiter`].
    pub fn recv_conflict_decision(
        &self,
        operation_id: &str,
        conflict_id: &str,
    ) -> Result<ConflictDecision, String> {
        let state = self.get_state(operation_id)?;
        let rx = {
            let mut decision_rx = state
                .decision_rx
                .lock()
                .map_err(|_| "Operation decision receiver is unavailable.")?;
            let (id, rx) = decision_rx
                .take()
                .ok_or_else(|| "No pending conflict for this operation.".to_string())?;
            if id != conflict_id {
                *decision_rx = Some((id, rx));
                return Err("Conflict ID does not match the pending conflict.".into());
            }
            rx
        };

        rx.recv()
            .map_err(|_| "Conflict decision channel closed.".to_string())
    }

    /// Install waiter and block until a decision (tests / callers that emit separately elsewhere).
    pub fn wait_for_conflict_decision(
        &self,
        operation_id: &str,
        conflict_id: &str,
    ) -> Result<ConflictDecision, String> {
        let state = self.get_state(operation_id)?;
        if state.cancelled.load(Ordering::Relaxed) {
            return Ok(ConflictDecision::Cancel);
        }

        self.install_conflict_waiter(operation_id, conflict_id)?;
        self.recv_conflict_decision(operation_id, conflict_id)
    }

    pub fn resolve_conflict(
        &self,
        operation_id: &str,
        conflict_id: &str,
        decision: ConflictDecision,
        apply_to_all: bool,
    ) -> Result<(), String> {
        let state = self.get_state(operation_id)?;
        let pending = {
            let mut waiter = state
                .waiter
                .lock()
                .map_err(|_| "Operation waiter is unavailable.")?;
            let pending = waiter
                .take()
                .ok_or_else(|| "No pending conflict for this operation.".to_string())?;
            if pending.conflict_id != conflict_id {
                *waiter = Some(pending);
                return Err("Conflict ID does not match the pending conflict.".into());
            }
            pending
        };

        if apply_to_all
            && matches!(
                decision,
                ConflictDecision::Overwrite | ConflictDecision::Skip | ConflictDecision::Rename
            )
        {
            let mut policy = state
                .apply_policy
                .lock()
                .map_err(|_| "Operation policy is unavailable.")?;
            *policy = Some(decision);
        }

        // Cancel via conflict dialog is equivalent to cancel_operation for the op.
        if matches!(decision, ConflictDecision::Cancel) {
            state.cancelled.store(true, Ordering::Relaxed);
        }

        pending
            .tx
            .send(decision)
            .map_err(|_| "Failed to deliver conflict decision.".to_string())?;
        Ok(())
    }

    pub fn take_apply_policy(&self, operation_id: &str) -> Option<ConflictDecision> {
        let state = self.get_state(operation_id).ok()?;
        state
            .apply_policy
            .lock()
            .ok()
            .and_then(|mut policy| policy.take())
    }

    pub fn peek_apply_policy(&self, operation_id: &str) -> Option<ConflictDecision> {
        let state = self.get_state(operation_id).ok()?;
        state.apply_policy.lock().ok().and_then(|policy| *policy)
    }
}
