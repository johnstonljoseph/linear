use linear::frontend::ValueFlow;
use linear::{
    Capabilities, CollectionMutability, Component, ComponentName, CoreError, Evaluator, TypeError,
    TypeKind, Value, frontend,
};

fn lower(src: &str) -> linear::TypeStore {
    let module = frontend::parse_module(src).unwrap();
    frontend::lower_type_items(&module).unwrap().types
}

#[test]
fn lowers_type_aliases_structs_and_enums() {
    let types = lower(
        r#"
        type UserId = U32
        type Balance = U32

        struct User { id: UserId, balance: Balance }

        enum Decision {
          allow { reason: U32 },
          deny,
          review { queue: U32, priority: U32 },
        }
        "#,
    );

    let u32_ty = types.type_id("U32").unwrap();
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
        struct MyInt(U32)
        type Pair = (U32, U32)
        struct UsesPair { left: Pair, right: Pair }
        "#,
    );

    let u32_ty = types.type_id("U32").unwrap();
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
        type UserId = U32
        struct User { id: UserId, balance: U32 }
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

    let module = frontend::parse_module("struct Bad { xs: List<U32, U32> }").unwrap();
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

    let module = frontend::parse_module("type U32 = U16").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::Type(TypeError::DuplicateName("U32".into()))
    );

    let module = frontend::parse_module("struct Bad { value: U32 }: Eq").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::UnknownCapability("Eq".into())
    );

    let module = frontend::parse_module("type Alias = U32: Dup").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::UnsupportedAliasCapabilities {
            name: "Alias".into()
        }
    );
}

#[test]
fn rejects_declared_capabilities_that_exceed_structural_capabilities() {
    let module = frontend::parse_module("struct Bad { work: MutList<U32> }: Dup").unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::Type(TypeError::DeclaredCapabilityExceedsStructural {
            declared: Capabilities {
                dup: true,
                zap: false,
            },
            structural: Capabilities::linear(),
        })
    );

    let module = frontend::parse_module(
        r#"
        enum Bad {
          item(MutList<U32>),
        }: Zap
        "#,
    )
    .unwrap();
    assert_eq!(
        frontend::lower_type_items(&module).unwrap_err(),
        frontend::LowerError::Type(TypeError::DeclaredCapabilityExceedsStructural {
            declared: Capabilities {
                dup: false,
                zap: true,
            },
            structural: Capabilities::linear(),
        })
    );
}

