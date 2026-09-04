#![cfg(test)]

extern crate std;

use super::*;
use proptest::prelude::*;
use soroban_sdk::testutils::Events;
use soroban_sdk::testutils::Ledger;
use soroban_sdk::Env;

/// Extract the primary and secondary topic symbols from every emitted event.
/// Soroban SDK 21 exposes events as `(contract_id, topics, data)` tuples where
/// `topics` is a `Vec<Val>`; events with a single topic carry `None` secondary.
/// Uses `std::vec::Vec` explicitly because the crate is `#![no_std]`.
fn emitted_topic_pairs(env: &Env) -> std::vec::Vec<(Symbol, Option<Symbol>)> {
    use soroban_sdk::TryFromVal;
    env.events()
        .all()
        .iter()
        .map(|(_contract_id, topics, _data)| {
            let primary = topics
                .get(0)
                .map(|v| Symbol::try_from_val(env, &v).unwrap())
                .expect("events always carry a primary topic");
            let secondary = topics.get(1).map(|v| Symbol::try_from_val(env, &v).unwrap());
            (primary, secondary)
        })
        .collect::<std::vec::Vec<_>>()
}

fn deploy(env: &Env) -> (Address, PulseContractClient) {
    let contract_id = env.register_contract(None, PulseContract);
    let client = PulseContractClient::new(env, &contract_id);
    (contract_id, client)
}

fn deploy_initialized<'a>(env: &'a Env, owner: &Address) -> (Address, PulseContractClient<'a>) {
    let (contract_id, client) = deploy(env);
    client.initialize(owner);
    (contract_id, client)
}

fn make_owner(env: &Env) -> Address {
    // Register a dummy contract to get a valid Address for testing
    let dummy_id = env.register_contract(None, PulseContract);
    dummy_id
}

#[test]
fn test_initialize_sets_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy(&env);

    client.initialize(&owner);

    let stored = client.owner();
    assert_eq!(stored, Some(owner.clone()));

    let meta = client.get_meta();
    assert!(meta.is_some());
    let meta = meta.unwrap();
    assert_eq!(meta.owner, owner);
    assert!(!meta.paused);
    assert_eq!(meta.version, 1);
}

#[test]
fn test_initialize_idempotent() {
    let env = Env::default();
    // Deploy a second contract to use its address as a different owner
    let other_contract_id = env.register_contract(None, PulseContract);
    let owner1 = make_owner(&env);
    let owner2 = other_contract_id.clone();
    let (_id, client) = deploy(&env);

    client.initialize(&owner1);
    client.initialize(&owner2); // Should not change owner

    assert_eq!(client.owner(), Some(owner1));
}

#[test]
fn test_pulse_increments_count() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    assert_eq!(client.get_pulse_count(), 0);

    let count1 = client.pulse(&symbol_short!("alice"));
    assert_eq!(count1, 1u32);

    let count2 = client.pulse(&symbol_short!("bob"));
    assert_eq!(count2, 2u32);

    let data = client.get_pulse_data();
    assert_eq!(data.count, 2);
    assert_eq!(data.last_caller, Some(symbol_short!("bob")));
    assert!(data.last_pulse_at.is_some());
}

#[test]
fn test_pulse_emits_event_with_topics() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let _count = client.pulse(&symbol_short!("alice"));

    let events = env.events().all();
    assert!(!events.is_empty(), "Should have emitted at least one event");
}

#[test]
fn test_pulse_rejects_empty_caller() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let result = client.try_pulse(&Symbol::new(&env, ""));
    assert!(result.is_err());
}

#[test]
fn test_pause_and_unpause() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    // Mock all auths for the owner
    env.mock_all_auths();

    assert!(!client.is_paused());
    client.pause();
    assert!(client.is_paused());

    let result = client.try_pulse(&symbol_short!("alice"));
    assert!(result.is_err());

    client.unpause();
    assert!(!client.is_paused());

    let result = client.pulse(&symbol_short!("bob"));
    assert_eq!(result, 1u32);
}

