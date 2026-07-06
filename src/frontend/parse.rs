use chumsky::prelude::*;

use super::ast::{
    Arg, BinaryOp, Block, Expr, Field, FunctionDef, FunctionSig, GlobalDef, ImplBlock, Item,
    LetStmt, MatchArm, Module, Param, Pattern, TraitDef, TypeDef, TypeExpr, ValueFlow,
};

pub type ParseErrors = Vec<String>;

#[derive(Clone, Debug)]
struct RawParam {
    flow: ValueFlow,
    name: String,
    ty: Option<TypeExpr>,
}

#[derive(Clone, Debug)]
struct RawMethodDef {
    name: String,
    generics: Vec<String>,
    params: Vec<RawParam>,
    output: TypeExpr,
    body: Expr,
}

pub fn parse_module(src: &str) -> Result<Module, ParseErrors> {
    module_parser()
        .parse(src)
        .into_result()
        .map_err(|errors| errors.into_iter().map(|error| error.to_string()).collect())
}

fn module_parser<'src>()
-> impl Parser<'src, &'src str, Module, extra::Err<Rich<'src, char>>> + Clone {
    padding()
        .ignore_then(item_parser().repeated().collect::<Vec<_>>())
        .then_ignore(padding())
        .then_ignore(end())
        .map(|items| Module { items })
        .boxed()
}

fn item_parser<'src>() -> impl Parser<'src, &'src str, Item, extra::Err<Rich<'src, char>>> + Clone {
    let ident = ident_parser();
    let ty = type_parser();
    let expr = expr_parser();
    let generics = generic_params_parser();
    let capability_clause = capability_clause_parser();

    let type_def = keyword("type")
        .ignore_then(ident.clone())
        .then(generics.clone())
        .then_ignore(sym('='))
        .then(ty.clone())
        .then(capability_clause.clone())
        .map(|(((name, generics), ty), capabilities)| {
            Item::Type(TypeDef {
                name,
                generics,
                ty,
                capabilities,
            })
        });

    let named_type_field =
        ident
            .clone()
            .then_ignore(sym(':'))
            .then(ty.clone())
            .map(|(name, value)| Field {
                name: Some(name),
                value,
            });

    let braced_struct_body = named_type_field
        .clone()
        .separated_by(sym(','))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(sym('{'), sym('}'))
        .map(record_type);

    let tuple_struct_body = ty
        .clone()
        .separated_by(sym(','))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(sym('('), sym(')'))
        .map(tuple_struct_type);

    let struct_def = keyword("struct")
        .ignore_then(ident.clone())
        .then(generics.clone())
        .then(choice((braced_struct_body, tuple_struct_body)))
        .then(capability_clause.clone())
        .map(|(((name, generics), ty), capabilities)| {
            Item::Struct(TypeDef {
                name,
                generics,
                ty,
                capabilities,
            })
        });

    let record_variant_payload = named_type_field
        .clone()
        .separated_by(sym(','))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(sym('{'), sym('}'))
        .map(record_type);

    let tuple_variant_payload = ty
        .clone()
        .separated_by(sym(','))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(sym('('), sym(')'))
        .map(tuple_payload_type);

    let enum_variant = ident
        .clone()
        .then(choice((record_variant_payload, tuple_variant_payload)).or_not())
        .map(|(name, payload)| Field {
            name: Some(name),
            value: payload.unwrap_or(TypeExpr::Unit),
        });

    let braced_enum_body = enum_variant
        .separated_by(sym(','))
        .allow_trailing()
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(sym('{'), sym('}'))
        .map(TypeExpr::Sum);

    let enum_def = keyword("enum")
        .ignore_then(ident.clone())
        .then(generics.clone())
        .then(braced_enum_body)
        .then(capability_clause.clone())
        .map(|(((name, generics), ty), capabilities)| {
            Item::Enum(TypeDef {
                name,
                generics,
                ty,
                capabilities,
            })
        });

    let global_def = keyword("global")
        .ignore_then(ident.clone())
        .then_ignore(sym(':'))
        .then(ty.clone())
        .then(sym('=').ignore_then(expr.clone()).or_not())
        .map(|((name, ty), value)| Item::Global(GlobalDef { name, ty, value }));

    let param = flow_marker()
        .then(ident.clone().then_ignore(sym(':')).then(ty.clone()))
        .map(|(flow, (name, ty))| Param { flow, name, ty });

    let params = param
        .separated_by(sym(','))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(sym('('), sym(')'));

    let function_def = keyword("fn")
        .ignore_then(ident.clone())
        .then(generics.clone())
        .then(params.clone())
        .then_ignore(op("->"))
        .then(ty.clone())
        .then(expr.clone())
        .map(|((((name, generics), params), output), body)| {
            Item::Function(FunctionDef {
                name,
                generics,
                params,
                output,
                body: expr_to_block(body),
            })
        });

    let raw_param = flow_marker()
        .then(
            ident
                .clone()
                .then(sym(':').ignore_then(ty.clone()).or_not()),
        )
        .map(|(flow, (name, ty))| RawParam { flow, name, ty });

    let raw_params = raw_param
        .separated_by(sym(','))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(sym('('), sym(')'));

    let method_def = keyword("fn")
        .ignore_then(ident.clone())
        .then(generics.clone())
        .then(raw_params)
        .then_ignore(op("->"))
        .then(ty.clone())
        .then(expr)
        .map(
            |((((name, generics), params), output), body)| RawMethodDef {
                name,
                generics,
                params,
                output,
                body,
            },
        );

    let sig_def = keyword("fn")
        .ignore_then(ident.clone())
        .then(generics.clone())
        .then(params.clone())
        .then_ignore(op("->"))
        .then(ty.clone())
        .map(|(((name, generics), params), output)| FunctionSig {
            name,
            generics,
            params,
            output,
        });

    let trait_def = keyword("trait")
        .ignore_then(ident.clone())
        .then(generics.clone())
        .then(
            sig_def
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(sym('{'), sym('}')),
        )
        .map(|((name, generics), methods)| {
            Item::Trait(TraitDef {
                name,
                generics,
                methods,
            })
        });

    let impl_header = generics
        .clone()
        .then(ty.clone())
        .then(keyword("for").ignore_then(ty.clone()).or_not())
        .map(|((generics, first_ty), maybe_target)| match maybe_target {
            Some(target) => (generics, Some(first_ty), target),
            None => (generics, None, first_ty),
        });

    let impl_def = keyword("impl")
        .ignore_then(impl_header)
        .then(
            method_def
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(sym('{'), sym('}')),
        )
        .try_map(|((generics, trait_ref, target), methods), span| {
            let methods = methods
                .into_iter()
                .map(|method| lower_method(target.clone(), method))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|message| Rich::custom(span, message))?;
            Ok(Item::Impl(ImplBlock {
                generics,
                trait_ref,
                target,
                methods,
            }))
        });

    choice((
        type_def,
        struct_def,
        enum_def,
        global_def,
        function_def,
        impl_def,
        trait_def,
    ))
    .padded()
    .boxed()
}

