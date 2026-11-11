extern crate std;

use super::*;
use soroban_sdk::testutils::{Address as _, Ledger};
use soroban_storage::{assert_ttl, KeyExt};

fn setup() -> (Env, Address, VaultClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let id = env.register(Vault, (&admin,));
    let client = VaultClient::new(&env, &id);
    (env, id, client)
}

fn advance(env: &Env, ledgers: u32) {
    env.ledger().with_mut(|l| l.sequence_number += ledgers);
}

#[test]
fn deposit_and_withdraw() {
    let (env, _, client) = setup();
    let user = Address::generate(&env);
    assert_eq!(client.balance(&user), 0);
    assert_eq!(client.deposit(&user, &100), 100);
    assert_eq!(client.withdraw(&user, &30), 70);
    assert_eq!(
        client.try_withdraw(&user, &10),
        Err(Ok(VaultError::CoolingDown))
    );
}

#[test]
fn balance_ttl_follows_policy() {
    let (env, id, client) = setup();
    let user = Address::generate(&env);
    client.deposit(&user, &100);
    assert_ttl!(env, id, Balance(user.clone()), BALANCE_TTL.extend_to);
    assert_ttl!(env, id, Admin, INSTANCE_TTL.extend_to);

    // Once the balance drops below its threshold, a read extends it again.
    advance(&env, BALANCE_TTL.extend_to - BALANCE_TTL.threshold + 1);
    env.as_contract(&id, || {
        assert!(Balance(user.clone()).ttl(&env) < BALANCE_TTL.threshold);
    });
    assert_eq!(client.balance(&user), 100);
    assert_ttl!(env, id, Balance(user), BALANCE_TTL.extend_to);
}

#[test]
fn cooldown_expires_with_its_temporary_entry() {
    let (env, _, client) = setup();
    let user = Address::generate(&env);
    client.deposit(&user, &100);
    client.withdraw(&user, &10);
    assert_eq!(
        client.try_withdraw(&user, &10),
        Err(Ok(VaultError::CoolingDown))
    );

    advance(&env, COOLDOWN_LEDGERS + 1);
    assert_eq!(client.withdraw(&user, &10), 80);
}

#[test]
fn rejects_overdraft_without_writing() {
    let (env, _, client) = setup();
    let user = Address::generate(&env);
    client.deposit(&user, &5);
    assert_eq!(
        client.try_withdraw(&user, &6),
        Err(Ok(VaultError::InsufficientBalance))
    );
    assert_eq!(client.balance(&user), 5);
    assert_eq!(client.withdraw(&user, &5), 0);
}
