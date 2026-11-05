extern crate std;

mod costs;
mod model;
mod values;

use crate::*;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype,
    testutils::{Address as _, Ledger},
    xdr::ScVal,
    Address, Env, Error, IntoVal, TryFromVal, Val,
};

const INSTANCE_TTL: TtlPolicy = TtlPolicy::new(days(7), days(30));
const BALANCE_TTL: TtlPolicy = TtlPolicy::new(1_000, 5_000);
const LAZY_TTL: TtlPolicy = TtlPolicy::new(1_000, 5_000).write_only();
const NONCE_TTL: TtlPolicy = TtlPolicy::new(50, 100);
// Far beyond any network's maximum entry TTL; must be clamped, not fail.
const HUGE_TTL: TtlPolicy = TtlPolicy::new(days(10_000), days(10_000));

storage_keys! {
    /// Contract administrator.
    pub instance(INSTANCE_TTL) Admin => Address;
    pub persistent(BALANCE_TTL) Balance(owner: Address) => i128;
    persistent(LAZY_TTL) Lazy(id: u32) => u32;
    temporary(NONCE_TTL) Nonce(owner: Address, nonce: u64) => ();
    persistent(HUGE_TTL) Forever => bool;
}

/// A hand-written key enum, as contracts write it without this crate. Used
/// to check that both produce the same ledger keys.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Balance(Address),
    Nonce(Address, u64),
    Forever,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum TestError {
    NotInitialized = 7,
}

#[contract]
pub struct TestContract;

#[contractimpl]
impl TestContract {
    pub fn required_balance(env: Env, owner: Address) -> i128 {
        Balance(owner).require(&env)
    }

    pub fn required_admin(env: Env) -> Address {
        Admin.require_or(&env, TestError::NotInitialized)
    }
}

fn setup() -> (Env, Address) {
    let env = Env::default();
    let id = env.register(TestContract, ());
    (env, id)
}

fn advance(env: &Env, ledgers: u32) {
    env.ledger().with_mut(|l| l.sequence_number += ledgers);
}

#[test]
fn ledger_time_helpers() {
    assert_eq!(LEDGERS_PER_MINUTE, 12);
    assert_eq!(minutes(5), 60);
    assert_eq!(hours(1), 720);
    assert_eq!(days(1), 17_280);
    assert_eq!(days(30), 518_400);
}

#[test]
#[should_panic(expected = "threshold must not exceed extend_to")]
fn policy_rejects_threshold_above_extend_to() {
    let threshold = std::hint::black_box(10);
    TtlPolicy::new(threshold, 5);
}

#[test]
#[should_panic(expected = "extend_to must be greater than zero")]
fn policy_rejects_zero_extend_to() {
    TtlPolicy::new(0, std::hint::black_box(0));
}

#[test]
fn set_extends_persistent_ttl_to_policy() {
    let (env, id) = setup();
    let owner = Address::generate(&env);
    env.as_contract(&id, || {
        Balance(owner.clone()).set(&env, &100);
        assert_eq!(Balance(owner.clone()).ttl(&env), BALANCE_TTL.extend_to);
        assert_eq!(Balance(owner).get(&env), Some(100));
    });
}

#[test]
fn read_bumps_only_below_threshold() {
    let (env, id) = setup();
    let owner = Address::generate(&env);
    let key = Balance(owner);
    env.as_contract(&id, || key.set(&env, &1));

    // Still above the threshold: reading must not extend.
    advance(&env, 3_000);
    env.as_contract(&id, || {
        assert_eq!(key.ttl(&env), 2_000);
        assert_eq!(key.get(&env), Some(1));
        assert_eq!(key.ttl(&env), 2_000);
    });

    // Below the threshold: reading extends back to `extend_to`.
    advance(&env, 1_500);
    env.as_contract(&id, || {
        assert_eq!(key.ttl(&env), 500);
        assert!(key.has(&env));
        assert_eq!(key.ttl(&env), BALANCE_TTL.extend_to);
    });
}

#[test]
fn write_only_policy_does_not_bump_on_read() {
    let (env, id) = setup();
    env.as_contract(&id, || Lazy(1).set(&env, &7));
    advance(&env, 4_500);
    env.as_contract(&id, || {
        assert_eq!(Lazy(1).get(&env), Some(7));
        assert_eq!(Lazy(1).ttl(&env), 500);
        // A write still extends.
        Lazy(1).set(&env, &8);
        assert_eq!(Lazy(1).ttl(&env), LAZY_TTL.extend_to);
    });
}

#[test]
fn temporary_entries_disappear_after_ttl() {
    let (env, id) = setup();
    let owner = Address::generate(&env);
    let key = Nonce(owner, 1);
    env.as_contract(&id, || {
        key.set(&env, &());
        assert_eq!(key.ttl(&env), NONCE_TTL.extend_to);
        assert!(key.has(&env));
    });

    advance(&env, NONCE_TTL.extend_to + 1);
    env.as_contract(&id, || {
        assert!(!key.has(&env));
        assert_eq!(key.get(&env), None);
        assert_eq!(key.try_get(&env), Err(StorageError::NotFound));
    });
}