fn expr_to_block(expr: Expr) -> Block {
    match expr {
        Expr::Block(block) => block,
        expr => Block {
            lets: Vec::new(),
            result: Some(Box::new(expr)),
        },
    }
}

fn lower_method(target: TypeExpr, method: RawMethodDef) -> Result<FunctionDef, String> {
    let Some(first_param) = method.params.first() else {
        return Err(format!(
            "method `{}` must take `self` as its first parameter",
            method.name
        ));
    };
    if first_param.name != "self" {
        return Err(format!(
            "method `{}` must take `self` as its first parameter",
            method.name
        ));
    }

    let params = method
        .params
        .into_iter()
        .enumerate()
        .map(|(index, param)| {
            let ty = match param.ty {
                Some(ty) => ty,
                None if index == 0 && param.name == "self" => target.clone(),
                None => {
                    return Err(format!(
                        "parameter `{}` in method `{}` needs an explicit type",
                        param.name, method.name
                    ));
                }
            };
            Ok(Param {
                flow: param.flow,
                name: param.name,
                ty,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(FunctionDef {
        name: method.name,
        generics: method.generics,
        params,
        output: method.output,
        body: expr_to_block(method.body),
    })
}

fn type_parser<'src>()
-> impl Parser<'src, &'src str, TypeExpr, extra::Err<Rich<'src, char>>> + Clone {
    recursive(|ty| {
        let ident = ident_parser();

        let generic_args = ty
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('<'), sym('>'));

        let named = ident
            .clone()
            .then(generic_args.or_not())
            .map(|(name, args)| match args {
                Some(args) => TypeExpr::Apply { name, args },
                None => TypeExpr::Name(name),
            });

        let named_product_field =
            ident
                .clone()
                .then_ignore(sym(':'))
                .then(ty.clone())
                .map(|(name, value)| Field {
                    name: Some(name),
                    value,
                });

        let record_type_expr = named_product_field
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('{'), sym('}'))
            .map(record_type);

        let tuple_or_group = ty
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .map(tuple_or_group_type);

        let record_variant_payload = named_product_field
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('{'), sym('}'))
            .map(record_type);

        let tuple_variant_payload = ty
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('('), sym(')'))
            .map(tuple_payload_type);

        let anonymous_enum_variant = ident
            .clone()
            .then(choice((record_variant_payload, tuple_variant_payload)))
            .map(|(name, payload)| Field {
                name: Some(name),
                value: payload,
            });

        let anonymous_enum = anonymous_enum_variant
            .separated_by(sym(','))
            .allow_trailing()
            .at_least(1)
            .collect::<Vec<_>>()
            .map(TypeExpr::Sum);

        let parenthesized =
            choice((anonymous_enum, tuple_or_group)).delimited_by(sym('('), sym(')'));

        let atom = choice((record_type_expr, parenthesized, named));

        atom.then(op("->").ignore_then(ty).or_not())
            .map(|(input, output)| match output {
                Some(output) => TypeExpr::Function {
                    input: Box::new(input),
                    output: Box::new(output),
                },
                None => input,
            })
    })
    .padded()
    .boxed()
}

