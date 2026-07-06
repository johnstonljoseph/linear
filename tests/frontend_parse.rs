use linear::frontend::{
    Arg, BinaryOp, Block, Expr, Field, Item, LetStmt, MatchArm, Param, Pattern, TypeExpr,
    ValueFlow, parse_module,
};

#[test]
fn parses_type_global_and_function_items() {
    let module = parse_module(
        r#"
        struct User { id: u32, balance: u32 }
        global root: User
        fn add_one(x: u32) -> u32 {
            let one: u32 = 1
            add(x, one)
        }
        "#,
    )
    .unwrap();

    assert_eq!(module.items.len(), 3);
    assert!(matches!(module.items[0], Item::Struct(_)));
    assert!(matches!(module.items[1], Item::Global(_)));
    let Item::Function(function) = &module.items[2] else {
        panic!("expected function item");
    };
    assert_eq!(function.name, "add_one");
    assert_eq!(function.params.len(), 1);
    assert_eq!(function.output, TypeExpr::Name("u32".into()));
    assert_eq!(
        function.body,
        Block {
            lets: vec![LetStmt {
                pattern: Pattern::Name("one".into()),
                ty: Some(TypeExpr::Name("u32".into())),
                value: Expr::Int(1),
            }],
            result: Some(Box::new(Expr::Call {
                callee: Box::new(Expr::Name("add".into())),
                args: vec![
                    Arg {
                        flow: ValueFlow::ReturnedUnchanged,
                        label: None,
                        value: Expr::Name("x".into()),
                    },
                    Arg {
                        flow: ValueFlow::ReturnedUnchanged,
                        label: None,
                        value: Expr::Name("one".into()),
                    },
                ],
            })),
        }
    );
}

#[test]
fn parses_algebraic_type_products_and_sums() {
    let module = parse_module(
        r#"
        enum UserEvent {
          created { id: UserId, initial_balance: Balance },
          updated { id: UserId, delta: Balance },
        }
        "#,
    )
    .unwrap();

    let Item::Enum(type_def) = &module.items[0] else {
        panic!("expected enum item");
    };
    assert_eq!(
        type_def.ty,
        TypeExpr::Sum(vec![
            Field {
                name: Some("created".into()),
                value: TypeExpr::Product(vec![
                    Field {
                        name: Some("id".into()),
                        value: TypeExpr::Name("UserId".into()),
                    },
                    Field {
                        name: Some("initial_balance".into()),
                        value: TypeExpr::Name("Balance".into()),
                    },
                ]),
            },
            Field {
                name: Some("updated".into()),
                value: TypeExpr::Product(vec![
                    Field {
                        name: Some("id".into()),
                        value: TypeExpr::Name("UserId".into()),
                    },
                    Field {
                        name: Some("delta".into()),
                        value: TypeExpr::Name("Balance".into()),
                    },
                ]),
            },
        ])
    );
}

#[test]
fn parses_braced_struct_and_enum_groups() {
    let module = parse_module(
        r#"
        struct User { id: UserId, balance: Balance }
        enum Flow { unchanged, changed, sunk }
        "#,
    )
    .unwrap();

    let Item::Struct(struct_def) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(
        struct_def.ty,
        TypeExpr::Product(vec![
            Field {
                name: Some("id".into()),
                value: TypeExpr::Name("UserId".into()),
            },
            Field {
                name: Some("balance".into()),
                value: TypeExpr::Name("Balance".into()),
            },
        ])
    );

    let Item::Enum(enum_def) = &module.items[1] else {
        panic!("expected enum item");
    };
    assert_eq!(enum_def.name, "Flow");
}

#[test]
fn parses_optional_arg_labels_and_method_calls() {
    let module = parse_module(
        r#"
        fn run(users: HashMap<u32, User>, id: u32) -> HashMap<u32, User> {
            users.insert(key: id, value: make_user(id))
        }
        "#,
    )
    .unwrap();

    let Item::Function(function) = &module.items[0] else {
        panic!("expected function item");
    };
    assert_eq!(
        function.params[0].ty,
        TypeExpr::Apply {
            name: "HashMap".into(),
            args: vec![TypeExpr::Name("u32".into()), TypeExpr::Name("User".into())],
        }
    );
    assert_eq!(
        function.body.result.as_deref(),
        Some(&Expr::MethodCall {
            receiver: Box::new(Expr::Name("users".into())),
            receiver_flow: ValueFlow::ReturnedUnchanged,
            method: "insert".into(),
            args: vec![
                Arg {
                    flow: ValueFlow::ReturnedUnchanged,
                    label: Some("key".into()),
                    value: Expr::Name("id".into()),
                },
                Arg {
                    flow: ValueFlow::ReturnedUnchanged,
                    label: Some("value".into()),
                    value: Expr::Call {
                        callee: Box::new(Expr::Name("make_user".into())),
                        args: vec![Arg {
                            flow: ValueFlow::ReturnedUnchanged,
                            label: None,
                            value: Expr::Name("id".into()),
                        }],
                    },
                },
            ],
        })
    );
}