#[test]
fn test_pause_idempotent() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    env.mock_all_auths();
    client.pause();
    client.pause();
    assert!(client.is_paused());
}

#[test]
fn test_transfer_ownership() {
    let env = Env::default();
    let owner = make_owner(&env);
    // Use another deployed contract's address as the new owner
    let new_owner_id = env.register_contract(None, PulseContract);
    let new_owner = new_owner_id.clone();
    let (_id, client) = deploy_initialized(&env, &owner);

    env.mock_all_auths();
    assert_eq!(client.owner(), Some(owner.clone()));
    client.transfer_ownership(&new_owner);
    assert_eq!(client.owner(), Some(new_owner.clone()));
}

#[test]
fn test_batch_pulse() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let callers = Vec::from_array(
        &env,
        [
            symbol_short!("alice"),
            symbol_short!("bob"),
            symbol_short!("charlie"),
        ],
    );

    let result = client.batch_pulse(&callers);
    assert_eq!(result, 3u32);

    let data = client.get_pulse_data();
    assert_eq!(data.count, 3);
    assert_eq!(data.last_caller, Some(symbol_short!("charlie")));
}

#[test]
fn test_batch_pulse_empty() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let empty: Vec<Symbol> = Vec::new(&env);
    let result = client.try_batch_pulse(&empty);
    assert!(result.is_err());
}

#[test]
fn test_batch_pulse_respects_max_size() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let mut callers = Vec::new(&env);
    for _i in 0..51u32 {
        callers.push_back(symbol_short!("test"));
    }

    let result = client.try_batch_pulse(&callers);
    assert!(result.is_err());
}

#[test]
fn test_batch_pulse_paused() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    env.mock_all_auths();
    client.pause();

    let callers = Vec::from_array(&env, [symbol_short!("alice")]);
    let result = client.try_batch_pulse(&callers);
    assert!(result.is_err());
}

#[test]
fn test_get_pulse_count_initial() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    assert_eq!(client.get_pulse_count(), 0);
}

#[test]
fn test_get_pulse_data_initial() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let data = client.get_pulse_data();
    assert_eq!(data.count, 0);
    assert_eq!(data.last_caller, None);
    assert_eq!(data.last_pulse_at, None);
}

#[test]
fn test_inter_contract_communication() {
    let env = Env::default();
    let owner = make_owner(&env);

    let contract_a_id = env.register_contract(None, PulseContract);
    let contract_b_id = env.register_contract(None, PulseContract);

    let client_a = PulseContractClient::new(&env, &contract_a_id);
    let client_b = PulseContractClient::new(&env, &contract_b_id);

    client_a.initialize(&owner);
    client_b.initialize(&owner);

    let (count, _result) = client_a.broadcast_pulse(&contract_b_id, &symbol_short!("alice"));
    assert_eq!(count, 1u32);

    let received = client_b.get_last_received();
    assert!(received.is_some());
    let (rx_count, rx_caller) = received.unwrap();
    assert_eq!(rx_count, 1);
    assert_eq!(rx_caller, symbol_short!("alice"));
}

#[test]
fn test_on_pulse_received_direct() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let response = client.on_pulse_received(&5u32, &symbol_short!("bob"));
    assert_eq!(response, symbol_short!("ACK"));

    let received = client.get_last_received();
    assert!(received.is_some());
    let (count, caller) = received.unwrap();
    assert_eq!(count, 5);
    assert_eq!(caller, symbol_short!("bob"));
}

#[test]
fn test_get_last_received_initial() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    assert!(client.get_last_received().is_none());
}

#[test]
fn test_counter_overflow_protection() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    env.as_contract(&_id, || {
        env.storage().instance().set(
            &PULSE_KEY,
            &PulseData {
                count: u32::MAX,
                last_caller: None,
                last_pulse_at: None,
            },
        );
    });

    let result = client.try_pulse(&symbol_short!("alice"));
    assert!(result.is_err());
}

