use linear::{
    BuiltinOp, CoreProgram, Evaluator, Expr, FlowViolation, Function, FunctionFlow, Param,
    ParamContract, Provenance, Statement, TypeStore, Value, ValueId, check_function_contract,
    infer_function_flows,
};

fn u32_type(types: &mut TypeStore) -> linear::TypeId {
    types.add_uint("U32", 32).unwrap()
}

fn flag_type(types: &mut TypeStore) -> linear::TypeId {
    let unit = types.unit();
    types
        .add_sum(
            Some("Flag".into()),
            vec![
                linear::Component::named("off", unit),
                linear::Component::named("on", unit),
            ],
            linear::DeclaredCapabilities::linear(),
        )
        .unwrap()
}

#[test]
fn returned_params_are_same_version_even_when_swapped() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    let swap = program
        .add_function(Function {
            name: Some("swap".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), u32_ty),
            ],
            outputs: vec![u32_ty, u32_ty],
            body: vec![],
            returns: vec![ValueId(1), ValueId(0)],
        })
        .unwrap();
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    assert_eq!(
        flows[&swap].outputs,
        vec![Provenance::Param(1), Provenance::Param(0)]
    );

    // A swapped return is NOT a valid borrow of either parameter.
    let violations = check_function_contract(
        "swap",
        &[
            ("a".into(), ParamContract::Borrowed),
            ("b".into(), ParamContract::Borrowed),
        ],
        &flows[&swap],
    );
    assert_eq!(violations.len(), 2);
    assert!(matches!(
        &violations[0],
        FlowViolation::BorrowNotProven { function, param } if function == "swap" && param == "a"
    ));
}

#[test]
fn dup_propagates_the_source_version_to_both_copies() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    let both = program
        .add_function(Function {
            name: Some("both".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty, u32_ty],
            body: vec![Statement::new(
                vec![ValueId(1), ValueId(2)],
                Expr::Dup { value: ValueId(0) },
            )],
            returns: vec![ValueId(1), ValueId(2)],
        })
        .unwrap();
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    assert_eq!(
        flows[&both].outputs,
        vec![Provenance::Param(0), Provenance::Param(0)]
    );
}

#[test]
fn observer_builtins_thread_operands_and_produce_fresh_results() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    let add = program
        .add_function(Function {
            name: Some("add".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), u32_ty),
            ],
            outputs: vec![u32_ty, u32_ty, u32_ty],
            body: vec![Statement::new(
                vec![ValueId(2), ValueId(3), ValueId(4)],
                Expr::Builtin {
                    op: BuiltinOp::FiniteAdd { ty: u32_ty },
                    args: vec![ValueId(0), ValueId(1)],
                },
            )],
            returns: vec![ValueId(2), ValueId(3), ValueId(4)],
        })
        .unwrap();
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    assert_eq!(
        flows[&add].outputs,
        vec![
            Provenance::Param(0),
            Provenance::Param(1),
            Provenance::Other
        ]
    );
}

#[test]
fn finite_next_is_a_changed_version_and_satisfies_mut() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    let bump = program
        .add_function(Function {
            name: Some("bump".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty],
            body: vec![Statement::new(
                vec![ValueId(1)],
                Expr::Builtin {
                    op: BuiltinOp::FiniteNext { ty: u32_ty },
                    args: vec![ValueId(0)],
                },
            )],
            returns: vec![ValueId(1)],
        })
        .unwrap();
    // A wrapper's mut-ness is inherited through the call to bump.
    let wrapper = program
        .add_function(Function {
            name: Some("wrapper".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty],
            body: vec![Statement::new(
                vec![ValueId(1)],
                Expr::Call {
                    function: bump,
                    args: vec![ValueId(0)],
                },
            )],
            returns: vec![ValueId(1)],
        })
        .unwrap();
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    assert_eq!(flows[&bump].outputs, vec![Provenance::Other]);
    assert_eq!(flows[&wrapper].outputs, vec![Provenance::Other]);

    // `mut` is accurate for both; a borrow claim would be rejected.
    for id in [bump, wrapper] {
        assert!(
            check_function_contract("f", &[("x".into(), ParamContract::Updated)], &flows[&id])
                .is_empty()
        );
        assert_eq!(
            check_function_contract("f", &[("x".into(), ParamContract::Borrowed)], &flows[&id])
                .len(),
            1
        );
    }

    // Evaluator semantics: next is +1 modulo the cardinality.
    let evaluator = Evaluator::new(&types, &program);
    assert_eq!(
        evaluator.run_function(bump, vec![Value::Finite(41)]).unwrap(),
        vec![Value::Finite(42)]
    );
    assert_eq!(
        evaluator
            .run_function(bump, vec![Value::Finite((1 << 32) - 1)])
            .unwrap(),
        vec![Value::Finite(0)]
    );
}

#[test]
fn same_version_composes_through_call_chains() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    let swap = program
        .add_function(Function {
            name: Some("swap".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), u32_ty),
            ],
            outputs: vec![u32_ty, u32_ty],
            body: vec![],
            returns: vec![ValueId(1), ValueId(0)],
        })
        .unwrap();
    // Swapping twice restores each parameter's own version.
    let double_swap = program
        .add_function(Function {
            name: Some("double_swap".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), u32_ty),
            ],
            outputs: vec![u32_ty, u32_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(2), ValueId(3)],
                    Expr::Call {
                        function: swap,
                        args: vec![ValueId(0), ValueId(1)],
                    },
                ),
                Statement::new(
                    vec![ValueId(4), ValueId(5)],
                    Expr::Call {
                        function: swap,
                        args: vec![ValueId(2), ValueId(3)],
                    },
                ),
            ],
            returns: vec![ValueId(4), ValueId(5)],
        })
        .unwrap();
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    assert_eq!(
        flows[&double_swap].outputs,
        vec![Provenance::Param(0), Provenance::Param(1)]
    );
    assert!(
        check_function_contract(
            "double_swap",
            &[
                ("a".into(), ParamContract::Borrowed),
                ("b".into(), ParamContract::Borrowed),
            ],
            &flows[&double_swap],
        )
        .is_empty()
    );
}