#[test]
fn parses_value_flow_markers_on_method_receivers() {
    let module = parse_module(
        r#"
        fn run(cache: Cache, event: Event) -> Cache {
            cache.insert!(key: "latest", @value: event)
        }
        "#,
    )
    .unwrap();

    let Item::Function(function) = &module.items[0] else {
        panic!("expected function item");
    };
    assert_eq!(
        function.body.result.as_deref(),
        Some(&Expr::MethodCall {
            receiver: Box::new(Expr::Name("cache".into())),
            receiver_flow: ValueFlow::ReturnedChanged,
            method: "insert".into(),
            args: vec![
                Arg {
                    flow: ValueFlow::ReturnedUnchanged,
                    label: Some("key".into()),
                    value: Expr::String("latest".into()),
                },
                Arg {
                    flow: ValueFlow::NotReturned,
                    label: Some("value".into()),
                    value: Expr::Name("event".into()),
                },
            ],
        })
    );
}

#[test]
fn parses_value_flow_markers_on_params_and_args() {
    let module = parse_module(
        r#"
        fn update(!state: State, config: Config, @event: Event) -> State {
            apply(!state, config, @event: event)
        }
        "#,
    )
    .unwrap();

    let Item::Function(function) = &module.items[0] else {
        panic!("expected function item");
    };
    assert_eq!(function.params[0].flow, ValueFlow::ReturnedChanged);
    assert_eq!(function.params[1].flow, ValueFlow::ReturnedUnchanged);
    assert_eq!(function.params[2].flow, ValueFlow::NotReturned);

    assert_eq!(
        function.body.result.as_deref(),
        Some(&Expr::Call {
            callee: Box::new(Expr::Name("apply".into())),
            args: vec![
                Arg {
                    flow: ValueFlow::ReturnedChanged,
                    label: None,
                    value: Expr::Name("state".into()),
                },
                Arg {
                    flow: ValueFlow::ReturnedUnchanged,
                    label: None,
                    value: Expr::Name("config".into()),
                },
                Arg {
                    flow: ValueFlow::NotReturned,
                    label: Some("event".into()),
                    value: Expr::Name("event".into()),
                },
            ],
        })
    );
}

#[test]
fn parses_impl_methods_with_self_as_first_arg() {
    let module = parse_module(
        r#"
        struct User { id: u32, balance: u32 }

        impl User {
            fn balance(self) -> u32 {
                self.balance
            }

            fn with_balance(!self: User, balance: u32) -> User {
                User { id: self.id, balance: balance }
            }
        }
        "#,
    )
    .unwrap();

    let Item::Impl(impl_block) = &module.items[1] else {
        panic!("expected impl item");
    };
    assert_eq!(impl_block.target, TypeExpr::Name("User".into()));
    assert_eq!(impl_block.methods.len(), 2);

    let balance = &impl_block.methods[0];
    assert_eq!(balance.name, "balance");
    assert_eq!(
        balance.params,
        vec![linear::frontend::Param {
            flow: ValueFlow::ReturnedUnchanged,
            name: "self".into(),
            ty: TypeExpr::Name("User".into()),
        }]
    );

    let with_balance = &impl_block.methods[1];
    assert_eq!(with_balance.name, "with_balance");
    assert_eq!(with_balance.params[0].flow, ValueFlow::ReturnedChanged);
    assert_eq!(with_balance.params[0].name, "self");
    assert_eq!(with_balance.params[0].ty, TypeExpr::Name("User".into()));
    assert_eq!(
        with_balance.body.result.as_deref(),
        Some(&Expr::Call {
            callee: Box::new(Expr::Name("User".into())),
            args: vec![Arg {
                flow: ValueFlow::ReturnedUnchanged,
                label: None,
                value: Expr::Product(vec![
                    Field {
                        name: Some("id".into()),
                        value: Expr::FieldAccess {
                            receiver: Box::new(Expr::Name("self".into())),
                            field: "id".into(),
                        },
                    },
                    Field {
                        name: Some("balance".into()),
                        value: Expr::Name("balance".into()),
                    },
                ]),
            }],
        })
    );
}