fn record_type(fields: Vec<Field<TypeExpr>>) -> TypeExpr {
    if fields.is_empty() {
        TypeExpr::Unit
    } else {
        TypeExpr::Product(fields)
    }
}

fn tuple_struct_type(types: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr::Product(
        types
            .into_iter()
            .map(|value| Field { name: None, value })
            .collect(),
    )
}

fn tuple_payload_type(types: Vec<TypeExpr>) -> TypeExpr {
    match types.len() {
        0 => TypeExpr::Unit,
        1 => types.into_iter().next().expect("one tuple payload type"),
        _ => tuple_struct_type(types),
    }
}

fn tuple_or_group_type(types: Vec<TypeExpr>) -> TypeExpr {
    match types.len() {
        0 => TypeExpr::Unit,
        1 => types.into_iter().next().expect("one grouped type"),
        _ => tuple_struct_type(types),
    }
}

fn pattern_parser<'src>()
-> impl Parser<'src, &'src str, Pattern, extra::Err<Rich<'src, char>>> + Clone {
    recursive(|pattern| {
        let ident = ident_parser();

        let wildcard = sym('_').to(Pattern::Wildcard);

        let record_field = ident
            .clone()
            .then(sym(':').ignore_then(pattern.clone()).or_not())
            .map(|(name, pattern)| Field {
                name: Some(name.clone()),
                value: pattern.unwrap_or(Pattern::Name(name)),
            });

        let record = record_field
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('{'), sym('}'))
            .map(Pattern::Record);

        let tuple = pattern
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('('), sym(')'))
            .map(tuple_or_group_pattern);

        choice((wildcard, record, tuple, ident.map(Pattern::Name)))
    })
    .padded_by(padding())
    .boxed()
}

fn tuple_or_group_pattern(patterns: Vec<Pattern>) -> Pattern {
    match patterns.len() {
        0 => Pattern::Unit,
        1 => patterns.into_iter().next().expect("one grouped pattern"),
        _ => Pattern::Tuple(patterns),
    }
}