#[test]
fn match_joins_meet_across_arms() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let flag_ty = flag_type(&mut types);
    let mut program = CoreProgram::new();

    // Both arms return the captured parameter: still the same version.
    let threaded = program
        .add_function(Function {
            name: Some("threaded".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), flag_ty),
            ],
            outputs: vec![u32_ty],
            body: vec![Statement::new(
                vec![ValueId(2)],
                Expr::Match {
                    scrutinee: ValueId(1),
                    arms: vec![
                        linear::MatchArm::new(
                            0,
                            ValueId(3),
                            vec![Statement::new(vec![], Expr::Zap { value: ValueId(3) })],
                            vec![ValueId(0)],
                        ),
                        linear::MatchArm::new(
                            1,
                            ValueId(4),
                            vec![Statement::new(vec![], Expr::Zap { value: ValueId(4) })],
                            vec![ValueId(0)],
                        ),
                    ],
                },
            )],
            returns: vec![ValueId(2)],
        })
        .unwrap();

    // One arm substitutes a fresh literal: the join degrades to Other.
    let replaced = program
        .add_function(Function {
            name: Some("replaced".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), flag_ty),
            ],
            outputs: vec![u32_ty],
            body: vec![Statement::new(
                vec![ValueId(2)],
                Expr::Match {
                    scrutinee: ValueId(1),
                    arms: vec![
                        linear::MatchArm::new(
                            0,
                            ValueId(3),
                            vec![Statement::new(vec![], Expr::Zap { value: ValueId(3) })],
                            vec![ValueId(0)],
                        ),
                        linear::MatchArm::new(
                            1,
                            ValueId(4),
                            vec![
                                Statement::new(vec![], Expr::Zap { value: ValueId(4) }),
                                Statement::new(vec![], Expr::Zap { value: ValueId(0) }),
                                Statement::new(
                                    vec![ValueId(5)],
                                    Expr::FiniteLiteral {
                                        ty: u32_ty,
                                        value: 7,
                                    },
                                ),
                            ],
                            vec![ValueId(5)],
                        ),
                    ],
                },
            )],
            returns: vec![ValueId(2)],
        })
        .unwrap();
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    assert_eq!(flows[&threaded].outputs, vec![Provenance::Param(0)]);
    assert_eq!(flows[&replaced].outputs, vec![Provenance::Other]);
}

#[test]
fn bare_recursion_converges_and_satisfies_any_contract_vacuously() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    let forever = program
        .add_function(Function {
            name: Some("forever".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty],
            body: vec![Statement::new(
                vec![ValueId(1)],
                Expr::Call {
                    function: linear::FunctionId(0),
                    args: vec![ValueId(0)],
                },
            )],
            returns: vec![ValueId(1)],
        })
        .unwrap();
    assert_eq!(forever, linear::FunctionId(0));
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    // No terminating path constrains the output: Top survives the fixpoint
    // and any marker holds vacuously.
    assert_eq!(flows[&forever].outputs, vec![Provenance::Top]);
    assert!(
        check_function_contract(
            "forever",
            &[("x".into(), ParamContract::Borrowed)],
            &flows[&forever],
        )
        .is_empty()
    );
}

#[test]
fn contract_slots_skip_consumed_params() {
    // fn f(take a, b) -> ...: the single hidden slot belongs to `b`.
    let flow = FunctionFlow {
        outputs: vec![Provenance::Param(1)],
    };
    assert!(
        check_function_contract(
            "f",
            &[
                ("a".into(), ParamContract::Consumed),
                ("b".into(), ParamContract::Borrowed),
            ],
            &flow,
        )
        .is_empty()
    );

    // A mut param whose slot is provably its own version is reported.
    let violations = check_function_contract(
        "f",
        &[
            ("a".into(), ParamContract::Consumed),
            ("b".into(), ParamContract::Updated),
        ],
        &flow,
    );
    assert_eq!(
        violations,
        vec![FlowViolation::MutIsBorrow {
            function: "f".into(),
            param: "b".into(),
        }]
    );
}

#[test]
fn take_params_that_escape_unchanged_are_reported() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    // Pure move-through: the "taken" value is the visible result.
    let id = program
        .add_function(Function {
            name: Some("id".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty],
            body: vec![],
            returns: vec![ValueId(0)],
        })
        .unwrap();
    program.check(&types).unwrap();

    let flows = infer_function_flows(&program);
    assert_eq!(
        check_function_contract("id", &[("x".into(), ParamContract::Consumed)], &flows[&id]),
        vec![FlowViolation::TakeIsBorrow {
            function: "id".into(),
            param: "x".into(),
        }]
    );

    // A take that is genuinely changed before returning is fine.
    let bumped = FunctionFlow {
        outputs: vec![Provenance::Other],
    };
    assert!(
        check_function_contract("bump", &[("x".into(), ParamContract::Consumed)], &bumped)
            .is_empty()
    );

    // A take escaping into ANOTHER param's slot is also reported.
    let crossed = FunctionFlow {
        outputs: vec![Provenance::Param(1), Provenance::Other],
    };
    let violations = check_function_contract(
        "f",
        &[
            ("kept".into(), ParamContract::Borrowed),
            ("gone".into(), ParamContract::Consumed),
        ],
        &crossed,
    );
    assert!(violations.contains(&FlowViolation::TakeIsBorrow {
        function: "f".into(),
        param: "gone".into(),
    }));
}