#[test]
fn parses_sum_types_and_match_expressions() {
    let module = parse_module(
        r#"
        enum MaybeU32 { none, some(u32) }
        fn or_zero(x: MaybeU32) -> u32 {
            match x {
                .none: 0,
                .some(value): value,
            }
        }
        "#,
    )
    .unwrap();

    let Item::Enum(type_def) = &module.items[0] else {
        panic!("expected enum item");
    };
    assert_eq!(
        type_def.ty,
        TypeExpr::Sum(vec![
            Field {
                name: Some("none".into()),
                value: TypeExpr::Unit,
            },
            Field {
                name: Some("some".into()),
                value: TypeExpr::Name("u32".into()),
            },
        ])
    );

    let Item::Function(function) = &module.items[1] else {
        panic!("expected function item");
    };
    assert_eq!(
        function.body.result.as_deref(),
        Some(&Expr::Match {
            scrutinee: Box::new(Expr::Name("x".into())),
            arms: vec![
                MatchArm {
                    variant: "none".into(),
                    payload: None,
                    body: Expr::Int(0),
                },
                MatchArm {
                    variant: "some".into(),
                    payload: Some(Pattern::Name("value".into())),
                    body: Expr::Name("value".into()),
                },
            ],
        })
    );
}

#[test]
fn parses_patterns_in_lets_and_match_payloads() {
    let module = parse_module(
        r#"
        enum Decision {
          allow { reason: u32 },
          review { queue: u32, priority: u32 },
        }

        fn unpack(pair: { left: u32, right: u32 }, decision: Decision) -> u32 {
          let { left, right: renamed } = pair
          let (a, b) = (left, renamed)
          match decision {
            .allow { reason }: a,
            .review { queue, priority: p }: b + p,
          }
        }
        "#,
    )
    .unwrap();

    let Item::Function(function) = &module.items[1] else {
        panic!("expected function item");
    };
    assert_eq!(
        function.body.lets[0].pattern,
        Pattern::Record(vec![
            Field {
                name: Some("left".into()),
                value: Pattern::Name("left".into()),
            },
            Field {
                name: Some("right".into()),
                value: Pattern::Name("renamed".into()),
            },
        ])
    );
    assert_eq!(
        function.body.lets[1].pattern,
        Pattern::Tuple(vec![Pattern::Name("a".into()), Pattern::Name("b".into())])
    );

    let Some(Expr::Match { arms, .. }) = function.body.result.as_deref() else {
        panic!("expected match result");
    };
    assert_eq!(
        arms[1].payload,
        Some(Pattern::Record(vec![
            Field {
                name: Some("queue".into()),
                value: Pattern::Name("queue".into()),
            },
            Field {
                name: Some("priority".into()),
                value: Pattern::Name("p".into()),
            },
        ]))
    );
}

#[test]
fn parses_generics_traits_and_trait_impls() {
    let module = parse_module(
        r#"
        struct Pair<T, U> { first: T, second: U }
        enum Option<T> { none, some(T) }

        trait Eq<T> {
          fn eq(self: T, other: T) -> Bool
        }

        impl<T> Eq<T> for Option<T> {
          fn eq(self, other: Option<T>) -> Bool {
            true
          }
        }
        "#,
    )
    .unwrap();

    let Item::Struct(pair) = &module.items[0] else {
        panic!("expected struct item");
    };
    assert_eq!(pair.generics, vec!["T", "U"]);

    let Item::Enum(option) = &module.items[1] else {
        panic!("expected enum item");
    };
    assert_eq!(option.generics, vec!["T"]);

    let Item::Trait(eq_trait) = &module.items[2] else {
        panic!("expected trait item");
    };
    assert_eq!(eq_trait.name, "Eq");
    assert_eq!(eq_trait.generics, vec!["T"]);
    assert_eq!(eq_trait.methods.len(), 1);
    assert_eq!(
        eq_trait.methods[0].params,
        vec![
            Param {
                flow: ValueFlow::ReturnedUnchanged,
                name: "self".into(),
                ty: TypeExpr::Name("T".into()),
            },
            Param {
                flow: ValueFlow::ReturnedUnchanged,
                name: "other".into(),
                ty: TypeExpr::Name("T".into()),
            },
        ]
    );

    let Item::Impl(impl_block) = &module.items[3] else {
        panic!("expected impl item");
    };
    assert_eq!(impl_block.generics, vec!["T"]);
    assert_eq!(
        impl_block.trait_ref,
        Some(TypeExpr::Apply {
            name: "Eq".into(),
            args: vec![TypeExpr::Name("T".into())],
        })
    );
    assert_eq!(
        impl_block.target,
        TypeExpr::Apply {
            name: "Option".into(),
            args: vec![TypeExpr::Name("T".into())],
        }
    );
    assert_eq!(impl_block.methods[0].generics, Vec::<String>::new());
}