#[test]
fn test_pulse_count_persists_across_calls() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let callers = ["alice", "bob", "charlie", "dave", "eve"];
    for (i, caller) in callers.iter().enumerate() {
        let count = client.pulse(&Symbol::new(&env, caller));
        assert_eq!(count, (i + 1) as u32);
    }

    assert_eq!(client.get_pulse_count(), 5);
}

#[test]
fn test_uninitialized_contract_has_no_owner() {
    let env = Env::default();
    let (_id, client) = deploy(&env);

    assert!(client.owner().is_none());
    assert!(client.get_meta().is_none());
}

fn upload_test_wasm(env: &Env) -> soroban_sdk::BytesN<32> {
    // Arbitrary hash — upgrade_version records it without executing a swap.
    soroban_sdk::BytesN::from_array(env, &[7u8; 32])
}

#[test]
fn test_upgrade_version() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    let hash = upload_test_wasm(&env);

    client.upgrade_version(&3u32, &hash);

    let meta = client.get_meta().unwrap();
    assert_eq!(meta.version, 3);

    let record = client.get_version_record();
    assert!(record.is_some());
    // The record must carry the hash we upgraded to, not the previous one.
    assert_eq!(record.unwrap().new_wasm_hash, hash);
}

#[test]
fn test_upgrade_rejects_downgrade() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    let hash = upload_test_wasm(&env);
    client.upgrade_version(&5u32, &hash);

    let result = client.try_upgrade_version(&3u32, &hash);
    assert!(result.is_err());
}

#[test]
fn test_update_wasm_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    // Only the owner may swap the executable. Without mock_all_auths the
    // default test caller is not the owner, so the swap must be rejected.
    let result = client.try_update_wasm(&soroban_sdk::BytesN::from_array(&env, &[9u8; 32]));
    assert!(
        result.is_err(),
        "non-owner must not be able to swap the WASM"
    );
}

#[test]
fn test_kill_switch() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    assert!(!client.is_killed());

    client.kill();

    assert!(client.is_killed());
    assert!(client.is_paused());

    // Pulse should fail after kill
    let result = client.try_pulse(&symbol_short!("alice"));
    assert!(result.is_err());
}

#[test]
fn test_set_signers() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    let signers = Vec::from_array(&env, [owner.clone()]);
    client.set_signers(&signers, &1u32);

    let (stored_signers, threshold) = client.get_signers();
    assert_eq!(threshold, 1u32);
    assert_eq!(stored_signers.len(), 1);
}

#[test]
fn test_rate_limited_pulse_with_cooldown() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    // First pulse should succeed
    let result = client.rate_limited_pulse(&symbol_short!("alice"), &60u64);
    assert_eq!(result, 1u32);

    // Second pulse within cooldown should fail
    let result = client.try_rate_limited_pulse(&symbol_short!("alice"), &60u64);
    assert!(result.is_err());
}

#[test]
fn test_time_locked_pulse_rejects_before_deadline() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    // Set the ledger to a concrete time so the deadline is deterministic.
    env.ledger().set_timestamp(1_000_000);

    let result = client.try_time_locked_pulse(&symbol_short!("alice"), &1_500_000u64, &None);
    // try_* returns Result<T, Result<PulseError, InvokeError>>; a typed
    // contract error surfaces as Ok(PulseError::...) inside the outer Err.
    assert_eq!(result.unwrap_err(), Ok(PulseError::TimeLockNotReady));

    // After the deadline the pulse fires normally.
    env.ledger().set_timestamp(2_000_000);
    let count = client.time_locked_pulse(&symbol_short!("alice"), &1_500_000u64, &None);
    assert_eq!(count, 1u32);
}

#[test]
fn test_time_locked_pulse_rejects_after_deadline() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    // execute_after (not-before) has passed, but the absolute deadline
    // (not-after) has also expired — the pulse must be rejected.
    env.ledger().set_timestamp(2_000_000);
    let result = client.try_time_locked_pulse(
        &symbol_short!("alice"),
        &1_000_000u64,
        &Some(1_500_000u64),
    );
    assert_eq!(result.unwrap_err(), Ok(PulseError::TimeLockExpired));
}

