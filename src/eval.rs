//! A reference interpreter for checked core programs.
//!
//! This is not the proving backend; it exists to make the semantics
//! executable while the language is designed. It assumes the program passed
//! `CoreProgram::check` (linearity means every environment entry is consumed
//! by removal, so `dup` is the only clone and drop never happens silently),
//! but it still reports malformed programs as errors rather than panicking.
//! Recursion depth and total work are bounded by a step limit; exceeding it
//! is the interpreter's stand-in for nontermination, which the language
//! treats as a completeness failure for that input.

use std::collections::HashMap;

use crate::core::{BuiltinOp, CoreProgram, Expr, Function, GlobalExpr, MatchArm, Statement};
use crate::id::{FunctionId, GlobalId, TypeId, ValueId};
use crate::types::{TypeKind, TypeStore};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    Unit,
    Finite(u128),
    Function(FunctionId),
    Symbol(String),
    Text(String),
    Product(Vec<Value>),
    Sum { variant: usize, payload: Box<Value> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvalError {
    UnknownFunction(FunctionId),
    UnknownGlobal(GlobalId),
    UndefinedGlobal(GlobalId),
    UnknownValue(ValueId),
    DuplicateValue(ValueId),
    Arity {
        expected: usize,
        actual: usize,
    },
    RuntimeType {
        expected: &'static str,
        actual: Value,
    },
    NotFinite(TypeId),
    IndexOutOfBounds {
        index: usize,
        len: usize,
    },
    MissingMatchArm {
        variant: usize,
    },
    StepLimitExceeded,
}

#[derive(Clone, Debug)]
pub struct Evaluator<'a> {
    types: &'a TypeStore,
    program: &'a CoreProgram,
    step_limit: usize,
}

impl<'a> Evaluator<'a> {
    pub fn new(types: &'a TypeStore, program: &'a CoreProgram) -> Self {
        Self {
            types,
            program,
            step_limit: 100_000,
        }
    }

    pub fn with_step_limit(mut self, step_limit: usize) -> Self {
        self.step_limit = step_limit;
        self
    }

