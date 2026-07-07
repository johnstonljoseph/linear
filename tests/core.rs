use linear::{
    BuiltinOp, Component, CoreError, CoreProgram, DeclaredCapabilities, Evaluator, Expr, Function,
    GlobalDef, GlobalExpr, MatchArm, Param, Statement, TypeStore, Value, ValueId,
};

fn token_type(types: &mut TypeStore) -> linear::TypeId {
    types
        .add_primitive("Token", DeclaredCapabilities::linear())
        .unwrap()
}

fn bool_type(types: &mut TypeStore) -> linear::TypeId {
    types
        .add_sum(
            Some("Bool".into()),
            vec![
                Component::named("false", types.unit()),
                Component::named("true", types.unit()),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap()
}

fn u32_type(types: &mut TypeStore) -> linear::TypeId {
    types.add_uint("U32", 32).unwrap()
}

#[test]
fn identity_function_checks() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("id".into()),
            inputs: vec![Param::new(ValueId(0), token)],
            outputs: vec![token],
            body: vec![],
            returns: vec![ValueId(0)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn duplicate_linear_use_is_rejected() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let pair = types
        .add_product(
            Some("Pair".into()),
            vec![
                Component::positional(0, token),
                Component::positional(1, token),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad".into()),
            inputs: vec![Param::new(ValueId(0), token)],
            outputs: vec![pair],
            body: vec![Statement::new(
                vec![ValueId(1)],
                Expr::Product {
                    ty: pair,
                    fields: vec![ValueId(0), ValueId(0)],
                },
            )],
            returns: vec![ValueId(1)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::ConsumedValue(ValueId(0)))
    );
}

#[test]
fn dup_requires_dup_capability() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_dup".into()),
            inputs: vec![Param::new(ValueId(0), token)],
            outputs: vec![token, token],
            body: vec![Statement::new(
                vec![ValueId(1), ValueId(2)],
                Expr::Dup { value: ValueId(0) },
            )],
            returns: vec![ValueId(1), ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Err(CoreError::CannotDup(token)));
}

#[test]
fn dup_can_split_copyable_values() {
    let types = TypeStore::new();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("dup_unit".into()),
            inputs: vec![Param::new(ValueId(0), types.unit())],
            outputs: vec![types.unit(), types.unit()],
            body: vec![Statement::new(
                vec![ValueId(1), ValueId(2)],
                Expr::Dup { value: ValueId(0) },
            )],
            returns: vec![ValueId(1), ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn zap_requires_zap_capability() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_zap".into()),
            inputs: vec![Param::new(ValueId(0), token)],
            outputs: vec![],
            body: vec![Statement::new(vec![], Expr::Zap { value: ValueId(0) })],
            returns: vec![],
        })
        .unwrap();

    assert_eq!(program.check(&types), Err(CoreError::CannotZap(token)));
}

#[test]
fn zap_can_drop_droppable_values() {
    let mut types = TypeStore::new();
    let droppable = types
        .add_primitive("Droppable", DeclaredCapabilities::zap())
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("drop".into()),
            inputs: vec![Param::new(ValueId(0), droppable)],
            outputs: vec![],
            body: vec![Statement::new(vec![], Expr::Zap { value: ValueId(0) })],
            returns: vec![],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn live_values_must_be_returned_or_consumed() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("leak".into()),
            inputs: vec![Param::new(ValueId(0), token), Param::new(ValueId(1), token)],
            outputs: vec![token],
            body: vec![],
            returns: vec![ValueId(0)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::LiveValueAtEnd(ValueId(1)))
    );
}