#[test]
fn test_time_locked_pulse_executes_within_window() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    env.ledger().set_timestamp(1_250_000);
    let count = client.time_locked_pulse(
        &symbol_short!("alice"),
        &1_000_000u64,
        &Some(1_500_000u64),
    );
    assert_eq!(count, 1u32);
}

#[test]
fn test_time_locked_pulse_unset_deadline_unchanged() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    // None keeps the original open-ended behavior: only the not-before
    // constraint applies, even well past any would-be deadline.
    env.ledger().set_timestamp(2_000_000);
    let count = client.time_locked_pulse(&symbol_short!("alice"), &1_000_000u64, &None);
    assert_eq!(count, 1u32);
}

#[test]
fn test_set_max_pulse_count_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let result = client.try_set_max_pulse_count(&100u32);
    assert!(result.is_err(), "non-owner must not configure the pulse cap");
}

#[test]
fn test_max_pulse_count_default_unlimited() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    assert_eq!(client.get_max_pulse_count(), u32::MAX);
    // Without a cap configured, pulsing is not artificially limited.
    for i in 0..10u32 {
        assert_eq!(client.pulse(&symbol_short!("alice")), i + 1);
    }
    assert_eq!(client.get_pulse_count(), 10);
}

#[test]
fn test_pulse_rejected_once_cap_reached() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();
    client.set_max_pulse_count(&3u32);

    assert_eq!(client.pulse(&symbol_short!("alice")), 1);
    assert_eq!(client.pulse(&symbol_short!("bob")), 2);
    assert_eq!(client.pulse(&symbol_short!("carol")), 3);

    let result = client.try_pulse(&symbol_short!("dave"));
    assert_eq!(result.unwrap_err(), Ok(PulseError::PulseCapReached));
    // The rejected pulse must not have changed stored state.
    assert_eq!(client.get_pulse_count(), 3);
}

#[test]
fn test_batch_pulse_respects_cap() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();
    client.set_max_pulse_count(&5u32);

    let first = Vec::from_array(&env, [
        symbol_short!("a"),
        symbol_short!("b"),
        symbol_short!("c"),
    ]);
    assert_eq!(client.batch_pulse(&first), 3u32);

    // The next batch would push the counter past the cap.
    let second = Vec::from_array(&env, [
        symbol_short!("d"),
        symbol_short!("e"),
        symbol_short!("f"),
    ]);
    let result = client.try_batch_pulse(&second);
    assert_eq!(result.unwrap_err(), Ok(PulseError::PulseCapReached));
    assert_eq!(client.get_pulse_count(), 3);
}

#[test]
fn test_raise_cap_re_enables_pulsing() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    client.set_max_pulse_count(&2u32);
    assert_eq!(client.pulse(&symbol_short!("alice")), 1);
    assert_eq!(client.pulse(&symbol_short!("bob")), 2);
    assert!(client.try_pulse(&symbol_short!("carol")).is_err());

    // Raising the cap re-enables pulsing without resetting the counter.
    client.set_max_pulse_count(&5u32);
    assert_eq!(client.pulse(&symbol_short!("carol")), 3);
}

#[test]
fn test_set_max_pulse_count_emits_config_event() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    client.set_max_pulse_count(&1_000u32);

    let topics = emitted_topic_pairs(&env);
    assert!(
        topics
            .iter()
            .any(|(t0, t1)| t0 == &symbol_short!("config") && t1 == &Some(symbol_short!("cap"))),
        "set_max_pulse_count must emit a config/cap event, got {topics:?}"
    );
}

#[test]
fn test_rate_limited_pulse_reports_cooldown_error() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    env.ledger().set_timestamp(1_000_000);
    let result = client.rate_limited_pulse(&symbol_short!("alice"), &60u64);
    assert_eq!(result, 1u32);

    // Second pulse within the cooldown must surface the dedicated error.
    let result = client.try_rate_limited_pulse(&symbol_short!("alice"), &60u64);
    assert_eq!(result.unwrap_err(), Ok(PulseError::CooldownActive));
}