fn expr_parser<'src>() -> impl Parser<'src, &'src str, Expr, extra::Err<Rich<'src, char>>> + Clone {
    recursive(|expr| {
        let ident = ident_parser();
        let ty = type_parser();
        let pattern = pattern_parser();

        let int = text::int(10)
            .map(|digits: &str| Expr::Int(digits.parse().expect("valid integer literal")))
            .padded();

        let string = none_of('"')
            .repeated()
            .collect::<String>()
            .delimited_by(just('"'), just('"'))
            .map(Expr::String)
            .padded_by(padding());

        let arg = flow_marker()
            .then(
                ident
                    .clone()
                    .then_ignore(sym(':'))
                    .then(expr.clone())
                    .map(|(label, value)| (Some(label), value))
                    .or(expr.clone().map(|value| (None, value))),
            )
            .map(|(flow, (label, value))| Arg { flow, label, value });

        let args = arg
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('('), sym(')'))
            .boxed();

        let let_stmt = keyword("let")
            .ignore_then(pattern.clone())
            .then(sym(':').ignore_then(ty).or_not())
            .then_ignore(sym('='))
            .then(expr.clone())
            .map(|((pattern, ty), value)| LetStmt { pattern, ty, value });

        let block_raw = let_stmt
            .repeated()
            .collect::<Vec<_>>()
            .then(expr.clone().or_not())
            .delimited_by(sym('{'), sym('}'))
            .map(|(lets, result)| Block {
                lets,
                result: result.map(Box::new),
            })
            .boxed();
        let block = block_raw.clone().map(Expr::Block);

        let record_field =
            ident
                .clone()
                .then_ignore(sym(':'))
                .then(expr.clone())
                .map(|(name, value)| Field {
                    name: Some(name),
                    value,
                });

        let record_expr = record_field
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('{'), sym('}'))
            .map(Expr::Product)
            .boxed();

        let paren_expr = expr
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('('), sym(')'))
            .map(tuple_or_group_expr)
            .boxed();

        let match_variant = sym('.').ignore_then(ident.clone()).or(ident.clone());
        let tuple_match_payloads = pattern
            .clone()
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('('), sym(')'))
            .map(tuple_or_group_pattern);
        let record_match_field = ident
            .clone()
            .then(sym(':').ignore_then(pattern.clone()).or_not())
            .map(|(name, pattern)| Field {
                name: Some(name.clone()),
                value: pattern.unwrap_or(Pattern::Name(name)),
            });
        let record_match_payloads = record_match_field
            .separated_by(sym(','))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(sym('{'), sym('}'))
            .map(Pattern::Record);
        let match_payloads = choice((tuple_match_payloads, record_match_payloads))
            .or_not()
            .map(|payload| payload);
        let match_separator = op("=>").or(sym(':'));
        let match_arm = match_variant
            .clone()
            .then(match_payloads)
            .then_ignore(match_separator)
            .then(expr.clone())
            .map(|((variant, payloads), body)| MatchArm {
                variant,
                payload: payloads,
                body,
            });

        let match_expr = keyword("match")
            .ignore_then(expr.clone())
            .then(
                match_arm
                    .separated_by(sym(','))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .delimited_by(sym('{'), sym('}')),
            )
            .map(|(scrutinee, arms)| Expr::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            });

        let if_expr = recursive(|if_expr| {
            let else_branch = block_raw.clone().or(if_expr.map(|else_if| Block {
                lets: Vec::new(),
                result: Some(Box::new(else_if)),
            }));

            keyword("if")
                .ignore_then(expr.clone())
                .then(block_raw.clone())
                .then_ignore(keyword("else"))
                .then(else_branch)
                .map(|((condition, then_branch), else_branch)| Expr::If {
                    condition: Box::new(condition),
                    then_branch,
                    else_branch,
                })
        })
        .boxed();

        let atom = choice((
            int,
            string,
            if_expr,
            match_expr,
            record_expr.clone(),
            block,
            ident.map(Expr::Name),
            paren_expr,
        ))
        .padded()
        .boxed();

        #[derive(Clone, Debug)]
        enum Postfix {
            Call(Vec<Arg>),
            Record(Vec<Field<Expr>>),
            Field(String),
            Method {
                name: String,
                receiver_flow: ValueFlow,
                args: Vec<Arg>,
            },
        }

        let call = args.clone().map(Postfix::Call).boxed();
        let record_construct = record_expr
            .map(|expr| match expr {
                Expr::Product(fields) => fields,
                _ => unreachable!("record expression parser produces products"),
            })
            .map(Postfix::Record)
            .boxed();
        let method = sym('.')
            .ignore_then(ident_parser())
            .then(flow_marker().then(args).or_not())
            .map(|(name, call)| match call {
                Some((receiver_flow, args)) => Postfix::Method {
                    name,
                    receiver_flow,
                    args,
                },
                None => Postfix::Field(name),
            })
            .boxed();

        let call_chain = atom
            .foldl(
                choice((method, call, record_construct)).repeated(),
                |callee, postfix| match postfix {
                    Postfix::Call(args) => Expr::Call {
                        callee: Box::new(callee),
                        args,
                    },
                    Postfix::Record(fields) => Expr::Call {
                        callee: Box::new(callee),
                        args: vec![Arg {
                            flow: ValueFlow::ReturnedUnchanged,
                            label: None,
                            value: Expr::Product(fields),
                        }],
                    },
                    Postfix::Field(field) => Expr::FieldAccess {
                        receiver: Box::new(callee),
                        field,
                    },
                    Postfix::Method {
                        name,
                        receiver_flow,
                        args,
                    } => Expr::MethodCall {
                        receiver: Box::new(callee),
                        receiver_flow,
                        method: name,
                        args,
                    },
                },
            )
            .boxed();

        let product = call_chain
            .clone()
            .foldl(
                choice((op("*").to(BinaryOp::Mul), op("/").to(BinaryOp::Div)))
                    .then(call_chain.clone())
                    .repeated(),
                |lhs, (op, rhs)| Expr::Binary {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            )
            .boxed();

        let sum = product
            .clone()
            .foldl(
                choice((op("+").to(BinaryOp::Add), op("-").to(BinaryOp::Sub)))
                    .then(product.clone())
                    .repeated(),
                |lhs, (op, rhs)| Expr::Binary {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            )
            .boxed();

        sum.clone().foldl(
            choice((
                op("==").to(BinaryOp::Eq),
                op("!=").to(BinaryOp::NotEq),
                op("<=").to(BinaryOp::Lte),
                op(">=").to(BinaryOp::Gte),
                op("<").to(BinaryOp::Lt),
                op(">").to(BinaryOp::Gt),
            ))
            .then(sum)
            .repeated(),
            |lhs, (op, rhs)| Expr::Binary {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
            },
        )
    })
    .padded_by(padding())
    .boxed()
}

