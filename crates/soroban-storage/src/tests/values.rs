//! Keys whose values are `#[contracttype]` structs and enums, `Vec` and
//! `Map`: set / get / update round-trips and the `WrongType` error path.

extern crate std;

use crate::*;
use soroban_sdk::{
    contract, contractimpl, contracttype, map, symbol_short, testutils::Address as _, vec, Address,
    Env, Error, IntoVal, Map, Symbol, Val, Vec,
};

const TTL: TtlPolicy = TtlPolicy::new(1_000, 5_000);

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    pub owner: Address,
    pub amount: i128,
    pub locked: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Status {
    Pending,
    Active(u64),
    Closed(Address, i128),
}

storage_keys! {
    persistent(TTL) PositionOf(id: u32) => Position;
    temporary(TTL) StatusOf(id: u32) => Status;
    instance(TTL) Members => Vec<Address>;
    persistent(TTL) Limits(owner: Address) => Map<Symbol, i128>;
}

#[contract]
pub struct ValuesContract;

#[contractimpl]
impl ValuesContract {
    pub fn position(env: Env, id: u32) -> Position {
        PositionOf(id).require(&env)
    }

    pub fn status(env: Env, id: u32) -> Status {
        StatusOf(id).require(&env)
    }

    pub fn members(env: Env) -> Vec<Address> {
        Members.require(&env)
    }

    pub fn limits(env: Env, owner: Address) -> Map<Symbol, i128> {
        Limits(owner).require(&env)
    }

    pub fn bump_position(env: Env, id: u32, by: i128) -> Position {
        PositionOf(id).update(&env, |p| {
            let mut p = p.unwrap();
            p.amount += by;
            p
        })
    }
}

fn setup() -> (Env, Address, ValuesContractClient<'static>) {
    let env = Env::default();
    let id = env.register(ValuesContract, ());
    let client = ValuesContractClient::new(&env, &id);
    (env, id, client)
}

fn wrong_type() -> Error {
    let err: Error = StorageError::WrongType.into();
    assert_eq!(err, Error::from_contract_error(1101));
    err
}

// ---------------------------------------------------------------------------
// #[contracttype] struct
// ---------------------------------------------------------------------------

#[test]
fn struct_set_get_update() {
    let (env, id, client) = setup();
    let owner = Address::generate(&env);
    let key = PositionOf(1);
    env.as_contract(&id, || {
        assert_eq!(key.get(&env), None);

        let pos = Position {
            owner: owner.clone(),
            amount: 100,
            locked: false,
        };
        key.set(&env, &pos);
        assert_eq!(key.get(&env), Some(pos.clone()));
        assert_eq!(key.ttl(&env), TTL.extend_to);

        let updated = key.update(&env, |old| {
            let mut p = old.unwrap();
            p.amount += 50;
            p.locked = true;
            p
        });
        assert_eq!(updated.amount, 150);
        assert!(updated.locked);
        assert_eq!(key.get(&env), Some(updated));

        // Other ids are independent.
        assert_eq!(PositionOf(2).get(&env), None);
    });

    assert_eq!(client.bump_position(&1, &-25).amount, 125);
    assert_eq!(client.position(&1).amount, 125);
    assert_eq!(client.position(&1).owner, owner);
}

#[test]
fn struct_wrong_type() {
    let (env, id, client) = setup();
    env.as_contract(&id, || {
        // A vector where a struct (stored as a map) is expected.
        let raw = PositionOf(1).to_key_val(&env);
        env.storage()
            .persistent()
            .set(&raw, &vec![&env, 1_u32, 2_u32]);
    });
    let err = client.try_position(&1).unwrap_err().unwrap();
    assert_eq!(err, wrong_type());
    let err = client.try_bump_position(&1, &1).unwrap_err().unwrap();
    assert_eq!(err, wrong_type());
}

#[test]
fn struct_with_missing_field_is_wrong_type() {
    let (env, id, client) = setup();
    env.as_contract(&id, || {
        // Right shape (a map), but a field is missing.
        let raw = PositionOf(1).to_key_val(&env);
        let partial: Map<Symbol, Val> = map![
            &env,
            (symbol_short!("amount"), 5_i128.into_val(&env)),
            (symbol_short!("locked"), true.into_val(&env)),
        ];
        env.storage().persistent().set(&raw, &partial);
    });
    let err = client.try_position(&1).unwrap_err().unwrap();
    assert_eq!(err, wrong_type());
}

// ---------------------------------------------------------------------------
// #[contracttype] enum
// ---------------------------------------------------------------------------