#[test]
fn parses_if_else_comments_and_binary_ops() {
    let module = parse_module(
        r#"
        // comments are ignored like whitespace
        fn clamp(x: u32, max: u32) -> u32 {
            if x > max {
                max
            } else {
                x + 1
            }
        }
        "#,
    )
    .unwrap();

    let Item::Function(function) = &module.items[0] else {
        panic!("expected function item");
    };
    assert_eq!(
        function.body.result.as_deref(),
        Some(&Expr::If {
            condition: Box::new(Expr::Binary {
                lhs: Box::new(Expr::Name("x".into())),
                op: BinaryOp::Gt,
                rhs: Box::new(Expr::Name("max".into())),
            }),
            then_branch: Block {
                lets: Vec::new(),
                result: Some(Box::new(Expr::Name("max".into()))),
            },
            else_branch: Block {
                lets: Vec::new(),
                result: Some(Box::new(Expr::Binary {
                    lhs: Box::new(Expr::Name("x".into())),
                    op: BinaryOp::Add,
                    rhs: Box::new(Expr::Int(1)),
                })),
            },
        })
    );
}

#[test]
fn parses_else_if_as_nested_if() {
    let module = parse_module(
        r#"
        fn classify(x: u32) -> u32 {
            if x < 10 {
                0
            } else if x < 20 {
                1
            } else {
                2
            }
        }
        "#,
    )
    .unwrap();

    let Item::Function(function) = &module.items[0] else {
        panic!("expected function item");
    };
    let Some(Expr::If { else_branch, .. }) = function.body.result.as_deref() else {
        panic!("expected outer if");
    };
    assert!(else_branch.lets.is_empty());
    assert!(matches!(
        else_branch.result.as_deref(),
        Some(Expr::If { .. })
    ));
}

#[test]
fn parses_braced_function_match_and_if_bodies() {
    let module = parse_module(
        r#"
        enum MaybeU32 {
          none,
          some(u32),
        }

        fn or_zero(x: MaybeU32) -> u32 {
          match x {
            .none: 0,
            .some(value): value,
          }
        }

        fn clamp(x: u32, max: u32) -> u32 {
          if x > max {
            max
          } else {
            x + 1
          }
        }
        "#,
    )
    .unwrap();

    let Item::Function(or_zero) = &module.items[1] else {
        panic!("expected function item");
    };
    assert!(matches!(
        or_zero.body.result.as_deref(),
        Some(Expr::Match { .. })
    ));

    let Item::Function(clamp) = &module.items[2] else {
        panic!("expected function item");
    };
    assert_eq!(
        clamp.body.result.as_deref(),
        Some(&Expr::If {
            condition: Box::new(Expr::Binary {
                lhs: Box::new(Expr::Name("x".into())),
                op: BinaryOp::Gt,
                rhs: Box::new(Expr::Name("max".into())),
            }),
            then_branch: Block {
                lets: Vec::new(),
                result: Some(Box::new(Expr::Name("max".into()))),
            },
            else_branch: Block {
                lets: Vec::new(),
                result: Some(Box::new(Expr::Binary {
                    lhs: Box::new(Expr::Name("x".into())),
                    op: BinaryOp::Add,
                    rhs: Box::new(Expr::Int(1)),
                })),
            },
        })
    );
}

#[test]
fn parses_all_bootstrap_sketches() {
    let bootstrap_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bootstrap");
    let mut parsed = 0;
    for entry in std::fs::read_dir(bootstrap_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|extension| extension == "lr") {
            let src = std::fs::read_to_string(&path).unwrap();
            parse_module(&src).unwrap_or_else(|errors| {
                panic!("failed to parse {}:\n{}", path.display(), errors.join("\n"))
            });
            parsed += 1;
        }
    }

    assert!(parsed >= 2);
}
