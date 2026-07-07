//! Value-flow verification for parameter markers.
//!
//! Every function output slot is related to the function's parameters by a
//! provenance: either it is provably the very same version of one parameter,
//! or nothing is proven. Builtins declare their provenance axiomatically
//! (finite arithmetic returns its operands unchanged, `FiniteNext` returns a
//! changed value, `dup` propagates the source version to both copies), and
//! function summaries are inferred bottom-up over the call graph with a
//! fixpoint so recursion and mutual recursion converge.
//!
//! Frontend flow markers are contracts against these summaries:
//!
//! - an unmarked (borrowed) parameter must be provably returned as the same
//!   version in its hidden output slot;
//! - a `mut` parameter whose slot is provably the same version is reported as
//!   actually being a borrow;
//! - `take` parameters have nothing to verify beyond linearity, which the core
//!   checker already enforces.

use std::collections::HashMap;

use crate::core::{BuiltinOp, CoreProgram, Expr, Function, Statement};
use crate::id::{FunctionId, ValueId};

/// How a value relates to the enclosing function's parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Unconstrained fixpoint start. Survives only on paths with no
    /// terminating definition (e.g. bare infinite recursion), where any
    /// contract holds vacuously.
    Top,
    /// Provably the same version of parameter `index`.
    Param(usize),
    /// No proven relationship to any parameter.
    Other,
}

impl Provenance {
    fn meet(self, other: Self) -> Self {
        match (self, other) {
            (Self::Top, provenance) | (provenance, Self::Top) => provenance,
            (Self::Param(left), Self::Param(right)) if left == right => Self::Param(left),
            _ => Self::Other,
        }
    }
}

/// Inferred provenance of each core output slot of one function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionFlow {
    pub outputs: Vec<Provenance>,
}

/// The declared contract of one frontend parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamContract {
    /// Unmarked: returned unchanged, same version.
    Borrowed,
    /// `mut`: returned changed.
    Updated,
    /// `take`: consumed, no output slot.
    Consumed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowViolation {
    /// An unmarked parameter's output slot is not provably the same version
    /// of that parameter.
    BorrowNotProven { function: String, param: String },
    /// A `mut` parameter's output slot is provably the same version of that
    /// parameter on every path, so the marker overstates what happens.
    MutIsBorrow { function: String, param: String },
    /// A `take` parameter provably escapes unchanged into some output slot on
    /// every path, so it is really moved through, not consumed.
    TakeIsBorrow { function: String, param: String },
}

/// Infer output provenances for every function in `program`.
///
/// The program is expected to have passed `CoreProgram::check`; on malformed
/// programs the analysis degrades to `Other` rather than panicking.
pub fn infer_function_flows(program: &CoreProgram) -> HashMap<FunctionId, FunctionFlow> {
    let mut flows: HashMap<FunctionId, FunctionFlow> = program
        .functions()
        .map(|(id, function)| {
            (
                id,
                FunctionFlow {
                    outputs: vec![Provenance::Top; function.outputs.len()],
                },
            )
        })
        .collect();

    loop {
        let mut changed = false;
        for (id, function) in program.functions() {
            let inferred = analyze_function(function, &flows);
            let current = flows.get_mut(&id).expect("flow initialized per function");
            for (slot, inferred) in current.outputs.iter_mut().zip(inferred) {
                let next = slot.meet(inferred);
                if next != *slot {
                    *slot = next;
                    changed = true;
                }
            }
        }
        if !changed {
            return flows;
        }
    }
}

/// Check one function's declared parameter contracts against its inferred
/// flow. `params` lists every parameter in declaration order; hidden output
/// slots are assigned to non-`Consumed` parameters in that same order.
pub fn check_function_contract(
    function_name: &str,
    params: &[(String, ParamContract)],
    flow: &FunctionFlow,
) -> Vec<FlowViolation> {
    let mut violations = Vec::new();
    let mut slot = 0;
    for (index, (param_name, contract)) in params.iter().enumerate() {
        match contract {
            ParamContract::Consumed => {
                // A consumed parameter has no slot of its own, but if its
                // exact version provably reappears in any output, the value
                // was moved through rather than taken.
                let escapes = flow
                    .outputs
                    .iter()
                    .any(|output| matches!(output, Provenance::Param(source) if *source == index));
                if escapes {
                    violations.push(FlowViolation::TakeIsBorrow {
                        function: function_name.to_owned(),
                        param: param_name.clone(),
                    });
                }
            }
            ParamContract::Borrowed => {
                let proven = match flow.outputs.get(slot) {
                    Some(Provenance::Top) => true,
                    Some(Provenance::Param(source)) => *source == index,
                    Some(Provenance::Other) | None => false,
                };
                if !proven {
                    violations.push(FlowViolation::BorrowNotProven {
                        function: function_name.to_owned(),
                        param: param_name.clone(),
                    });
                }
                slot += 1;
            }
            ParamContract::Updated => {
                if matches!(flow.outputs.get(slot), Some(Provenance::Param(i)) if *i == index) {
                    violations.push(FlowViolation::MutIsBorrow {
                        function: function_name.to_owned(),
                        param: param_name.clone(),
                    });
                }
                slot += 1;
            }
        }
    }
    violations
}

