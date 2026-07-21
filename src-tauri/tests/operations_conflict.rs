use archi_backend_lib::models::ConflictDecision;
use archi_backend_lib::operations::OperationRegistry;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

#[test]
fn resolve_delivers_decision_to_waiter() {
    let reg = std::sync::Arc::new(OperationRegistry::default());
    reg.start("op-1").unwrap();
    let reg_w = reg.clone();
    let handle = thread::spawn(move || reg_w.wait_for_conflict_decision("op-1", "c-1").unwrap());
    thread::sleep(Duration::from_millis(20));
    reg.resolve_conflict("op-1", "c-1", ConflictDecision::Skip, false)
        .unwrap();
    assert_eq!(handle.join().unwrap(), ConflictDecision::Skip);
    reg.finish("op-1");
}

#[test]
fn apply_to_all_stores_policy() {
    let reg = std::sync::Arc::new(OperationRegistry::default());
    reg.start("op-2").unwrap();
    let reg_w = reg.clone();
    let h = thread::spawn(move || reg_w.wait_for_conflict_decision("op-2", "c-1").unwrap());
    thread::sleep(Duration::from_millis(20));
    reg.resolve_conflict("op-2", "c-1", ConflictDecision::Overwrite, true)
        .unwrap();
    assert_eq!(h.join().unwrap(), ConflictDecision::Overwrite);
    assert_eq!(
        reg.peek_apply_policy("op-2"),
        Some(ConflictDecision::Overwrite)
    );
    reg.finish("op-2");
}

#[test]
fn cancel_unblocks_waiter_with_cancel() {
    let reg = std::sync::Arc::new(OperationRegistry::default());
    reg.start("op-3").unwrap();
    let reg_w = reg.clone();
    let h = thread::spawn(move || reg_w.wait_for_conflict_decision("op-3", "c-1").unwrap());
    thread::sleep(Duration::from_millis(20));
    assert!(reg.cancel("op-3"));
    assert_eq!(h.join().unwrap(), ConflictDecision::Cancel);
    reg.finish("op-3");
}

#[test]
fn resolve_cancel_sets_cancelled_flag() {
    let reg = std::sync::Arc::new(OperationRegistry::default());
    let state = reg.start("op-cancel-resolve").unwrap();
    let reg_w = reg.clone();
    let h = thread::spawn(move || {
        reg_w
            .wait_for_conflict_decision("op-cancel-resolve", "c-1")
            .unwrap()
    });
    thread::sleep(Duration::from_millis(20));
    reg.resolve_conflict("op-cancel-resolve", "c-1", ConflictDecision::Cancel, false)
        .unwrap();
    assert_eq!(h.join().unwrap(), ConflictDecision::Cancel);
    assert!(state.cancelled.load(Ordering::Relaxed));
    reg.finish("op-cancel-resolve");
}

#[test]
fn start_returns_shared_cancel_flag() {
    let reg = OperationRegistry::default();
    let state = reg.start("op-4").unwrap();
    assert!(reg.cancel("op-4"));
    assert!(state.cancelled.load(Ordering::Relaxed));
    reg.finish("op-4");
    assert!(!reg.cancel("op-4"));
}