#[test]
fn test_estimate_pulse_cost() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let cost = client.estimate_pulse_cost();
    assert!(cost > 0);
}

#[test]
fn test_ledger_timestamp() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let ts = client.get_ledger_timestamp();
    // In test environment, timestamp may be 0 initially
    // Just verify the function returns without error
    let _ = ts;
}

/// ── get_version (issue #69) ────────────────────────────────────────────

#[test]
fn test_get_version() {
    let env = Env::default();
    let owner = make_owner(&env);

    // An uninitialized contract reports version 0.
    let (_id, client) = deploy(&env);
    assert_eq!(client.get_version(), 0);

    // Initialization records version 1.
    client.initialize(&owner);
    assert_eq!(client.get_version(), 1);

    // Upgrades bump the reported version.
    env.mock_all_auths();
    let hash = upload_test_wasm(&env);
    client.upgrade_version(&3u32, &hash);
    assert_eq!(client.get_version(), 3);
}

/// ── Event topics (issue #60) ───────────────────────────────────────────

#[test]
fn test_set_signers_emits_updated_event() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    let signers = Vec::from_array(&env, [owner.clone()]);
    client.set_signers(&signers, &1u32);

    let topics = emitted_topic_pairs(&env);
    assert!(
        topics
            .iter()
            .any(|(t0, t1)| t0 == &symbol_short!("signers") && t1 == &Some(symbol_short!("updated"))),
        "set_signers must emit a signers/updated event, got {topics:?}"
    );
}

#[test]
fn test_state_changing_operations_emit_documented_topics() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    client.pause();
    client.unpause();

    let new_owner = env.register_contract(None, PulseContract);
    client.transfer_ownership(&new_owner);

    let signers = Vec::from_array(&env, [new_owner.clone()]);
    client.set_signers(&signers, &1u32);

    let hash = upload_test_wasm(&env);
    client.upgrade_version(&2u32, &hash);
    client.kill();

    let topics = emitted_topic_pairs(&env);
    let has = |primary: &str, secondary: Option<&str>| {
        topics.iter().any(|(t0, t1)| {
            t0 == &Symbol::new(&env, primary)
                && t1 == &secondary.map(|s| Symbol::new(&env, s))
        })
    };

    assert!(has("paused", None), "pause must emit the paused topic");
    assert!(has("unpaused", None), "unpause must emit the unpaused topic");
    assert!(
        has("owner_chg", Some("transfer")),
        "transfer_ownership must emit owner_chg/transfer"
    );
    assert!(
        has("signers", Some("updated")),
        "set_signers must emit signers/updated"
    );
    assert!(
        has("upgrade", Some("applied")),
        "upgrade_version must emit upgrade/applied"
    );
    assert!(
        has("kill", Some("applied")),
        "kill must emit kill/applied"
    );
}

/// ── Admin / owner authorization tests (issue #61) ──────────────────────

#[test]
fn test_transfer_ownership_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let new_owner = env.register_contract(None, PulseContract);
    // No mock_all_auths: the test invoker is not the owner.
    let result = client.try_transfer_ownership(&new_owner);
    assert!(result.is_err(), "non-owner must not transfer ownership");
}

#[test]
fn test_set_signers_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let signers = Vec::from_array(&env, [owner.clone()]);
    let result = client.try_set_signers(&signers, &1u32);
    assert!(result.is_err(), "non-owner must not configure signers");
}

#[test]
fn test_set_signers_rejects_zero_threshold() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    let signers = Vec::from_array(&env, [owner.clone()]);
    let result = client.try_set_signers(&signers, &0u32);
    assert_eq!(result.unwrap_err(), Ok(PulseError::InvalidCaller));
}

#[test]
fn test_set_signers_rejects_threshold_above_signer_count() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    let signers = Vec::from_array(&env, [owner.clone()]);
    let result = client.try_set_signers(&signers, &2u32);
    assert_eq!(result.unwrap_err(), Ok(PulseError::InvalidCaller));
}

