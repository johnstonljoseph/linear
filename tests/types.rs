use linear::{
    Capabilities, Component, DeclaredCapabilities, TypeError, TypeId,
    TypeKind, TypeStore,
};

#[test]
fn store_starts_with_never_and_unit() {
    let store = TypeStore::new();

    assert_eq!(store.type_id("Never"), Some(store.never()));
    assert_eq!(store.type_id("Unit"), Some(store.unit()));
    assert_eq!(store.get(store.never()).unwrap().kind, TypeKind::Never);
    assert_eq!(store.get(store.unit()).unwrap().kind, TypeKind::Unit);
}

#[test]
fn finite_integer_type_represents_large_finite_sum_compactly() {
    let mut store = TypeStore::new();
    let u32_ty = store.add_uint("U32", 32).unwrap();

    assert_eq!(store.type_id("U32"), Some(u32_ty));
    assert_eq!(
        store.get(u32_ty).unwrap().kind,
        TypeKind::Finite {
            values: 1u128 << 32
        }
    );
    assert_eq!(store.capabilities(u32_ty).unwrap(), Capabilities::dup_zap());
}

#[test]
fn type_aliases_resolve_to_existing_type_ids() {
    let mut store = TypeStore::new();
    let u32_ty = store.add_uint("U32", 32).unwrap();

    store.add_alias("UserId", u32_ty).unwrap();

    assert_eq!(store.type_id("UserId"), Some(u32_ty));
    assert_eq!(
        store.add_alias("UserId", u32_ty).unwrap_err(),
        TypeError::DuplicateName("UserId".into())
    );
    assert_eq!(
        store.add_alias("Missing", TypeId(999)).unwrap_err(),
        TypeError::UnknownType(TypeId(999))
    );
}

#[test]
fn bool_can_be_built_as_named_sum_of_units() {
    let mut store = TypeStore::new();
    let bool_ty = store
        .add_sum(
            Some("Bool".into()),
            vec![
                Component::named("false", store.unit()),
                Component::named("true", store.unit()),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();

    assert!(store.can_dup(bool_ty).unwrap());
    assert!(store.can_zap(bool_ty).unwrap());
}

#[test]
fn product_capabilities_are_structural() {
    let mut store = TypeStore::new();
    let token = store
        .add_primitive("Token", DeclaredCapabilities::linear())
        .unwrap();
    let droppable = store
        .add_primitive("Droppable", DeclaredCapabilities::zap())
        .unwrap();
    let product = store
        .add_product(
            Some("Pair".into()),
            vec![
                Component::named("token", token),
                Component::named("droppable", droppable),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();

    assert_eq!(store.capabilities(token).unwrap(), Capabilities::linear());
    assert!(!store.can_dup(product).unwrap());
    assert!(!store.can_zap(product).unwrap());

    let droppable_pair = store
        .add_product(
            Some("DroppablePair".into()),
            vec![
                Component::positional(0, droppable),
                Component::positional(1, store.unit()),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    assert!(!store.can_dup(droppable_pair).unwrap());
    assert!(store.can_zap(droppable_pair).unwrap());
}

#[test]
fn composites_cannot_grant_capabilities_beyond_their_structure() {
    let mut store = TypeStore::new();
    let token = store
        .add_primitive("Token", DeclaredCapabilities::linear())
        .unwrap();

    assert_eq!(
        store
            .add_product(
                Some("CopyToken".into()),
                vec![Component::named("token", token)],
                DeclaredCapabilities::dup(),
            )
            .unwrap_err(),
        TypeError::DeclaredCapabilityExceedsStructural {
            declared: Capabilities {
                dup: true,
                zap: false,
            },
            structural: Capabilities::linear(),
        }
    );

    assert_eq!(
        store
            .add_sum(
                Some("DropToken".into()),
                vec![Component::named("token", token)],
                DeclaredCapabilities::zap(),
            )
            .unwrap_err(),
        TypeError::DeclaredCapabilityExceedsStructural {
            declared: Capabilities {
                dup: false,
                zap: true,
            },
            structural: Capabilities::linear(),
        }
    );
}

#[test]
fn composite_declared_capabilities_may_confirm_structural_capabilities() {
    let mut store = TypeStore::new();
    let u32_ty = store.add_uint("U32", 32).unwrap();
    let pair = store
        .add_product(
            Some("Pair".into()),
            vec![
                Component::positional(0, u32_ty),
                Component::positional(1, u32_ty),
            ],
            DeclaredCapabilities::dup(),
        )
        .unwrap();

    assert_eq!(store.capabilities(pair).unwrap(), Capabilities::dup_zap());
}

#[test]
fn primitive_capabilities_are_axiomatic() {
    let mut store = TypeStore::new();
    let copyable = store
        .add_primitive("Copyable", DeclaredCapabilities::dup_zap())
        .unwrap();

    assert_eq!(
        store.capabilities(copyable).unwrap(),
        Capabilities::dup_zap()
    );
}

#[test]
fn function_types_have_dup_and_zap() {
    let mut store = TypeStore::new();
    let f = store
        .add_function(Some("UnitFn".into()), store.unit(), store.unit())
        .unwrap();

    assert_eq!(store.capabilities(f).unwrap(), Capabilities::dup_zap());
}

#[test]
fn symbol_and_text_types_have_dup_and_zap() {
    let mut store = TypeStore::new();
    let symbol = store.add_symbol("Symbol").unwrap();
    let text = store.add_text("Text").unwrap();

    assert_eq!(store.capabilities(symbol).unwrap(), Capabilities::dup_zap());
    assert_eq!(store.capabilities(text).unwrap(), Capabilities::dup_zap());
}

#[test]
fn duplicate_type_names_are_rejected() {
    let mut store = TypeStore::new();
    store
        .add_finite(Some("U8".into()), 256, DeclaredCapabilities::dup_zap())
        .unwrap();

    assert_eq!(
        store.add_uint("U8", 8).unwrap_err(),
        TypeError::DuplicateName("U8".into())
    );
}

#[test]
fn unknown_type_references_are_rejected() {
    let mut store = TypeStore::new();
    let missing = TypeId(999);

    assert_eq!(
        store
            .add_product(
                Some("Bad".into()),
                vec![Component::named("missing", missing)],
                DeclaredCapabilities::linear(),
            )
            .unwrap_err(),
        TypeError::UnknownType(missing)
    );
}

#[test]
fn invalid_finite_cardinalities_are_rejected() {
    let mut store = TypeStore::new();

    assert_eq!(
        store
            .add_finite(None, 0, DeclaredCapabilities::dup_zap())
            .unwrap_err(),
        TypeError::ZeroFiniteCardinality
    );
    assert_eq!(
        store.add_uint("U128", 128).unwrap_err(),
        TypeError::UIntTooWide(128)
    );
}