#[test]
fn enum_set_get_update() {
    let (env, id, client) = setup();
    let closer = Address::generate(&env);
    let key = StatusOf(7);
    env.as_contract(&id, || {
        assert_eq!(key.get(&env), None);

        key.set(&env, &Status::Pending);
        assert_eq!(key.get(&env), Some(Status::Pending));
        assert_eq!(key.ttl(&env), TTL.extend_to);

        // Change the variant.
        assert_eq!(
            key.update(&env, |old| match old {
                Some(Status::Pending) => Status::Active(1),
                other => panic!("unexpected {other:?}"),
            }),
            Status::Active(1)
        );
        // Update the payload of the same variant.
        assert_eq!(
            key.update(&env, |old| match old {
                Some(Status::Active(n)) => Status::Active(n + 1),
                other => panic!("unexpected {other:?}"),
            }),
            Status::Active(2)
        );

        key.set(&env, &Status::Closed(closer.clone(), -3));
    });
    assert_eq!(client.status(&7), Status::Closed(closer, -3));
}

#[test]
fn enum_wrong_type() {
    let (env, id, client) = setup();
    env.as_contract(&id, || {
        let raw = StatusOf(7).to_key_val(&env);
        env.storage().temporary().set(&raw, &42_i128);
    });
    let err = client.try_status(&7).unwrap_err().unwrap();
    assert_eq!(err, wrong_type());
}

// ---------------------------------------------------------------------------
// Vec<_>
// ---------------------------------------------------------------------------

#[test]
fn vec_set_get_update() {
    let (env, id, client) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let c = Address::generate(&env);
    env.as_contract(&id, || {
        assert_eq!(Members.get(&env), None);

        Members.set(&env, &vec![&env, a.clone()]);
        assert_eq!(Members.get(&env), Some(vec![&env, a.clone()]));
        assert_eq!(Members.ttl(&env), TTL.extend_to);

        let after = Members.update(&env, |old| {
            let mut v = old.unwrap_or_else(|| Vec::new(&env));
            v.push_back(b.clone());
            v.push_back(c.clone());
            v
        });
        assert_eq!(after, vec![&env, a.clone(), b.clone(), c.clone()]);

        let after = Members.update(&env, |old| {
            let mut v = old.unwrap();
            v.remove(0);
            v
        });
        assert_eq!(after.len(), 2);

        // An empty vector is a stored value, not a missing one.
        Members.set(&env, &Vec::new(&env));
        assert!(Members.has(&env));
    });
    assert_eq!(client.members(), Vec::<Address>::new(&env));
}

#[test]
fn vec_wrong_type() {
    let (env, id, client) = setup();
    env.as_contract(&id, || {
        let raw = Members.to_key_val(&env);
        let not_a_vec: Map<u32, u32> = map![&env, (1, 2)];
        env.storage().instance().set(&raw, &not_a_vec);
    });
    let err = client.try_members().unwrap_err().unwrap();
    assert_eq!(err, wrong_type());
}

// ---------------------------------------------------------------------------
// Map<_, _>
// ---------------------------------------------------------------------------

#[test]
fn map_set_get_update() {
    let (env, id, client) = setup();
    let owner = Address::generate(&env);
    let key = Limits(owner.clone());
    env.as_contract(&id, || {
        assert_eq!(key.get(&env), None);

        let limits = map![&env, (symbol_short!("daily"), 1_000_i128)];
        key.set(&env, &limits);
        assert_eq!(key.get(&env), Some(limits));
        assert_eq!(key.ttl(&env), TTL.extend_to);

        let after = key.update(&env, |old| {
            let mut m = old.unwrap();
            m.set(symbol_short!("daily"), 2_000);
            m.set(symbol_short!("weekly"), 9_000);
            m
        });
        assert_eq!(after.len(), 2);
        assert_eq!(after.get(symbol_short!("daily")), Some(2_000));
        assert_eq!(after.get(symbol_short!("weekly")), Some(9_000));

        // Another owner's map is untouched.
        assert_eq!(Limits(Address::generate(&env)).get(&env), None);
    });
    let stored = client.limits(&owner);
    assert_eq!(stored.get(symbol_short!("daily")), Some(2_000));
    assert_eq!(stored.get(symbol_short!("weekly")), Some(9_000));
}

#[test]
fn map_wrong_type() {
    let (env, id, client) = setup();
    let owner = Address::generate(&env);
    env.as_contract(&id, || {
        let raw = Limits(owner.clone()).to_key_val(&env);
        env.storage()
            .persistent()
            .set(&raw, &vec![&env, symbol_short!("daily")]);
    });
    let err = client.try_limits(&owner).unwrap_err().unwrap();
    assert_eq!(err, wrong_type());
}