#[test]
fn calls_consume_arguments_and_produce_declared_outputs() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    let id = program
        .add_function(Function {
            name: Some("id".into()),
            inputs: vec![Param::new(ValueId(0), token)],
            outputs: vec![token],
            body: vec![],
            returns: vec![ValueId(0)],
        })
        .unwrap();
    program
        .add_function(Function {
            name: Some("caller".into()),
            inputs: vec![Param::new(ValueId(10), token)],
            outputs: vec![token],
            body: vec![Statement::new(
                vec![ValueId(11)],
                Expr::Call {
                    function: id,
                    args: vec![ValueId(10)],
                },
            )],
            returns: vec![ValueId(11)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn functions_can_reference_global_values() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    let global = program
        .add_global_decl(linear::GlobalDecl::new("root", token))
        .unwrap();
    program
        .add_function(Function {
            name: Some("read_root".into()),
            inputs: vec![],
            outputs: vec![token],
            body: vec![Statement::new(vec![ValueId(0)], Expr::Global { global })],
            returns: vec![ValueId(0)],
        })
        .unwrap();

    assert_eq!(program.global_decl_id("root"), Some(global));
    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn repeated_global_references_do_not_consume_the_global_symbol() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let pair = types
        .add_product(
            Some("Pair".into()),
            vec![
                Component::positional(0, token),
                Component::positional(1, token),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    let global = program
        .add_global_decl(linear::GlobalDecl::new("root", token))
        .unwrap();
    program
        .add_function(Function {
            name: Some("pair_roots".into()),
            inputs: vec![],
            outputs: vec![pair],
            body: vec![
                Statement::new(vec![ValueId(0)], Expr::Global { global }),
                Statement::new(vec![ValueId(1)], Expr::Global { global }),
                Statement::new(
                    vec![ValueId(2)],
                    Expr::Product {
                        ty: pair,
                        fields: vec![ValueId(0), ValueId(1)],
                    },
                ),
            ],
            returns: vec![ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn global_and_function_names_share_one_namespace() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_global_decl(linear::GlobalDecl::new("item", token))
        .unwrap();

    assert_eq!(
        program
            .add_function(Function {
                name: Some("item".into()),
                inputs: vec![],
                outputs: vec![],
                body: vec![],
                returns: vec![],
            })
            .unwrap_err(),
        CoreError::DuplicateFunctionName("item".into())
    );
}

#[test]
fn global_decls_require_non_empty_names() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();

    assert_eq!(
        program
            .add_global_decl(linear::GlobalDecl::new("", token))
            .unwrap_err(),
        CoreError::EmptyName
    );
}

#[test]
fn global_defs_can_define_literal_trees() {
    let mut types = TypeStore::new();
    let bit = types
        .add_finite(Some("Bit".into()), 2, DeclaredCapabilities::dup_zap())
        .unwrap();
    let pair = types
        .add_product(
            Some("Pair".into()),
            vec![
                Component::named("left", bit),
                Component::named("right", types.unit()),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    let global = program
        .add_global_def(GlobalDef::new(
            "pair",
            pair,
            GlobalExpr::Product {
                ty: pair,
                fields: vec![
                    GlobalExpr::FiniteLiteral { ty: bit, value: 1 },
                    GlobalExpr::Unit,
                ],
            },
        ))
        .unwrap();

    assert_eq!(program.global_decl_id("pair"), Some(global));
    assert_eq!(program.get_global_def(global).is_some(), true);
    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn global_defs_must_match_declared_type() {
    let mut types = TypeStore::new();
    let bit = types
        .add_finite(Some("Bit".into()), 2, DeclaredCapabilities::dup_zap())
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_global_def(GlobalDef::new("bad", bit, GlobalExpr::Unit))
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::TypeMismatch {
            expected: bit,
            actual: types.unit(),
        })
    );
}

#[test]
fn global_defs_check_literal_bounds() {
    let mut types = TypeStore::new();
    let bit = types
        .add_finite(Some("Bit".into()), 2, DeclaredCapabilities::dup_zap())
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_global_def(GlobalDef::new(
            "bad_bit",
            bit,
            GlobalExpr::FiniteLiteral { ty: bit, value: 2 },
        ))
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::FiniteLiteralOutOfRange {
            ty: bit,
            value: 2,
            values: 2,
        })
    );
}

#[test]
fn functions_can_reference_global_defs() {
    let mut types = TypeStore::new();
    let bit = types
        .add_finite(Some("Bit".into()), 2, DeclaredCapabilities::dup_zap())
        .unwrap();
    let mut program = CoreProgram::new();
    let global = program
        .add_global_def(GlobalDef::new(
            "one",
            bit,
            GlobalExpr::FiniteLiteral { ty: bit, value: 1 },
        ))
        .unwrap();
    program
        .add_function(Function {
            name: Some("read_one".into()),
            inputs: vec![],
            outputs: vec![bit],
            body: vec![Statement::new(vec![ValueId(0)], Expr::Global { global })],
            returns: vec![ValueId(0)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn global_defs_can_define_static_function_values() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let fn_ty = types
        .add_function(Some("U32ToU32".into()), u32_ty, u32_ty)
        .unwrap();
    let mut program = CoreProgram::new();
    let inc = program
        .add_function(Function {
            name: Some("inc".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(1)],
                    Expr::FiniteLiteral {
                        ty: u32_ty,
                        value: 1,
                    },
                ),
                Statement::new(
                    vec![ValueId(2), ValueId(3), ValueId(4)],
                    Expr::Builtin {
                        op: BuiltinOp::FiniteAdd { ty: u32_ty },
                        args: vec![ValueId(0), ValueId(1)],
                    },
                ),
                Statement::new(vec![], Expr::Zap { value: ValueId(2) }),
                Statement::new(vec![], Expr::Zap { value: ValueId(3) }),
            ],
            returns: vec![ValueId(4)],
        })
        .unwrap();
    let global = program
        .add_global_def(GlobalDef::new(
            "inc_ref",
            fn_ty,
            GlobalExpr::FunctionRef {
                ty: fn_ty,
                function: inc,
            },
        ))
        .unwrap();
    let apply = program
        .add_function(Function {
            name: Some("apply_global".into()),
            inputs: vec![],
            outputs: vec![u32_ty],
            body: vec![
                Statement::new(vec![ValueId(0)], Expr::Global { global }),
                Statement::new(
                    vec![ValueId(1)],
                    Expr::FiniteLiteral {
                        ty: u32_ty,
                        value: 41,
                    },
                ),
                Statement::new(
                    vec![ValueId(2)],
                    Expr::CallValue {
                        function: ValueId(0),
                        arg: ValueId(1),
                    },
                ),
            ],
            returns: vec![ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(apply, vec![])
            .unwrap(),
        vec![Value::Finite(42)]
    );
}

#[test]
fn global_static_function_values_must_match_their_type() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let fn_ty = types
        .add_function(Some("U32ToU32".into()), u32_ty, u32_ty)
        .unwrap();
    let mut program = CoreProgram::new();
    let unit_id = program
        .add_function(Function {
            name: Some("unit_id".into()),
            inputs: vec![Param::new(ValueId(0), types.unit())],
            outputs: vec![types.unit()],
            body: vec![],
            returns: vec![ValueId(0)],
        })
        .unwrap();
    program
        .add_global_def(GlobalDef::new(
            "bad_ref",
            fn_ty,
            GlobalExpr::FunctionRef {
                ty: fn_ty,
                function: unit_id,
            },
        ))
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::FunctionTypeMismatch {
            function: unit_id,
            ty: fn_ty,
        })
    );
}

#[test]
fn builtin_finite_arithmetic_checks_and_runs() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let mut program = CoreProgram::new();
    let function = program
        .add_function(Function {
            name: Some("add".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), u32_ty),
            ],
            outputs: vec![u32_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(2), ValueId(3), ValueId(4)],
                    Expr::Builtin {
                        op: BuiltinOp::FiniteAdd { ty: u32_ty },
                        args: vec![ValueId(0), ValueId(1)],
                    },
                ),
                Statement::new(vec![], Expr::Zap { value: ValueId(2) }),
                Statement::new(vec![], Expr::Zap { value: ValueId(3) }),
            ],
            returns: vec![ValueId(4)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(function, vec![Value::Finite(40), Value::Finite(2)])
            .unwrap(),
        vec![Value::Finite(42)]
    );
}

#[test]
fn builtin_finite_comparison_returns_bool_sum() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let bool_ty = bool_type(&mut types);
    let mut program = CoreProgram::new();
    let function = program
        .add_function(Function {
            name: Some("lt".into()),
            inputs: vec![
                Param::new(ValueId(0), u32_ty),
                Param::new(ValueId(1), u32_ty),
            ],
            outputs: vec![bool_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(2), ValueId(3), ValueId(4)],
                    Expr::Builtin {
                        op: BuiltinOp::FiniteLt {
                            ty: u32_ty,
                            bool_ty,
                        },
                        args: vec![ValueId(0), ValueId(1)],
                    },
                ),
                Statement::new(vec![], Expr::Zap { value: ValueId(2) }),
                Statement::new(vec![], Expr::Zap { value: ValueId(3) }),
            ],
            returns: vec![ValueId(4)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(function, vec![Value::Finite(1), Value::Finite(2)])
            .unwrap(),
        vec![Value::Sum {
            variant: 1,
            payload: Box::new(Value::Unit),
        }]
    );
}

#[test]
fn recursive_functions_check_and_run() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let bool_ty = bool_type(&mut types);
    let mut program = CoreProgram::new();
    let self_id = linear::FunctionId(0);
    let function = program
        .add_function(Function {
            name: Some("count_down".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(20), ValueId(21)],
                    Expr::Dup { value: ValueId(0) },
                ),
                Statement::new(
                    vec![ValueId(1)],
                    Expr::FiniteLiteral {
                        ty: u32_ty,
                        value: 1,
                    },
                ),
                Statement::new(
                    vec![ValueId(22), ValueId(23), ValueId(2)],
                    Expr::Builtin {
                        op: BuiltinOp::FiniteLt {
                            ty: u32_ty,
                            bool_ty,
                        },
                        args: vec![ValueId(20), ValueId(1)],
                    },
                ),
                Statement::new(vec![], Expr::Zap { value: ValueId(22) }),
                Statement::new(vec![], Expr::Zap { value: ValueId(23) }),
                Statement::new(
                    vec![ValueId(10)],
                    Expr::Match {
                        scrutinee: ValueId(2),
                        arms: vec![
                            MatchArm::new(
                                0,
                                ValueId(3),
                                vec![
                                    Statement::new(vec![], Expr::Zap { value: ValueId(3) }),
                                    Statement::new(
                                        vec![ValueId(4)],
                                        Expr::FiniteLiteral {
                                            ty: u32_ty,
                                            value: 1,
                                        },
                                    ),
                                    Statement::new(
                                        vec![ValueId(22), ValueId(23), ValueId(5)],
                                        Expr::Builtin {
                                            op: BuiltinOp::FiniteSub { ty: u32_ty },
                                            args: vec![ValueId(21), ValueId(4)],
                                        },
                                    ),
                                    Statement::new(vec![], Expr::Zap { value: ValueId(22) }),
                                    Statement::new(vec![], Expr::Zap { value: ValueId(23) }),
                                    Statement::new(
                                        vec![ValueId(6)],
                                        Expr::Call {
                                            function: self_id,
                                            args: vec![ValueId(5)],
                                        },
                                    ),
                                ],
                                vec![ValueId(6)],
                            ),
                            MatchArm::new(
                                1,
                                ValueId(7),
                                vec![
                                    Statement::new(vec![], Expr::Zap { value: ValueId(7) }),
                                    Statement::new(vec![], Expr::Zap { value: ValueId(21) }),
                                    Statement::new(
                                        vec![ValueId(8)],
                                        Expr::FiniteLiteral {
                                            ty: u32_ty,
                                            value: 0,
                                        },
                                    ),
                                ],
                                vec![ValueId(8)],
                            ),
                        ],
                    },
                ),
            ],
            returns: vec![ValueId(10)],
        })
        .unwrap();
    assert_eq!(function, self_id);

    assert_eq!(program.check(&types), Ok(()));
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(function, vec![Value::Finite(3)])
            .unwrap(),
        vec![Value::Finite(0)]
    );
}

