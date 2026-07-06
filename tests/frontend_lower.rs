use linear::frontend::ValueFlow;
use linear::{
    CollectionMutability, Component, ComponentName, CoreError, Evaluator, TypeError, TypeKind,
    Value, frontend,
};

fn lower(src: &str) -> linear::TypeStore {
    let module = frontend::parse_module(src).unwrap();
    frontend::lower_type_items(&module).unwrap().types
}

#[test]
fn lowers_type_aliases_structs_and_enums() {
    let types = lower(
        r#"
        type UserId = u32
        type Balance = u32

        struct User { id: UserId, balance: Balance }

        enum Decision {
          allow { reason: u32 },
          deny,
          review { queue: u32, priority: u32 },
        }
        "#,
    );

    let u32_ty = types.type_id("u32").unwrap();
    assert_eq!(types.type_id("UserId"), Some(u32_ty));
    assert_eq!(types.type_id("Balance"), Some(u32_ty));

    let user_ty = types.type_id("User").unwrap();
    assert_eq!(
        types.get(user_ty).unwrap().kind,
        TypeKind::Product(vec![
            Component::named("id", u32_ty),
            Component::named("balance", u32_ty),
        ])
    );

    let decision_ty = types.type_id("Decision").unwrap();
    let TypeKind::Sum(variants) = &types.get(decision_ty).unwrap().kind else {
        panic!("expected sum");
    };
    assert_eq!(variants.len(), 3);
    assert_eq!(variants[0].name, ComponentName::Named("allow".into()));
    assert_eq!(variants[1], Component::named("deny", types.unit()));
    assert_eq!(variants[2].name, ComponentName::Named("review".into()));
}

#[test]
fn lowers_tuple_structs_and_anonymous_products() {
    let types = lower(
        r#"
        struct MyInt(u32)
        type Pair = (u32, u32)
        struct UsesPair { left: Pair, right: Pair }
        "#,
    );

    let u32_ty = types.type_id("u32").unwrap();
    let my_int = types.type_id("MyInt").unwrap();
    assert_eq!(
        types.get(my_int).unwrap().kind,
        TypeKind::Product(vec![Component::positional(0, u32_ty)])
    );

    let pair = types.type_id("Pair").unwrap();
    let uses_pair = types.type_id("UsesPair").unwrap();
    assert_eq!(
        types.get(uses_pair).unwrap().kind,
        TypeKind::Product(vec![
            Component::named("left", pair),
            Component::named("right", pair),
        ])
    );
}

#[test]
fn lowers_builtin_collection_types_and_interns_repeated_shapes() {
    let types = lower(
        r#"
        type UserId = u32
        struct User { id: UserId, balance: u32 }
        struct Store {
          active: HashMap<UserId, User>,
          pending: HashMap<UserId, User>,
          log: List<User>,
          work: MutList<User>,
          edits: MutHashMap<UserId, User>,
        }
        "#,
    );

    let user = types.type_id("User").unwrap();
    let user_id = types.type_id("UserId").unwrap();
    let store = types.type_id("Store").unwrap();
    let TypeKind::Product(fields) = &types.get(store).unwrap().kind else {
        panic!("expected product");
    };

    let active_ty = fields[0].ty;
    let pending_ty = fields[1].ty;
    assert_eq!(active_ty, pending_ty);
    assert_eq!(
        types.get(active_ty).unwrap().kind,
        TypeKind::HashMap {
            key: user_id,
            value: user,
            mutability: CollectionMutability::Immutable,
        }
    );
    assert_eq!(
        types.get(fields[2].ty).unwrap().kind,
        TypeKind::List {
            element: user,
            mutability: CollectionMutability::Immutable,
        }
    );
    assert_eq!(
        types.get(fields[3].ty).unwrap().kind,
        TypeKind::List {
            element: user,
            mutability: CollectionMutability::Mutable,
        }
    );
    assert_eq!(
        types.get(fields[4].ty).unwrap().kind,
        TypeKind::HashMap {
            key: user_id,
            value: user,
            mutability: CollectionMutability::Mutable,
        }
    );
}

#[test]
fn rejects_unknown_types_bad_collection_arity_and_generic_type_decls() {
    let module = frontend::parse_module("struct Bad { missing: Missing }").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::UnknownType("Missing".into())
    );

    let module = frontend::parse_module("struct Bad { xs: List<u32, u32> }").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::BadGenericArity {
            name: "List".into(),
            expected: 1,
            actual: 2,
        }
    );

    let module = frontend::parse_module("struct Box<T> { value: T }").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::UnsupportedGenericDecl { name: "Box".into() }
    );

    let module = frontend::parse_module("type u32 = u16").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::Type(TypeError::DuplicateName("u32".into()))
    );
}