#[test]
fn lowers_global_and_function_signatures() {
    let module = frontend::parse_module(
        r#"
        type UserId = U32
        struct User { id: UserId, balance: U32 }
        global root: User

        fn decide(mut user: User, config: U32, take event: UserId) -> Bool {
          true
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_signatures(&module).unwrap();
    let user = lowered.types.type_id("User").unwrap();
    let bool_ty = lowered.types.type_id("Bool").unwrap();
    let u32_ty = lowered.types.type_id("U32").unwrap();

    assert_eq!(lowered.globals.len(), 1);
    assert_eq!(lowered.globals[0].name, "root");
    assert_eq!(lowered.globals[0].ty, user);
    assert_eq!(
        lowered.program.global_decl_id("root"),
        Some(lowered.globals[0].id)
    );

    assert_eq!(lowered.functions.len(), 1);
    let decide = &lowered.functions[0];
    assert_eq!(decide.name, "decide");
    assert_eq!(decide.output, bool_ty);
    assert_eq!(lowered.program.function_id("decide"), Some(decide.id));
    assert_eq!(decide.params.len(), 3);
    assert_eq!(decide.params[0].flow, ValueFlow::ReturnedChanged);
    assert_eq!(decide.params[0].name, "user");
    assert_eq!(decide.params[0].ty, user);
    assert_eq!(decide.params[1].flow, ValueFlow::ReturnedUnchanged);
    assert_eq!(decide.params[1].name, "config");
    assert_eq!(decide.params[1].ty, u32_ty);
    assert_eq!(decide.params[2].flow, ValueFlow::NotReturned);
    assert_eq!(decide.params[2].name, "event");
    assert_eq!(decide.params[2].ty, u32_ty);

    let shell = lowered.program.get(decide.id).unwrap();
    assert_eq!(shell.inputs.len(), 3);
    assert_eq!(shell.outputs, vec![user, u32_ty, bool_ty]);
    assert!(shell.body.is_empty());
    assert!(shell.returns.is_empty());
}

#[test]
fn lowers_impl_method_signatures_with_expanded_self() {
    let module = frontend::parse_module(
        r#"
        struct User { id: U32, balance: U32 }

        impl User {
          fn balance(self) -> U32 {
            self.balance
          }

          fn with_balance(mut self, take balance: U32) -> () {
            ()
          }
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_signatures(&module).unwrap();
    let user = lowered.types.type_id("User").unwrap();
    let u32_ty = lowered.types.type_id("U32").unwrap();

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
    assert_eq!(with_balance.function.params[1].flow, ValueFlow::NotReturned);
    assert_eq!(with_balance.function.params[1].ty, u32_ty);
    assert_eq!(with_balance.function.output, lowered.types.unit());
    assert_eq!(with_balance.function.core_outputs, vec![user]);
}

#[test]
fn lowers_trait_impl_method_names_without_lowering_trait_semantics() {
    let module = frontend::parse_module(
        r#"
        struct User { id: U32 }

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
        global root: U32
        fn root(x: U32) -> U32 { x }
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
        fn add(take x: U32, take y: U32) -> U32 {
          x + y
        }

        fn add_one(take x: U32) -> U32 {
          add(take x, 1)
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
fn infix_ops_thread_returned_unchanged_params_without_dup() {
    let module = frontend::parse_module(
        r#"
        fn below_ten(x: U32) -> Bool {
          x < 10
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("below_ten").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(function, vec![Value::Finite(7)])
        .unwrap();

    assert_eq!(
        result,
        vec![
            Value::Finite(7),
            Value::Sum {
                variant: 1,
                payload: Box::new(Value::Unit),
            },
        ]
    );
}

#[test]
fn infix_ops_rebind_returned_local_operands() {
    let module = frontend::parse_module(
        r#"
        fn two_reads(take x: U32) -> U32 {
          let y = x
          let a = y + 1
          let b = y + 2
          b
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("two_reads").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(function, vec![Value::Finite(10)])
        .unwrap();

    assert_eq!(result, vec![Value::Finite(12)]);
}

#[test]
fn infix_ops_report_duplicate_linear_operands() {
    let module = frontend::parse_module(
        r#"
        fn double(take x: U32) -> U32 {
          x + x
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::DuplicateLinearUse("x".into())
    );
}

#[test]
fn infix_ops_report_names_moved_inside_rhs() {
    let module = frontend::parse_module(
        r#"
        fn bump(take x: U32) -> U32 {
          x + 1
        }

        fn bad(take x: U32) -> U32 {
          x + bump(take x)
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::ValueMovedDuringExpression("x".into())
    );
}

#[test]
fn body_lowering_does_not_auto_dup_visible_returns() {
    let module = frontend::parse_module(
        r#"
        fn copy_return(x: U32) -> U32 {
          x
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::Core(CoreError::ConsumedValue(linear::ValueId(0)))
    );
}

#[test]
fn calls_rebind_hidden_returned_arguments() {
    let module = frontend::parse_module(
        r#"
        fn pass(mut state: U32, config: U32, take event: U32) -> U32 {
          event
        }

        fn caller(mut state: U32, config: U32, take event: U32) -> U32 {
          pass(mut state, config, take event)
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("caller").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(
            function,
            vec![Value::Finite(1), Value::Finite(2), Value::Finite(3)],
        )
        .unwrap();

    assert_eq!(
        result,
        vec![Value::Finite(1), Value::Finite(2), Value::Finite(3)]
    );
}

#[test]
fn bare_call_statements_are_separated_by_newlines() {
    let module = frontend::parse_module(
        r#"
        fn touch(x: U32) -> () {
          ()
        }

        fn caller(x: U32) -> U32 {
          touch(x)
          x + 1
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("caller").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(function, vec![Value::Finite(10)])
        .unwrap();

    assert_eq!(result, vec![Value::Finite(10), Value::Finite(11)]);
}

#[test]
fn calls_auto_zap_hidden_returns_for_temporary_arguments() {
    let module = frontend::parse_module(
        r#"
        fn make(take x: U32) -> U32 {
          x + 1
        }

        fn observe(x: U32) -> U32 {
          x + 1
        }

        fn caller(take x: U32) -> U32 {
          observe(make(take x))
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("caller").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(function, vec![Value::Finite(40)])
        .unwrap();

    assert_eq!(result, vec![Value::Finite(42)]);
}

#[test]
fn body_lowering_rejects_mutating_an_unchanged_threaded_param() {
    let module = frontend::parse_module(
        r#"
        fn touch(mut x: U32) -> () {
          ()
        }

        fn bad(x: U32) -> () {
          touch(x)
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::FlowMismatch {
            expected: ValueFlow::ReturnedUnchanged,
            actual: ValueFlow::ReturnedChanged,
        }
    );
}

#[test]
fn lowers_let_bindings_globals_and_comparisons() {
    let module = frontend::parse_module(
        r#"
        global root: U32

        fn below_root(x: U32) -> Bool {
          let limit: U32 = root
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
        vec![
            lowered.types.type_id("U32").unwrap(),
            lowered.types.type_id("Bool").unwrap(),
        ]
    );
    assert_eq!(core_function.returns.len(), 2);
}

#[test]
fn lowers_product_constructor_bodies() {
    let module = frontend::parse_module(
        r#"
        struct Pair { left: U32, right: U32 }

        fn make_pair(take left: U32, take right: U32) -> Pair {
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
fn lowers_enum_constructors() {
    let module = frontend::parse_module(
        r#"
        enum MaybeU32 { none, some(U32) }

        fn make_none() -> MaybeU32 {
          MaybeU32.none
        }

        fn make_some(take value: U32) -> MaybeU32 {
          MaybeU32.some(value)
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let make_none = lowered.program.function_id("make_none").unwrap();
    let make_some = lowered.program.function_id("make_some").unwrap();
    let evaluator = Evaluator::new(&lowered.types, &lowered.program);

    assert_eq!(
        evaluator.run_function(make_none, vec![]).unwrap(),
        vec![Value::Sum {
            variant: 0,
            payload: Box::new(Value::Unit),
        }]
    );
    assert_eq!(
        evaluator
            .run_function(make_some, vec![Value::Finite(9)])
            .unwrap(),
        vec![Value::Sum {
            variant: 1,
            payload: Box::new(Value::Finite(9)),
        }]
    );
}

#[test]
fn lowers_match_with_record_payload_patterns() {
    let module = frontend::parse_module(
        r#"
        enum Decision {
          allow { reason: U32 },
          deny,
        }

        fn reason(take decision: Decision) -> U32 {
          match decision {
            .allow { reason }: reason,
            .deny: 0,
          }
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let reason = lowered.program.function_id("reason").unwrap();
    let decision = Value::Sum {
        variant: 0,
        payload: Box::new(Value::Product(vec![Value::Finite(12)])),
    };
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(reason, vec![decision])
        .unwrap();

    assert_eq!(result, vec![Value::Finite(12)]);
}

#[test]
fn match_branches_thread_unchanged_params() {
    let module = frontend::parse_module(
        r#"
        enum MaybeU32 { none, some(U32) }

        fn score(config: U32, take value: MaybeU32) -> U32 {
          match value {
            .none: config + 1,
            .some(x): x + config,
          }
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let score = lowered.program.function_id("score").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(
            score,
            vec![
                Value::Finite(4),
                Value::Sum {
                    variant: 1,
                    payload: Box::new(Value::Finite(10)),
                },
            ],
        )
        .unwrap();

    assert_eq!(result, vec![Value::Finite(4), Value::Finite(14)]);
}

#[test]
fn match_branches_thread_live_locals() {
    let module = frontend::parse_module(
        r#"
        enum MaybeU32 { none, some(U32) }

        fn after_match(take value: MaybeU32) -> U32 {
          let y = 5
          let z = match value {
            .none: 1,
            .some(x): x + 2,
          }
          y + z
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("after_match").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(
            function,
            vec![Value::Sum {
                variant: 1,
                payload: Box::new(Value::Finite(10)),
            }],
        )
        .unwrap();

    assert_eq!(result, vec![Value::Finite(17)]);
}

#[test]
fn match_expression_synthesizes_visible_result_type() {
    let module = frontend::parse_module(
        r#"
        enum MaybeU32 { none, some(U32) }

        fn score(take value: MaybeU32) -> U32 {
          let z = match value {
            .none: 1,
            .some(x): x + 2,
          }
          z
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("score").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(
            function,
            vec![Value::Sum {
                variant: 0,
                payload: Box::new(Value::Unit),
            }],
        )
        .unwrap();

    assert_eq!(result, vec![Value::Finite(1)]);
}

#[test]
fn match_expression_synthesizes_payload_derived_result_type() {
    let module = frontend::parse_module(
        r#"
        enum EitherU32 { left(U32), right(U32) }

        fn pick(take value: EitherU32) -> U32 {
          let z = match value {
            .left(v): v,
            .right(w): w,
          }
          z
        }
        "#,
    )
    .unwrap();

    let lowered = frontend::lower_module_bodies(&module).unwrap();
    let function = lowered.program.function_id("pick").unwrap();
    let result = Evaluator::new(&lowered.types, &lowered.program)
        .run_function(
            function,
            vec![Value::Sum {
                variant: 1,
                payload: Box::new(Value::Finite(42)),
            }],
        )
        .unwrap();

    assert_eq!(result, vec![Value::Finite(42)]);
}

#[test]
fn body_lowering_rejects_unsupported_expressions_and_linear_leaks() {
    let module = frontend::parse_module(
        r#"
        struct Pair { left: U32, right: U32 }

        fn bad(pair: Pair) -> U32 {
          pair.left
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::UnsupportedExpression("field access is not lowered yet")
    );

    let module = frontend::parse_module(
        r#"
        fn leak(take x: MutList<U32>, take y: MutList<U32>) -> MutList<U32> {
          x
        }
        "#,
    )
    .unwrap();

    assert!(matches!(
        frontend::lower_module_bodies(&module).unwrap_err(),
        frontend::LowerError::DeadLinearLocal { name, .. } if name == "y"
    ));
}

#[test]
fn body_lowering_reports_dead_linear_local_by_name() {
    let module = frontend::parse_module(
        r#"
        fn leak_local(take start: MutList<U32>) -> () {
          let h = start
          ()
        }
        "#,
    )
    .unwrap();

    let err = frontend::lower_module_bodies(&module).unwrap_err();
    assert!(
        matches!(&err, frontend::LowerError::DeadLinearLocal { name, .. } if name == "h"),
        "{err:?}"
    );
}