#[test]
fn static_function_values_check_and_run() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let fn_ty = types
        .add_function(Some("U32ToU32".into()), u32_ty, u32_ty)
        .unwrap();
    let mut program = CoreProgram::new();
    let inc = program
        .add_function(Function {
            name: Some("inc".into()),
            inputs: vec![Param::new(ValueId(0), u32_ty)],
            outputs: vec![u32_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(1)],
                    Expr::FiniteLiteral {
                        ty: u32_ty,
                        value: 1,
                    },
                ),
                Statement::new(
                    vec![ValueId(2), ValueId(3), ValueId(4)],
                    Expr::Builtin {
                        op: BuiltinOp::FiniteAdd { ty: u32_ty },
                        args: vec![ValueId(0), ValueId(1)],
                    },
                ),
                Statement::new(vec![], Expr::Zap { value: ValueId(2) }),
                Statement::new(vec![], Expr::Zap { value: ValueId(3) }),
            ],
            returns: vec![ValueId(4)],
        })
        .unwrap();
    let apply = program
        .add_function(Function {
            name: Some("apply".into()),
            inputs: vec![],
            outputs: vec![u32_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(0)],
                    Expr::FunctionRef {
                        ty: fn_ty,
                        function: inc,
                    },
                ),
                Statement::new(
                    vec![ValueId(1)],
                    Expr::FiniteLiteral {
                        ty: u32_ty,
                        value: 41,
                    },
                ),
                Statement::new(
                    vec![ValueId(2)],
                    Expr::CallValue {
                        function: ValueId(0),
                        arg: ValueId(1),
                    },
                ),
            ],
            returns: vec![ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(apply, vec![])
            .unwrap(),
        vec![Value::Finite(42)]
    );
}

