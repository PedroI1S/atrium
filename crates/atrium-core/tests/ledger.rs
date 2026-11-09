//! M1-03: approval ledger (contract §5, D-046). Un-ignore each test as you
//! make it pass; none may stay ignored at merge. M1-13 adds adversarial cases.

#[test]
#[ignore = "M1-03"]
fn allow_once_redeems_exactly_once() {
    unimplemented!()
}

#[test]
#[ignore = "M1-03"]
fn redeem_rejects_changed_input() {
    unimplemented!()
}

#[test]
#[ignore = "M1-03"]
fn resolve_after_turn_end_is_expired_and_never_redeemable() {
    unimplemented!()
}

#[test]
#[ignore = "M1-03"]
fn resolve_for_older_incarnation_is_stale() {
    unimplemented!()
}

#[test]
#[ignore = "M1-03"]
fn second_resolve_from_another_window_is_rejected() {
    unimplemented!()
}

#[test]
#[ignore = "M1-03"]
fn deny_or_cancel_never_creates_a_grant() {
    unimplemented!()
}

#[test]
#[ignore = "M1-03"]
fn daemon_startup_expires_everything_pending_or_approved() {
    unimplemented!()
}

#[test]
#[ignore = "M1-03"]
fn remember_creates_rule_atomically_and_never_revives_an_expired_request() {
    unimplemented!()
}