fn tuple_or_group_expr(values: Vec<Expr>) -> Expr {
    match values.len() {
        0 => Expr::Unit,
        1 => values.into_iter().next().expect("one grouped expression"),
        _ => Expr::Product(
            values
                .into_iter()
                .map(|value| Field { name: None, value })
                .collect(),
        ),
    }
}

fn ident_parser<'src>() -> impl Parser<'src, &'src str, String, extra::Err<Rich<'src, char>>> + Clone
{
    text::ascii::ident()
        .map(str::to_string)
        .padded_by(padding())
}

fn generic_params_parser<'src>()
-> impl Parser<'src, &'src str, Vec<String>, extra::Err<Rich<'src, char>>> + Clone {
    ident_parser()
        .separated_by(sym(','))
        .allow_trailing()
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(sym('<'), sym('>'))
        .or_not()
        .map(Option::unwrap_or_default)
        .boxed()
}

fn capability_clause_parser<'src>()
-> impl Parser<'src, &'src str, Vec<String>, extra::Err<Rich<'src, char>>> + Clone {
    sym(':')
        .ignore_then(
            ident_parser()
                .separated_by(op("+"))
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .or_not()
        .map(Option::unwrap_or_default)
        .boxed()
}

fn keyword<'src>(
    word: &'static str,
) -> impl Parser<'src, &'src str, (), extra::Err<Rich<'src, char>>> + Clone {
    text::ascii::keyword(word).ignored().padded_by(padding())
}

fn sym<'src>(c: char) -> impl Parser<'src, &'src str, (), extra::Err<Rich<'src, char>>> + Clone {
    just(c).ignored().padded_by(padding())
}

fn op<'src>(
    token: &'static str,
) -> impl Parser<'src, &'src str, (), extra::Err<Rich<'src, char>>> + Clone {
    just(token).ignored().padded_by(padding())
}

fn flow_marker<'src>()
-> impl Parser<'src, &'src str, ValueFlow, extra::Err<Rich<'src, char>>> + Clone {
    choice((
        keyword("mut").to(ValueFlow::ReturnedChanged),
        keyword("take").to(ValueFlow::NotReturned),
    ))
    .or_not()
    .map(|flow| flow.unwrap_or(ValueFlow::ReturnedUnchanged))
}

fn padding<'src>() -> impl Parser<'src, &'src str, (), extra::Err<Rich<'src, char>>> + Clone {
    let whitespace = any().filter(|c: &char| c.is_whitespace()).ignored();
    let line_comment = just("//").ignore_then(none_of('\n').repeated()).ignored();
    choice((whitespace, line_comment)).repeated().ignored()
}