#[test]
fn lowers_global_and_function_signatures() {
    let module = frontend::parse_module(
        r#"
        type UserId = u32
        struct User { id: UserId, balance: u32 }
        global root: User

        fn deposit(!user: User, amount: u32) -> User {
          user
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_signatures(&module).unwrap();
    let user = lowered.types.type_id("User").unwrap();
    let u32_ty = lowered.types.type_id("u32").unwrap();

    assert_eq!(lowered.globals.len(), 1);
    assert_eq!(lowered.globals[0].name, "root");
    assert_eq!(lowered.globals[0].ty, user);
    assert_eq!(
        lowered.program.global_decl_id("root"),
        Some(lowered.globals[0].id)
    );

    assert_eq!(lowered.functions.len(), 1);
    let deposit = &lowered.functions[0];
    assert_eq!(deposit.name, "deposit");
    assert_eq!(deposit.output, user);
    assert_eq!(lowered.program.function_id("deposit"), Some(deposit.id));
    assert_eq!(deposit.params.len(), 2);
    assert_eq!(deposit.params[0].flow, ValueFlow::ReturnedChanged);
    assert_eq!(deposit.params[0].name, "user");
    assert_eq!(deposit.params[0].ty, user);
    assert_eq!(deposit.params[1].flow, ValueFlow::ReturnedUnchanged);
    assert_eq!(deposit.params[1].name, "amount");
    assert_eq!(deposit.params[1].ty, u32_ty);

    let shell = lowered.program.get(deposit.id).unwrap();
    assert_eq!(shell.inputs.len(), 2);
    assert_eq!(shell.outputs, vec![user]);
    assert!(shell.body.is_empty());
    assert!(shell.returns.is_empty());
}

#[test]
fn lowers_impl_method_signatures_with_expanded_self() {
    let module = frontend::parse_module(
        r#"
        struct User { id: u32, balance: u32 }

        impl User {
          fn balance(self) -> u32 {
            self.balance
          }

          fn with_balance(!self, balance: u32) -> User {
            self
          }
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_signatures(&module).unwrap();
    let user = lowered.types.type_id("User").unwrap();
    let u32_ty = lowered.types.type_id("u32").unwrap();

    assert!(lowered.functions.is_empty());
    assert_eq!(lowered.methods.len(), 2);

    let balance = &lowered.methods[0];
    assert_eq!(balance.owner, user);
    assert_eq!(balance.method, "balance");
    assert_eq!(balance.function.name, "User.balance");
    assert_eq!(
        lowered.program.function_id("User.balance"),
        Some(balance.function.id)
    );
    assert_eq!(balance.function.params[0].name, "self");
    assert_eq!(balance.function.params[0].ty, user);
    assert_eq!(balance.function.output, u32_ty);

    let with_balance = &lowered.methods[1];
    assert_eq!(with_balance.function.name, "User.with_balance");
    assert_eq!(
        with_balance.function.params[0].flow,
        ValueFlow::ReturnedChanged
    );
    assert_eq!(with_balance.function.params[0].ty, user);
    assert_eq!(with_balance.function.params[1].ty, u32_ty);
    assert_eq!(with_balance.function.output, user);
}

#[test]
fn lowers_trait_impl_method_names_without_lowering_trait_semantics() {
    let module = frontend::parse_module(
        r#"
        struct User { id: u32 }

        trait Eq {
          fn eq(self: User, other: User) -> Bool
        }

        impl Eq for User {
          fn eq(self, other: User) -> Bool {
            true
          }
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_signatures(&module).unwrap();
    let user = lowered.types.type_id("User").unwrap();
    let bool_ty = lowered.types.type_id("Bool").unwrap();

    assert_eq!(lowered.methods.len(), 1);
    let method = &lowered.methods[0];
    assert_eq!(method.owner, user);
    assert_eq!(method.method, "eq");
    assert_eq!(method.function.name, "User.Eq.eq");
    assert_eq!(method.function.output, bool_ty);
    assert_eq!(
        lowered.program.function_id("User.Eq.eq"),
        Some(method.function.id)
    );
}

#[test]
fn signature_lowering_rejects_duplicate_names_and_generic_functions() {
    let module = frontend::parse_module(
        r#"
        global root: u32
        fn root(x: u32) -> u32 { x }
        "#,
    )
    .unwrap();
    assert_eq!(
        frontend::lower_module_signatures(&module).unwrap_err(),
        frontend::LowerError::Core(CoreError::DuplicateFunctionName("root".into()))
    );

    let module = frontend::parse_module("fn id<T>(x: T) -> T { x }").unwrap();
    assert_eq!(
        frontend::lower_module_signatures(&module).unwrap_err(),
        frontend::LowerError::UnsupportedGenericDecl { name: "id".into() }
    );
}

#[test]
fn lowers_and_runs_simple_arithmetic_bodies() {
    let module = frontend::parse_module(
        r#"
        fn add(x: u32, y: u32) -> u32 {
          x + y
        }

        fn add_one(x: u32) -> u32 {
          add(x, 1)
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let add_one = lowered.program.function_id("add_one").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(add_one, vec![Value::Finite(41)])
        .unwrap();

    assert_eq!(result, vec![Value::Finite(42)]);
}

#[test]
fn lowers_let_bindings_globals_and_comparisons() {
    let module = frontend::parse_module(
        r#"
        global root: u32

        fn below_root(x: u32) -> Bool {
          let limit: u32 = root
          x < limit
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("below_root").unwrap();
    let core_function = lowered.program.get(function).unwrap();

    assert_eq!(core_function.inputs.len(), 1);
    assert_eq!(
        core_function.outputs,
        vec![lowered.types.type_id("Bool").unwrap()]
    );
    assert_eq!(core_function.returns.len(), 1);
}

#[test]
fn lowers_product_constructor_bodies() {
    let module = frontend::parse_module(
        r#"
        struct Pair { left: u32, right: u32 }

        fn make_pair(left: u32, right: u32) -> Pair {
          Pair { left: left, right: right }
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let make_pair = lowered.program.function_id("make_pair").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(make_pair, vec![Value::Finite(3), Value::Finite(5)])
        .unwrap();

    assert_eq!(
        result,
        vec![Value::Product(vec![Value::Finite(3), Value::Finite(5)])]
    );
}

#[test]
fn body_lowering_rejects_unsupported_expressions_and_linear_leaks() {
    let module = frontend::parse_module(
        r#"
        struct Pair { left: u32, right: u32 }

        fn bad(pair: Pair) -> u32 {
          pair.left
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::UnsupportedExpression("expression form is not lowered yet")
    );

    let module = frontend::parse_module(
        r#"
        fn leak(x: u32, y: u32) -> u32 {
          x
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::Core(CoreError::LiveValueAtEnd(linear::ValueId(1)))
    );
}