#[test]
fn static_function_values_pack_multiple_inputs_and_outputs() {
    let mut types = TypeStore::new();
    let u32_ty = u32_type(&mut types);
    let pair_ty = types
        .add_product(
            Some("U32Pair".into()),
            vec![
                Component::positional(0, u32_ty),
                Component::positional(1, u32_ty),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let fn_ty = types
        .add_function(Some("PairToPair".into()), pair_ty, pair_ty)
        .unwrap();
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
    let apply = program
        .add_function(Function {
            name: Some("apply_swap".into()),
            inputs: vec![],
            outputs: vec![pair_ty],
            body: vec![
                Statement::new(
                    vec![ValueId(2)],
                    Expr::FunctionRef {
                        ty: fn_ty,
                        function: swap,
                    },
                ),
                Statement::new(
                    vec![ValueId(3)],
                    Expr::FiniteLiteral {
                        ty: u32_ty,
                        value: 10,
                    },
                ),
                Statement::new(
                    vec![ValueId(4)],
                    Expr::FiniteLiteral {
                        ty: u32_ty,
                        value: 32,
                    },
                ),
                Statement::new(
                    vec![ValueId(5)],
                    Expr::Product {
                        ty: pair_ty,
                        fields: vec![ValueId(3), ValueId(4)],
                    },
                ),
                Statement::new(
                    vec![ValueId(6)],
                    Expr::CallValue {
                        function: ValueId(2),
                        arg: ValueId(5),
                    },
                ),
            ],
            returns: vec![ValueId(6)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(apply, vec![])
            .unwrap(),
        vec![Value::Product(vec![Value::Finite(32), Value::Finite(10)])]
    );
}

#[test]
fn product_split_consumes_product_and_returns_fields() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let pair = types
        .add_product(
            Some("Pair".into()),
            vec![
                Component::named("token", token),
                Component::named("done", types.unit()),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("split".into()),
            inputs: vec![Param::new(ValueId(0), pair)],
            outputs: vec![token, types.unit()],
            body: vec![Statement::new(
                vec![ValueId(1), ValueId(2)],
                Expr::SplitProduct { value: ValueId(0) },
            )],
            returns: vec![ValueId(1), ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn product_projection_returns_field_and_residual() {
    let mut types = TypeStore::new();
    let user_id = types
        .add_primitive("UserId", DeclaredCapabilities::linear())
        .unwrap();
    let balance = types
        .add_primitive("Balance", DeclaredCapabilities::linear())
        .unwrap();
    let locked = types
        .add_primitive("Locked", DeclaredCapabilities::linear())
        .unwrap();
    let user = types
        .add_product(
            Some("User".into()),
            vec![
                Component::named("id", user_id),
                Component::named("balance", balance),
                Component::named("locked", locked),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let user_without_balance = types
        .add_product(
            Some("UserWithoutBalance".into()),
            vec![
                Component::named("id", user_id),
                Component::named("locked", locked),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("take_balance".into()),
            inputs: vec![Param::new(ValueId(0), user)],
            outputs: vec![balance, user_without_balance],
            body: vec![Statement::new(
                vec![ValueId(1), ValueId(2)],
                Expr::ProjectProduct {
                    value: ValueId(0),
                    field: 1,
                    residual_ty: user_without_balance,
                },
            )],
            returns: vec![ValueId(1), ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn product_insert_reassembles_field_and_residual() {
    let mut types = TypeStore::new();
    let user_id = types
        .add_primitive("UserId", DeclaredCapabilities::linear())
        .unwrap();
    let balance = types
        .add_primitive("Balance", DeclaredCapabilities::linear())
        .unwrap();
    let locked = types
        .add_primitive("Locked", DeclaredCapabilities::linear())
        .unwrap();
    let user = types
        .add_product(
            Some("User".into()),
            vec![
                Component::named("id", user_id),
                Component::named("balance", balance),
                Component::named("locked", locked),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let user_without_balance = types
        .add_product(
            Some("UserWithoutBalance".into()),
            vec![
                Component::named("id", user_id),
                Component::named("locked", locked),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("put_balance".into()),
            inputs: vec![
                Param::new(ValueId(0), balance),
                Param::new(ValueId(1), user_without_balance),
            ],
            outputs: vec![user],
            body: vec![Statement::new(
                vec![ValueId(2)],
                Expr::InsertProductField {
                    ty: user,
                    field: 1,
                    field_value: ValueId(0),
                    residual: ValueId(1),
                },
            )],
            returns: vec![ValueId(2)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn product_projection_rejects_wrong_residual_shape() {
    let mut types = TypeStore::new();
    let user_id = types
        .add_primitive("UserId", DeclaredCapabilities::linear())
        .unwrap();
    let balance = types
        .add_primitive("Balance", DeclaredCapabilities::linear())
        .unwrap();
    let locked = types
        .add_primitive("Locked", DeclaredCapabilities::linear())
        .unwrap();
    let user = types
        .add_product(
            Some("User".into()),
            vec![
                Component::named("id", user_id),
                Component::named("balance", balance),
                Component::named("locked", locked),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let wrong_residual = types
        .add_product(
            Some("WrongResidual".into()),
            vec![
                Component::named("locked", locked),
                Component::named("id", user_id),
            ],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_take_balance".into()),
            inputs: vec![Param::new(ValueId(0), user)],
            outputs: vec![balance, wrong_residual],
            body: vec![Statement::new(
                vec![ValueId(1), ValueId(2)],
                Expr::ProjectProduct {
                    value: ValueId(0),
                    field: 1,
                    residual_ty: wrong_residual,
                },
            )],
            returns: vec![ValueId(1), ValueId(2)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::BadProductResidual {
            product: user,
            field: 1,
            residual: wrong_residual,
        })
    );
}

#[test]
fn product_projection_rejects_bad_field_index() {
    let mut types = TypeStore::new();
    let token = token_type(&mut types);
    let product = types
        .add_product(
            Some("Box".into()),
            vec![Component::named("value", token)],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let empty_residual = types
        .add_product(
            Some("EmptyResidual".into()),
            vec![],
            DeclaredCapabilities::linear(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_project".into()),
            inputs: vec![Param::new(ValueId(0), product)],
            outputs: vec![token, empty_residual],
            body: vec![Statement::new(
                vec![ValueId(1), ValueId(2)],
                Expr::ProjectProduct {
                    value: ValueId(0),
                    field: 1,
                    residual_ty: empty_residual,
                },
            )],
            returns: vec![ValueId(1), ValueId(2)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::BadField {
            ty: product,
            field: 1,
        })
    );
}

#[test]
fn sum_inject_consumes_payload_and_returns_sum() {
    let mut types = TypeStore::new();
    let bool_ty = bool_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("true".into()),
            inputs: vec![Param::new(ValueId(0), types.unit())],
            outputs: vec![bool_ty],
            body: vec![Statement::new(
                vec![ValueId(1)],
                Expr::SumInject {
                    ty: bool_ty,
                    variant: 1,
                    payload: ValueId(0),
                },
            )],
            returns: vec![ValueId(1)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn match_eliminates_sums() {
    let mut types = TypeStore::new();
    let bool_ty = bool_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("not".into()),
            inputs: vec![Param::new(ValueId(0), bool_ty)],
            outputs: vec![bool_ty],
            body: vec![Statement::new(
                vec![ValueId(10)],
                Expr::Match {
                    scrutinee: ValueId(0),
                    arms: vec![
                        MatchArm::new(
                            0,
                            ValueId(1),
                            vec![Statement::new(
                                vec![ValueId(2)],
                                Expr::SumInject {
                                    ty: bool_ty,
                                    variant: 1,
                                    payload: ValueId(1),
                                },
                            )],
                            vec![ValueId(2)],
                        ),
                        MatchArm::new(
                            1,
                            ValueId(3),
                            vec![Statement::new(
                                vec![ValueId(4)],
                                Expr::SumInject {
                                    ty: bool_ty,
                                    variant: 0,
                                    payload: ValueId(3),
                                },
                            )],
                            vec![ValueId(4)],
                        ),
                    ],
                },
            )],
            returns: vec![ValueId(10)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn match_requires_all_variants() {
    let mut types = TypeStore::new();
    let bool_ty = bool_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_match".into()),
            inputs: vec![Param::new(ValueId(0), bool_ty)],
            outputs: vec![bool_ty],
            body: vec![Statement::new(
                vec![ValueId(10)],
                Expr::Match {
                    scrutinee: ValueId(0),
                    arms: vec![MatchArm::new(
                        0,
                        ValueId(1),
                        vec![Statement::new(
                            vec![ValueId(2)],
                            Expr::SumInject {
                                ty: bool_ty,
                                variant: 1,
                                payload: ValueId(1),
                            },
                        )],
                        vec![ValueId(2)],
                    )],
                },
            )],
            returns: vec![ValueId(10)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::MissingMatchArm {
            ty: bool_ty,
            variant: 1,
        })
    );
}

#[test]
fn match_rejects_duplicate_variants() {
    let mut types = TypeStore::new();
    let bool_ty = bool_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_match".into()),
            inputs: vec![Param::new(ValueId(0), bool_ty)],
            outputs: vec![bool_ty],
            body: vec![Statement::new(
                vec![ValueId(10)],
                Expr::Match {
                    scrutinee: ValueId(0),
                    arms: vec![
                        MatchArm::new(
                            0,
                            ValueId(1),
                            vec![Statement::new(
                                vec![ValueId(2)],
                                Expr::SumInject {
                                    ty: bool_ty,
                                    variant: 1,
                                    payload: ValueId(1),
                                },
                            )],
                            vec![ValueId(2)],
                        ),
                        MatchArm::new(
                            0,
                            ValueId(3),
                            vec![Statement::new(
                                vec![ValueId(4)],
                                Expr::SumInject {
                                    ty: bool_ty,
                                    variant: 1,
                                    payload: ValueId(3),
                                },
                            )],
                            vec![ValueId(4)],
                        ),
                    ],
                },
            )],
            returns: vec![ValueId(10)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::DuplicateMatchArm {
            ty: bool_ty,
            variant: 0,
        })
    );
}

#[test]
fn match_arms_must_return_same_types() {
    let mut types = TypeStore::new();
    let bool_ty = bool_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_match".into()),
            inputs: vec![Param::new(ValueId(0), bool_ty)],
            outputs: vec![bool_ty],
            body: vec![Statement::new(
                vec![ValueId(10)],
                Expr::Match {
                    scrutinee: ValueId(0),
                    arms: vec![
                        MatchArm::new(
                            0,
                            ValueId(1),
                            vec![Statement::new(
                                vec![ValueId(2)],
                                Expr::SumInject {
                                    ty: bool_ty,
                                    variant: 1,
                                    payload: ValueId(1),
                                },
                            )],
                            vec![ValueId(2)],
                        ),
                        MatchArm::new(1, ValueId(3), vec![], vec![ValueId(3)]),
                    ],
                },
            )],
            returns: vec![ValueId(10)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::TypeMismatch {
            expected: bool_ty,
            actual: types.unit(),
        })
    );
}

#[test]
fn match_arms_thread_outer_locals() {
    let mut types = TypeStore::new();
    let bool_ty = bool_type(&mut types);
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_capture".into()),
            inputs: vec![
                Param::new(ValueId(0), bool_ty),
                Param::new(ValueId(1), token),
            ],
            outputs: vec![token],
            body: vec![Statement::new(
                vec![ValueId(10)],
                Expr::Match {
                    scrutinee: ValueId(0),
                    arms: vec![
                        MatchArm::new(
                            0,
                            ValueId(2),
                            vec![Statement::new(vec![], Expr::Zap { value: ValueId(2) })],
                            vec![ValueId(1)],
                        ),
                        MatchArm::new(
                            1,
                            ValueId(3),
                            vec![Statement::new(vec![], Expr::Zap { value: ValueId(3) })],
                            vec![ValueId(1)],
                        ),
                    ],
                },
            )],
            returns: vec![ValueId(10)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
}

#[test]
fn match_arms_must_account_for_threaded_outer_locals() {
    let mut types = TypeStore::new();
    let bool_ty = bool_type(&mut types);
    let token = token_type(&mut types);
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_thread".into()),
            inputs: vec![
                Param::new(ValueId(0), bool_ty),
                Param::new(ValueId(1), token),
            ],
            outputs: vec![types.unit()],
            body: vec![Statement::new(
                vec![ValueId(10)],
                Expr::Match {
                    scrutinee: ValueId(0),
                    arms: vec![
                        MatchArm::new(0, ValueId(2), vec![], vec![ValueId(2)]),
                        MatchArm::new(1, ValueId(3), vec![], vec![ValueId(3)]),
                    ],
                },
            )],
            returns: vec![ValueId(10)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::LiveValueAtEnd(ValueId(1)))
    );
}

#[test]
fn finite_literals_check_cardinality() {
    let mut types = TypeStore::new();
    let bit = types
        .add_finite(Some("Bit".into()), 2, DeclaredCapabilities::dup_zap())
        .unwrap();
    let mut program = CoreProgram::new();
    program
        .add_function(Function {
            name: Some("bad_bit".into()),
            inputs: vec![],
            outputs: vec![bit],
            body: vec![Statement::new(
                vec![ValueId(0)],
                Expr::FiniteLiteral { ty: bit, value: 2 },
            )],
            returns: vec![ValueId(0)],
        })
        .unwrap();

    assert_eq!(
        program.check(&types),
        Err(CoreError::FiniteLiteralOutOfRange {
            ty: bit,
            value: 2,
            values: 2
        })
    );
}

#[test]
fn evaluator_finite_arithmetic_does_not_overflow_u128() {
    let mut types = TypeStore::new();
    let huge = types
        .add_finite(
            Some("Huge".into()),
            u128::MAX,
            DeclaredCapabilities::dup_zap(),
        )
        .unwrap();
    let mut program = CoreProgram::new();
    let add = program
        .add_function(Function {
            name: Some("huge_add".into()),
            inputs: vec![Param::new(ValueId(0), huge), Param::new(ValueId(1), huge)],
            outputs: vec![huge],
            body: vec![
                Statement::new(
                    vec![ValueId(2), ValueId(3), ValueId(4)],
                    Expr::Builtin {
                        op: BuiltinOp::FiniteAdd { ty: huge },
                        args: vec![ValueId(0), ValueId(1)],
                    },
                ),
                Statement::new(vec![], Expr::Zap { value: ValueId(2) }),
                Statement::new(vec![], Expr::Zap { value: ValueId(3) }),
            ],
            returns: vec![ValueId(4)],
        })
        .unwrap();
    let mul = program
        .add_function(Function {
            name: Some("huge_mul".into()),
            inputs: vec![Param::new(ValueId(0), huge), Param::new(ValueId(1), huge)],
            outputs: vec![huge],
            body: vec![
                Statement::new(
                    vec![ValueId(2), ValueId(3), ValueId(4)],
                    Expr::Builtin {
                        op: BuiltinOp::FiniteMul { ty: huge },
                        args: vec![ValueId(0), ValueId(1)],
                    },
                ),
                Statement::new(vec![], Expr::Zap { value: ValueId(2) }),
                Statement::new(vec![], Expr::Zap { value: ValueId(3) }),
            ],
            returns: vec![ValueId(4)],
        })
        .unwrap();

    assert_eq!(program.check(&types), Ok(()));
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(
                add,
                vec![Value::Finite(u128::MAX - 1), Value::Finite(u128::MAX - 1)]
            )
            .unwrap(),
        vec![Value::Finite(u128::MAX - 2)]
    );
    assert_eq!(
        Evaluator::new(&types, &program)
            .run_function(
                mul,
                vec![Value::Finite(u128::MAX - 1), Value::Finite(u128::MAX - 1)]
            )
            .unwrap(),
        vec![Value::Finite(1)]
    );
}