#[test]
fn instance_keys_extend_the_instance() {
    let (env, id) = setup();
    let admin = Address::generate(&env);
    env.as_contract(&id, || {
        Admin.set(&env, &admin);
        assert_eq!(Admin.ttl(&env), INSTANCE_TTL.extend_to);
        assert_eq!(Admin.require(&env), admin);
    });
}

#[test]
fn extend_instance_uses_policy() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        extend_instance(&env, &INSTANCE_TTL);
        assert_eq!(Admin.ttl(&env), INSTANCE_TTL.extend_to);
    });
}

#[test]
fn policy_is_clamped_to_network_max_ttl() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let max = env.storage().max_ttl();
        assert!(HUGE_TTL.extend_to > max);
        Forever.set(&env, &true);
        assert_eq!(Forever.ttl(&env), max);
    });
}

#[test]
fn require_fails_with_not_found_error() {
    let (env, id) = setup();
    let client = TestContractClient::new(&env, &id);
    let owner = Address::generate(&env);

    let err = client.try_required_balance(&owner).unwrap_err().unwrap();
    assert_eq!(err, Error::from_contract_error(1100));
    assert_eq!(err, StorageError::NotFound.into());

    let err = client.try_required_admin().unwrap_err().unwrap();
    assert_eq!(err, Error::from_contract_error(7));

    env.as_contract(&id, || Balance(owner.clone()).set(&env, &42));
    assert_eq!(client.required_balance(&owner), 42);
}

#[test]
fn wrong_stored_type_fails_with_wrong_type_error() {
    let (env, id) = setup();
    let client = TestContractClient::new(&env, &id);
    let owner = Address::generate(&env);
    env.as_contract(&id, || {
        // Write a Symbol where Balance expects an i128.
        env.storage().persistent().set(
            &DataKey::Balance(owner.clone()),
            &soroban_sdk::symbol_short!("oops"),
        );
    });
    let err = client.try_required_balance(&owner).unwrap_err().unwrap();
    assert_eq!(err, StorageError::WrongType.into());
}

#[test]
fn update_take_remove_and_defaults() {
    let (env, id) = setup();
    let owner = Address::generate(&env);
    let key = Balance(owner);
    env.as_contract(&id, || {
        assert_eq!(key.get_or(&env, 0), 0);
        assert_eq!(key.get_or_else(&env, || 9), 9);
        assert!(!key.has(&env), "defaults must not be written");
        assert!(!key.extend(&env), "extending a missing key is a no-op");

        assert_eq!(key.update(&env, |v| v.unwrap_or(0) + 10), 10);
        assert_eq!(key.update(&env, |v| v.unwrap_or(0) + 5), 15);
        assert_eq!(key.ttl(&env), BALANCE_TTL.extend_to);

        let res: Result<i128, &str> = key.try_update(&env, |_| Err("nope"));
        assert_eq!(res, Err("nope"));
        assert_eq!(key.get(&env), Some(15), "failed update must not write");

        assert!(key.extend(&env));
        assert_eq!(key.take(&env), Some(15));
        assert_eq!(key.take(&env), None);

        key.set(&env, &1);
        key.remove(&env);
        assert!(!key.has(&env));
    });
}

#[test]
fn keys_match_contracttype_enum_encoding() {
    let env = Env::default();
    let owner = Address::generate(&env);
    let cases: [(Val, Val); 4] = [
        (Admin.to_key_val(&env), DataKey::Admin.into_val(&env)),
        (
            Balance(owner.clone()).to_key_val(&env),
            DataKey::Balance(owner.clone()).into_val(&env),
        ),
        (
            Nonce(owner.clone(), 9).to_key_val(&env),
            DataKey::Nonce(owner.clone(), 9).into_val(&env),
        ),
        (Forever.to_key_val(&env), DataKey::Forever.into_val(&env)),
    ];
    for (ours, sdk) in cases {
        let ours: ScVal = ScVal::try_from_val(&env, &ours).unwrap();
        let sdk: ScVal = ScVal::try_from_val(&env, &sdk).unwrap();
        assert_eq!(ours, sdk);
    }
}

#[test]
fn data_written_via_raw_enum_is_readable() {
    let (env, id) = setup();
    let owner = Address::generate(&env);
    env.as_contract(&id, || {
        // Simulates a contract migrating from a hand-written `DataKey`.
        env.storage()
            .persistent()
            .set(&DataKey::Balance(owner.clone()), &77_i128);
        assert_eq!(Balance(owner).get(&env), Some(77));
    });
}

#[test]
fn lifecycle_is_exposed() {
    let env = Env::default();
    let owner = Address::generate(&env);
    assert_eq!(Admin.lifecycle(), Lifecycle::Instance);
    assert_eq!(Balance(owner.clone()).lifecycle(), Lifecycle::Persistent);
    assert_eq!(Nonce(owner, 0).lifecycle(), Lifecycle::Temporary);
}