fn analyze_function(
    function: &Function,
    flows: &HashMap<FunctionId, FunctionFlow>,
) -> Vec<Provenance> {
    let mut env = HashMap::new();
    for (index, param) in function.inputs.iter().enumerate() {
        env.insert(param.id, Provenance::Param(index));
    }
    analyze_block(&function.body, &function.returns, env, flows)
}

fn analyze_block(
    body: &[Statement],
    returns: &[ValueId],
    mut env: HashMap<ValueId, Provenance>,
    flows: &HashMap<FunctionId, FunctionFlow>,
) -> Vec<Provenance> {
    for statement in body {
        analyze_statement(statement, &mut env, flows);
    }
    returns
        .iter()
        .map(|id| env.get(id).copied().unwrap_or(Provenance::Other))
        .collect()
}

fn analyze_statement(
    statement: &Statement,
    env: &mut HashMap<ValueId, Provenance>,
    flows: &HashMap<FunctionId, FunctionFlow>,
) {
    let results = analyze_expr(&statement.expr, statement.results.len(), env, flows);
    for (index, id) in statement.results.iter().enumerate() {
        let provenance = results.get(index).copied().unwrap_or(Provenance::Other);
        env.insert(*id, provenance);
    }
}

fn analyze_expr(
    expr: &Expr,
    result_count: usize,
    env: &HashMap<ValueId, Provenance>,
    flows: &HashMap<FunctionId, FunctionFlow>,
) -> Vec<Provenance> {
    let lookup = |id: ValueId| env.get(&id).copied().unwrap_or(Provenance::Other);
    match expr {
        Expr::Dup { value } => {
            // Both copies of an immutable value are the same version.
            let provenance = lookup(*value);
            vec![provenance, provenance]
        }
        Expr::Zap { .. } => Vec::new(),
        Expr::Call { function, args } => match flows.get(function) {
            Some(flow) => flow
                .outputs
                .iter()
                .map(|slot| match slot {
                    Provenance::Top => Provenance::Top,
                    Provenance::Param(arg_index) => args
                        .get(*arg_index)
                        .map(|id| lookup(*id))
                        .unwrap_or(Provenance::Other),
                    Provenance::Other => Provenance::Other,
                })
                .collect(),
            None => vec![Provenance::Other; result_count],
        },
        Expr::Builtin { op, args } => analyze_builtin(op, args, &lookup),
        Expr::Match { arms, .. } => {
            let mut joined: Option<Vec<Provenance>> = None;
            for arm in arms {
                let mut arm_env = env.clone();
                // The payload is a piece of the scrutinee, not any parameter.
                arm_env.insert(arm.payload, Provenance::Other);
                let returned = analyze_block(&arm.body, &arm.returns, arm_env, flows);
                joined = Some(match joined {
                    None => returned,
                    Some(previous) => previous
                        .iter()
                        .enumerate()
                        .map(|(index, provenance)| {
                            provenance.meet(
                                returned.get(index).copied().unwrap_or(Provenance::Other),
                            )
                        })
                        .collect(),
                });
            }
            joined.unwrap_or_else(|| vec![Provenance::Other; result_count])
        }
        // Structure operations produce parts or fresh compositions, never the
        // same version of a whole parameter. Path-sensitive provenance
        // (project + reassemble = same version) is future work for the
        // borrow/plug design.
        Expr::Unit
        | Expr::FiniteLiteral { .. }
        | Expr::FunctionRef { .. }
        | Expr::Product { .. }
        | Expr::SplitProduct { .. }
        | Expr::ProjectProduct { .. }
        | Expr::InsertProductField { .. }
        | Expr::SumInject { .. }
        | Expr::Global { .. }
        | Expr::CallValue { .. } => vec![Provenance::Other; result_count],
    }
}

fn analyze_builtin(
    op: &BuiltinOp,
    args: &[ValueId],
    lookup: &dyn Fn(ValueId) -> Provenance,
) -> Vec<Provenance> {
    let arg = |index: usize| {
        args.get(index)
            .map(|id| lookup(*id))
            .unwrap_or(Provenance::Other)
    };
    match op {
        // Observer ops return their operands unchanged before the fresh
        // visible result.
        BuiltinOp::FiniteAdd { .. }
        | BuiltinOp::FiniteSub { .. }
        | BuiltinOp::FiniteMul { .. }
        | BuiltinOp::FiniteEq { .. }
        | BuiltinOp::FiniteLt { .. } => vec![arg(0), arg(1), Provenance::Other],
        // FiniteNext consumes its operand and returns a changed version.
        BuiltinOp::FiniteNext { .. } => vec![Provenance::Other],
    }
}