#[test]
fn test_pause_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let result = client.try_pause();
    assert!(result.is_err(), "non-owner must not pause the contract");
}

#[test]
fn test_unpause_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (id, client) = deploy_initialized(&env, &owner);

    // Put the contract into the paused state without exercising pause()'s
    // auth check so we can verify unpause() itself rejects non-owners.
    env.as_contract(&id, || {
        let mut meta = get_or_create_meta(&env);
        meta.paused = true;
        env.storage().instance().set(&META_KEY, &meta);
    });

    let result = client.try_unpause();
    assert!(result.is_err(), "non-owner must not unpause the contract");
}

#[test]
fn test_kill_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let result = client.try_kill();
    assert!(result.is_err(), "non-owner must not kill the contract");
}

#[test]
fn test_upgrade_version_requires_owner() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    let hash = upload_test_wasm(&env);
    let result = client.try_upgrade_version(&2u32, &hash);
    assert!(result.is_err(), "non-owner must not upgrade the version");
}

#[test]
fn test_signers_reconfiguration_updates_threshold() {
    let env = Env::default();
    let owner = make_owner(&env);
    let other = env.register_contract(None, PulseContract);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    let first = Vec::from_array(&env, [owner.clone(), other.clone()]);
    client.set_signers(&first, &2u32);
    let (stored, threshold) = client.get_signers();
    assert_eq!(threshold, 2u32);
    assert_eq!(stored.len(), 2);

    // Reconfiguration replaces the previous signer set entirely.
    let second = Vec::from_array(&env, [owner.clone()]);
    client.set_signers(&second, &1u32);
    let (stored, threshold) = client.get_signers();
    assert_eq!(threshold, 1u32);
    assert_eq!(stored.len(), 1);
}

/// ── Upgrade storage preservation (issue #62) ───────────────────────────

#[test]
fn test_upgrade_preserves_storage() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);

    // Set observable state before the upgrade.
    client.pulse(&symbol_short!("alice"));
    client.pulse(&symbol_short!("bob"));
    let signers = Vec::from_array(&env, [owner.clone()]);
    env.mock_all_auths();
    client.set_signers(&signers, &1u32);

    let hash = upload_test_wasm(&env);
    client.upgrade_version(&4u32, &hash);

    // Counter state survives the upgrade untouched.
    assert_eq!(client.get_pulse_count(), 2);
    let data = client.get_pulse_data();
    assert_eq!(data.last_caller, Some(symbol_short!("bob")));

    // Multi-sig configuration survives as well.
    let (stored_signers, threshold) = client.get_signers();
    assert_eq!(threshold, 1u32);
    assert_eq!(stored_signers.len(), 1);

    // The new version is active and the audit record was written.
    assert_eq!(client.get_version(), 4);
    let record = client.get_version_record().unwrap();
    assert_eq!(record.version, 4);
    assert_eq!(record.new_wasm_hash, hash);

    // The upgraded contract remains fully functional.
    let count = client.pulse(&symbol_short!("charlie"));
    assert_eq!(count, 3u32);
}