    pub fn run_function(
        &self,
        function: FunctionId,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, EvalError> {
        let mut steps = self.step_limit;
        self.run_function_inner(function, args, &mut steps)
    }

    fn run_function_inner(
        &self,
        function: FunctionId,
        args: Vec<Value>,
        steps: &mut usize,
    ) -> Result<Vec<Value>, EvalError> {
        self.take_step(steps)?;
        let function = self
            .program
            .get(function)
            .ok_or(EvalError::UnknownFunction(function))?;
        self.run_function_body(function, args, steps)
    }

    fn run_function_body(
        &self,
        function: &Function,
        args: Vec<Value>,
        steps: &mut usize,
    ) -> Result<Vec<Value>, EvalError> {
        if args.len() != function.inputs.len() {
            return Err(EvalError::Arity {
                expected: function.inputs.len(),
                actual: args.len(),
            });
        }
        let mut env = HashMap::new();
        for (param, value) in function.inputs.iter().zip(args) {
            define(&mut env, param.id, value)?;
        }
        self.run_block(&function.body, &function.returns, env, steps)
    }

    fn run_block(
        &self,
        body: &[Statement],
        returns: &[ValueId],
        mut env: HashMap<ValueId, Value>,
        steps: &mut usize,
    ) -> Result<Vec<Value>, EvalError> {
        for statement in body {
            self.take_step(steps)?;
            let values = self.eval_expr(&statement.expr, &mut env, steps)?;
            if values.len() != statement.results.len() {
                return Err(EvalError::Arity {
                    expected: statement.results.len(),
                    actual: values.len(),
                });
            }
            for (id, value) in statement.results.iter().copied().zip(values) {
                define(&mut env, id, value)?;
            }
        }

        returns
            .iter()
            .copied()
            .map(|id| consume(&mut env, id))
            .collect()
    }

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &mut HashMap<ValueId, Value>,
        steps: &mut usize,
    ) -> Result<Vec<Value>, EvalError> {
        match expr {
            Expr::Unit => Ok(vec![Value::Unit]),
            Expr::FiniteLiteral { value, .. } => Ok(vec![Value::Finite(*value)]),
            Expr::FunctionRef { function, .. } => Ok(vec![Value::Function(*function)]),
            Expr::Product { fields, .. } => fields
                .iter()
                .copied()
                .map(|field| consume(env, field))
                .collect::<Result<Vec<_>, _>>()
                .map(|fields| vec![Value::Product(fields)]),
            Expr::SplitProduct { value } => match consume(env, *value)? {
                Value::Product(fields) => Ok(fields),
                actual => Err(EvalError::RuntimeType {
                    expected: "product",
                    actual,
                }),
            },
            Expr::FocusField { value, field, .. } => match consume(env, *value)? {
                Value::Product(mut fields) => {
                    if *field >= fields.len() {
                        return Err(EvalError::IndexOutOfBounds {
                            index: *field,
                            len: fields.len(),
                        });
                    }
                    let selected = fields.remove(*field);
                    Ok(vec![selected, Value::Product(fields)])
                }
                actual => Err(EvalError::RuntimeType {
                    expected: "product",
                    actual,
                }),
            },
            Expr::PlugField {
                field,
                part,
                context,
                ..
            } => {
                let part = consume(env, *part)?;
                match consume(env, *context)? {
                    Value::Product(mut fields) => {
                        if *field > fields.len() {
                            return Err(EvalError::IndexOutOfBounds {
                                index: *field,
                                len: fields.len(),
                            });
                        }
                        fields.insert(*field, part);
                        Ok(vec![Value::Product(fields)])
                    }
                    actual => Err(EvalError::RuntimeType {
                        expected: "product",
                        actual,
                    }),
                }
            }
            Expr::SumInject {
                variant, payload, ..
            } => {
                let payload = consume(env, *payload)?;
                Ok(vec![Value::Sum {
                    variant: *variant,
                    payload: Box::new(payload),
                }])
            }
            Expr::Match { scrutinee, arms } => match consume(env, *scrutinee)? {
                Value::Sum { variant, payload } => {
                    let arm = arms
                        .iter()
                        .find(|arm| arm.variant == variant)
                        .ok_or(EvalError::MissingMatchArm { variant })?;
                    let captured = std::mem::take(env);
                    self.run_match_arm(arm, *payload, captured, steps)
                }
                actual => Err(EvalError::RuntimeType {
                    expected: "sum",
                    actual,
                }),
            },
            Expr::Global { global } => {
                let value = self.eval_global(*global, steps)?;
                Ok(vec![value])
            }
            Expr::Call { function, args } => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| consume(env, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                self.run_function_inner(*function, args, steps)
            }
            Expr::CallValue { function, arg } => match consume(env, *function)? {
                Value::Function(function) => {
                    let arg = consume(env, *arg)?;
                    let callee = self
                        .program
                        .get(function)
                        .ok_or(EvalError::UnknownFunction(function))?;
                    let args = unpack_call_value_arg(arg, callee.inputs.len())?;
                    let values = self.run_function_inner(function, args, steps)?;
                    Ok(vec![pack_call_value_output(values)])
                }
                actual => Err(EvalError::RuntimeType {
                    expected: "function",
                    actual,
                }),
            },
            Expr::Builtin { op, args } => {
                let args = args
                    .iter()
                    .copied()
                    .map(|arg| consume(env, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                self.eval_builtin(op, args)
            }
            Expr::Dup { value } => {
                let value = consume(env, *value)?;
                Ok(vec![value.clone(), value])
            }
            Expr::Zap { value } => {
                consume(env, *value)?;
                Ok(vec![])
            }
        }
    }

    fn run_match_arm(
        &self,
        arm: &MatchArm,
        payload: Value,
        mut env: HashMap<ValueId, Value>,
        steps: &mut usize,
    ) -> Result<Vec<Value>, EvalError> {
        define(&mut env, arm.payload, payload)?;
        self.run_block(&arm.body, &arm.returns, env, steps)
    }

    fn eval_global(&self, global: GlobalId, steps: &mut usize) -> Result<Value, EvalError> {
        self.take_step(steps)?;
        let value = self
            .program
            .get_global_def(global)
            .ok_or(EvalError::UndefinedGlobal(global))?;
        self.eval_global_expr(value, steps)
    }

    fn eval_global_expr(&self, expr: &GlobalExpr, steps: &mut usize) -> Result<Value, EvalError> {
        self.take_step(steps)?;
        match expr {
            GlobalExpr::Unit => Ok(Value::Unit),
            GlobalExpr::FiniteLiteral { value, .. } => Ok(Value::Finite(*value)),
            GlobalExpr::SymbolLiteral { value, .. } => Ok(Value::Symbol(value.clone())),
            GlobalExpr::TextLiteral { value, .. } => Ok(Value::Text(value.clone())),
            GlobalExpr::FunctionRef { function, .. } => Ok(Value::Function(*function)),
            GlobalExpr::Product { fields, .. } => fields
                .iter()
                .map(|field| self.eval_global_expr(field, steps))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Product),
            GlobalExpr::SumInject {
                variant, payload, ..
            } => Ok(Value::Sum {
                variant: *variant,
                payload: Box::new(self.eval_global_expr(payload, steps)?),
            }),
        }
    }

    fn eval_builtin(&self, op: &BuiltinOp, args: Vec<Value>) -> Result<Vec<Value>, EvalError> {
        match op {
            BuiltinOp::FiniteAdd { ty } => {
                let modulus = finite_cardinality(self.types, *ty)?;
                let [lhs, rhs] = finite_pair(args)?;
                Ok(vec![
                    Value::Finite(lhs),
                    Value::Finite(rhs),
                    Value::Finite(mod_add(lhs, rhs, modulus)),
                ])
            }
            BuiltinOp::FiniteSub { ty } => {
                let modulus = finite_cardinality(self.types, *ty)?;
                let [lhs, rhs] = finite_pair(args)?;
                Ok(vec![
                    Value::Finite(lhs),
                    Value::Finite(rhs),
                    Value::Finite(mod_sub(lhs, rhs, modulus)),
                ])
            }
            BuiltinOp::FiniteMul { ty } => {
                let modulus = finite_cardinality(self.types, *ty)?;
                let [lhs, rhs] = finite_pair(args)?;
                Ok(vec![
                    Value::Finite(lhs),
                    Value::Finite(rhs),
                    Value::Finite(mod_mul(lhs, rhs, modulus)),
                ])
            }
            BuiltinOp::FiniteEq { .. } => {
                let [lhs, rhs] = finite_pair(args)?;
                Ok(vec![
                    Value::Finite(lhs),
                    Value::Finite(rhs),
                    bool_value(lhs == rhs),
                ])
            }
            BuiltinOp::FiniteLt { .. } => {
                let [lhs, rhs] = finite_pair(args)?;
                Ok(vec![
                    Value::Finite(lhs),
                    Value::Finite(rhs),
                    bool_value(lhs < rhs),
                ])
            }
            BuiltinOp::FiniteNext { ty } => {
                let modulus = finite_cardinality(self.types, *ty)?;
                let [value] = expect_array(args)?;
                let value = expect_finite(value)?;
                Ok(vec![Value::Finite(mod_add(value, 1, modulus))])
            }
        }
    }

    fn take_step(&self, steps: &mut usize) -> Result<(), EvalError> {
        if *steps == 0 {
            return Err(EvalError::StepLimitExceeded);
        }
        *steps -= 1;
        Ok(())
    }
}

fn define(env: &mut HashMap<ValueId, Value>, id: ValueId, value: Value) -> Result<(), EvalError> {
    if env.insert(id, value).is_some() {
        Err(EvalError::DuplicateValue(id))
    } else {
        Ok(())
    }
}

fn consume(env: &mut HashMap<ValueId, Value>, id: ValueId) -> Result<Value, EvalError> {
    env.remove(&id).ok_or(EvalError::UnknownValue(id))
}

fn bool_value(value: bool) -> Value {
    Value::Sum {
        variant: usize::from(value),
        payload: Box::new(Value::Unit),
    }
}

fn unpack_call_value_arg(arg: Value, arity: usize) -> Result<Vec<Value>, EvalError> {
    match arity {
        0 => match arg {
            Value::Unit => Ok(Vec::new()),
            actual => Err(EvalError::RuntimeType {
                expected: "unit",
                actual,
            }),
        },
        1 => Ok(vec![arg]),
        expected => match arg {
            Value::Product(fields) if fields.len() == expected => Ok(fields),
            Value::Product(fields) => Err(EvalError::Arity {
                expected,
                actual: fields.len(),
            }),
            actual => Err(EvalError::RuntimeType {
                expected: "product",
                actual,
            }),
        },
    }
}

fn pack_call_value_output(values: Vec<Value>) -> Value {
    match values.len() {
        0 => Value::Unit,
        1 => values.into_iter().next().expect("checked arity"),
        _ => Value::Product(values),
    }
}

fn finite_cardinality(types: &TypeStore, id: TypeId) -> Result<u128, EvalError> {
    match types.get(id).map(|def| &def.kind) {
        Some(TypeKind::Finite { values }) => Ok(*values),
        _ => Err(EvalError::NotFinite(id)),
    }
}

fn finite_pair(args: Vec<Value>) -> Result<[u128; 2], EvalError> {
    let [lhs, rhs] = expect_array(args)?;
    Ok([expect_finite(lhs)?, expect_finite(rhs)?])
}

fn mod_add(lhs: u128, rhs: u128, modulus: u128) -> u128 {
    debug_assert!(modulus > 0);
    let lhs = lhs % modulus;
    let rhs = rhs % modulus;
    let threshold = modulus - rhs;
    if lhs >= threshold {
        lhs - threshold
    } else {
        lhs + rhs
    }
}

fn mod_sub(lhs: u128, rhs: u128, modulus: u128) -> u128 {
    debug_assert!(modulus > 0);
    let lhs = lhs % modulus;
    let rhs = rhs % modulus;
    if lhs >= rhs {
        lhs - rhs
    } else {
        modulus - (rhs - lhs)
    }
}

fn mod_mul(lhs: u128, rhs: u128, modulus: u128) -> u128 {
    debug_assert!(modulus > 0);
    let mut lhs = lhs % modulus;
    let mut rhs = rhs % modulus;
    let mut result = 0;
    while rhs != 0 {
        if rhs & 1 == 1 {
            result = mod_add(result, lhs, modulus);
        }
        rhs >>= 1;
        if rhs != 0 {
            lhs = mod_add(lhs, lhs, modulus);
        }
    }
    result
}

fn expect_array<const N: usize>(args: Vec<Value>) -> Result<[Value; N], EvalError> {
    let actual = args.len();
    args.try_into().map_err(|_| EvalError::Arity {
        expected: N,
        actual,
    })
}

fn expect_finite(value: Value) -> Result<u128, EvalError> {
    match value {
        Value::Finite(value) => Ok(value),
        actual => Err(EvalError::RuntimeType {
            expected: "finite",
            actual,
        }),
    }
}