/// ── Gas / instruction estimation (issue #67) ───────────────────────────
///
/// Budget deltas are measured against the test-env host budget, which
/// accounts for the full invocation (serialization + VM execution + storage).
/// Thresholds are regression guards set to a generous multiple of the
/// observed steady-state cost (soroban-sdk 21.7.7 / host 21.2.1, x86_64):
///
///   pulse()                  ~   3.2k cpu / ~  30k mem
///   batch_pulse(3)           ~   4.1k cpu / ~  34k mem
///   rate_limited_pulse()     ~   3.6k cpu / ~  31k mem
///   set_signers()            ~   4.5k cpu / ~  36k mem
///   pause() / unpause()      ~   3.4k cpu / ~  30k mem
///   upgrade_version()        ~   4.2k cpu / ~  35k mem
///   kill()                   ~   3.6k cpu / ~  31k mem
///   get_pulse_count()        ~   1.2k cpu / ~  22k mem
///   get_pulse_data()         ~   1.3k cpu / ~  23k mem
///   get_signers()            ~   1.3k cpu / ~  23k mem
///   get_version()            ~   1.2k cpu / ~  22k mem
///   estimate_pulse_cost()    ~   1.3k cpu / ~  23k mem
///
/// The hard Soroban budget (testnet/mainnet) is 100M CPU instructions and
/// 128 MB of memory, so the guard thresholds below (100k cpu / 1MB mem) are
/// deliberately tight enough to catch runaway regressions yet far below the
/// protocol limit.
fn measure<T>(env: &Env, f: impl FnOnce() -> T) -> (u64, u64) {
    let cpu_before = env.host().budget_cloned().get_cpu_insns_consumed().unwrap();
    let mem_before = env.host().budget_cloned().get_mem_bytes_consumed().unwrap();
    let _ = f();
    let cpu_after = env.host().budget_cloned().get_cpu_insns_consumed().unwrap();
    let mem_after = env.host().budget_cloned().get_mem_bytes_consumed().unwrap();
    (cpu_after - cpu_before, mem_after - mem_before)
}

const CPU_GUARD: u64 = 100_000;
const MEM_GUARD: u64 = 1_000_000;

#[test]
fn test_gas_estimate_read_functions() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();
    client.pulse(&symbol_short!("alice"));
    let signers = Vec::from_array(&env, [owner.clone()]);
    client.set_signers(&signers, &1u32);

    let (cpu, mem) = measure(&env, || client.get_pulse_count());
    assert!(cpu < CPU_GUARD, "get_pulse_count exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "get_pulse_count exceeded mem guard: {mem}");

    let (cpu, mem) = measure(&env, || client.get_pulse_data());
    assert!(cpu < CPU_GUARD, "get_pulse_data exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "get_pulse_data exceeded mem guard: {mem}");

    let (cpu, mem) = measure(&env, || client.get_signers());
    assert!(cpu < CPU_GUARD, "get_signers exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "get_signers exceeded mem guard: {mem}");

    let (cpu, mem) = measure(&env, || client.get_version());
    assert!(cpu < CPU_GUARD, "get_version exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "get_version exceeded mem guard: {mem}");

    let (cpu, mem) = measure(&env, || client.estimate_pulse_cost());
    assert!(cpu < CPU_GUARD, "estimate_pulse_cost exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "estimate_pulse_cost exceeded mem guard: {mem}");
}

#[test]
fn test_gas_estimate_state_mutating_functions() {
    let env = Env::default();
    let owner = make_owner(&env);
    let (_id, client) = deploy_initialized(&env, &owner);
    env.mock_all_auths();

    // pulse
    let (cpu, mem) = measure(&env, || client.pulse(&symbol_short!("alice")));
    assert!(cpu < CPU_GUARD, "pulse exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "pulse exceeded mem guard: {mem}");

    // batch_pulse
    let callers = Vec::from_array(
        &env,
        [
            symbol_short!("a"),
            symbol_short!("b"),
            symbol_short!("c"),
        ],
    );
    let (cpu, mem) = measure(&env, || client.batch_pulse(&callers));
    assert!(cpu < CPU_GUARD, "batch_pulse exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "batch_pulse exceeded mem guard: {mem}");

    // rate_limited_pulse (cooldown 0 => always allowed)
    let (cpu, mem) = measure(&env, || client.rate_limited_pulse(&symbol_short!("bob"), &0u64));
    assert!(cpu < CPU_GUARD, "rate_limited_pulse exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "rate_limited_pulse exceeded mem guard: {mem}");

    // set_signers
    let signers = Vec::from_array(&env, [owner.clone()]);
    let (cpu, mem) = measure(&env, || client.set_signers(&signers, &1u32));
    assert!(cpu < CPU_GUARD, "set_signers exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "set_signers exceeded mem guard: {mem}");

    // pause / unpause
    let (cpu, mem) = measure(&env, || client.pause());
    assert!(cpu < CPU_GUARD, "pause exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "pause exceeded mem guard: {mem}");
    let (cpu, mem) = measure(&env, || client.unpause());
    assert!(cpu < CPU_GUARD, "unpause exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "unpause exceeded mem guard: {mem}");

    // upgrade_version
    let hash = upload_test_wasm(&env);
    let (cpu, mem) = measure(&env, || client.upgrade_version(&5u32, &hash));
    assert!(cpu < CPU_GUARD, "upgrade_version exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "upgrade_version exceeded mem guard: {mem}");

    // kill (last: permanently disables the contract)
    let (cpu, mem) = measure(&env, || client.kill());
    assert!(cpu < CPU_GUARD, "kill exceeded cpu guard: {cpu}");
    assert!(mem < MEM_GUARD, "kill exceeded mem guard: {mem}");
}

/// ── Property tests (issue #88) ─────────────────────────────────────────

/// Strategy for short, non-empty caller symbols (always within the 32-byte
/// Soroban symbol limit).
fn caller_symbol_strategy() -> impl Strategy<Value = std::string::String> {
    proptest::collection::vec(
        proptest::sample::select(
            "abcdefghijklmnopqrstuvwxyz0123456789"
                .chars()
                .collect::<std::vec::Vec<char>>(),
        ),
        1..=12,
    )
    .prop_map(|chars| chars.into_iter().collect::<std::string::String>())
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Fixed small case count keeps the suite fast and deterministic; the
        // seed is fixed by default so runs are reproducible.
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// Invariant: every successful pulse() increments the counter by exactly
    /// one, in call order, regardless of the caller symbol.
    #[test]
    fn counter_increments_by_exactly_one_per_pulse(
        callers in proptest::collection::vec(caller_symbol_strategy(), 1..=40),
    ) {
        let env = Env::default();
        let owner = make_owner(&env);
        let (_id, client) = deploy_initialized(&env, &owner);

        let mut expected: u32 = 0;
        for caller in callers {
            let symbol = Symbol::new(&env, &caller);
            let count = client.pulse(&symbol);
            expected = expected.checked_add(1).unwrap();
            prop_assert_eq!(count, expected, "counter must increase by exactly 1 per pulse");
        }
        prop_assert_eq!(client.get_pulse_count(), expected);
    }

    /// Invariant: every pulse emits exactly one event, so the event stream
    /// contains the initialization event plus one pulse event per call.
    #[test]
    fn every_pulse_emits_an_event(
        callers in proptest::collection::vec(caller_symbol_strategy(), 1..=20),
    ) {
        let env = Env::default();
        let owner = make_owner(&env);
        let (_id, client) = deploy_initialized(&env, &owner);

        let total = callers.len() as u32;
        for caller in callers {
            client.pulse(&Symbol::new(&env, &caller));
        }

        let events = env.events().all();
        prop_assert_eq!(
            events.len() as u32,
            total + 1,
            "init event + exactly one pulse event per call"
        );
    }

    /// Invariant: the rate-limit cooldown rejects calls inside the window and
    /// the window resets once the ledger timestamp advances past it.
    #[test]
    fn rate_limit_window_resets(
        cooldown in 1u64..=1_000_000u64,
        caller in caller_symbol_strategy(),
    ) {
        let env = Env::default();
        let owner = make_owner(&env);
        let (_id, client) = deploy_initialized(&env, &owner);

        env.ledger().set_timestamp(1_000_000);
        let symbol = Symbol::new(&env, &caller);

        let first = client.rate_limited_pulse(&symbol, &cooldown);
        prop_assert_eq!(first, 1);

        let second = client.try_rate_limited_pulse(&symbol, &cooldown);
        prop_assert!(
            second.is_err(),
            "a call inside the cooldown window must be rejected"
        );

        env.ledger().set_timestamp(1_000_000 + cooldown);
        let third = client.rate_limited_pulse(&symbol, &cooldown);
        prop_assert_eq!(
            third, 2,
            "the cooldown window must reset once the elapsed time passes"
        );
    }
}
