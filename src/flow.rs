//! Value-flow analysis: which outputs are provably the *same version* of
//! which inputs.
//!
//! # The model
//!
//! Linearity (checked in `core.rs`) counts uses; it cannot tell whether a
//! value that flows from an input to an output is still *the same value*.
//! This module adds that judgment. Every value in a function body gets a
//! provenance:
//!
//! - `Same(place)`: provably the same version of the value at `place`, where
//!   a [`Place`] is a parameter plus a path of product-field / sum-variant
//!   steps into it. `Same(param 0, [])` is the whole first argument;
//!   `Same(param 0, [Field(1)])` is its second field.
//! - `Context { whole, hole }`: a one-hole context of the value at `whole` —
//!   everything except the field at `hole`. Produced by `FocusField`,
//!   consumed by `PlugField`.
//! - `Top`: unconstrained. Only survives on paths that never produce a value
//!   (bare infinite recursion), where any claim holds vacuously.
//! - `Fresh`: a new value. Literals, arithmetic results, and anything
//!   constructed rather than moved are fresh by axiom. Anything the analysis
//!   cannot relate to an input is *also* fresh: computing a value too
//!   indirectly to track is semantically indistinguishable from computing a
//!   new one. There is deliberately no "unknown" verdict — a borrow claim
//!   over a fresh value is a hard error and the program must be rewritten
//!   into provable form.
//!
//! # The axioms
//!
//! Builtins declare their flow axiomatically: observer ops (finite
//! arithmetic/comparison) return their operands unchanged; `next` returns a
//! changed version; `dup` gives both copies the source's version (sound for
//! immutable value semantics: copies are indistinguishable).
//!
//! Structure operations follow the one-hole-context (type derivative) rules:
//!
//! - focus:  `Same(w)` --focus f-->  `Same(w.f)` + `Context { w, f }`
//! - plug:   `Context { w, f }` + `Same(w.f)`  -->  `Same(w)`
//! - split:  `Same(w)`  -->  `Same(w.0), ..., Same(w.n-1)`
//! - build:  `Same(w.0), ..., Same(w.n-1)` at the original type -->  `Same(w)`
//! - match:  arm for variant k of `Same(w)` binds payload `Same(w.k)`
//! - inject: `Same(w.k)` at variant k of the original type  -->  `Same(w)`
//!
//! Taking a value apart and putting the identical parts back in the identical
//! places is therefore recognized as the same version, at any nesting depth.
//! The `build`, `plug`, and `inject` rules compare against the type at the
//! source place, so rebuilding a *different* (even structurally identical)
//! nominal type is not the same version.
//!
//! # Composition and recursion
//!
//! Each function gets a summary: one provenance per core output slot, stated
//! in terms of its own parameters. Call sites substitute: if the callee's
//! slot is `Same(param k at path q)` and the caller's argument k is
//! `Same(w)`, the result is `Same(w ++ q)`. Summaries are computed by a
//! fixpoint over the call graph starting from `Top`, so recursion and mutual
//! recursion converge (each slot can only descend `Top` -> concrete ->
//! `Fresh`, so the iteration terminates).
//!
//! # The contracts
//!
//! Frontend flow markers are claims checked against the inferred summaries
//! ([`check_function_contract`]):
//!
//! - unmarked (borrow): the parameter's hidden output slot must be provably
//!   `Same` the whole parameter — a hard error otherwise;
//! - `mut`: if the slot is provably `Same` the whole parameter on every
//!   path, the value never changes and the marker is reported as a borrow;
//! - `take`: if the whole parameter provably escapes into *any* output slot,
//!   it was moved through, not consumed, and is reported as a borrow.

use std::collections::HashMap;

use crate::core::{BuiltinOp, CoreProgram, Expr, Function, Statement};
use crate::id::{FunctionId, TypeId, ValueId};
use crate::types::{TypeKind, TypeStore};

/// One step of a path into a value: a product field or a sum variant payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStep {
    Field(usize),
    Variant(usize),
}

/// A location inside a function's arguments: parameter `param`, then follow
/// `path` inward. An empty path is the whole parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub param: usize,
    pub path: Vec<PathStep>,
}

impl Place {
    /// The whole parameter `param`.
    pub fn param(param: usize) -> Self {
        Self {
            param,
            path: Vec::new(),
        }
    }

    fn extended(&self, step: PathStep) -> Self {
        let mut path = self.path.clone();
        path.push(step);
        Self {
            param: self.param,
            path,
        }
    }

    /// Split into (parent place, last step); `None` if the path is empty.
    fn parent(&self) -> Option<(Self, PathStep)> {
        let (last, front) = self.path.split_last()?;
        Some((
            Self {
                param: self.param,
                path: front.to_vec(),
            },
            *last,
        ))
    }
}

/// What is provable about one value, relative to the enclosing function's
/// parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Unconstrained fixpoint start; survives only on paths that never
    /// terminate, where any contract holds vacuously.
    Top,
    /// Provably the same version of the value at `place`.
    Same(Place),
    /// A one-hole context of the value at `whole`: all of it except the
    /// product field at index `hole`.
    Context { whole: Place, hole: usize },
    /// A new value: introduced here (literal, builtin result,
    /// construction), or not provably related to any input — which is
    /// treated identically, since an untrackable computation is
    /// indistinguishable from a fresh one. There is no "unknown".
    Fresh,
}

impl Provenance {
    /// `Same` the whole of parameter `index` — what a borrow must prove.
    pub fn whole_param(index: usize) -> Self {
        Self::Same(Place::param(index))
    }

    fn meet(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Top, provenance) | (provenance, Self::Top) => provenance.clone(),
            (left, right) if left == right => left.clone(),
            _ => Self::Fresh,
        }
    }

    fn is_whole_param(&self, index: usize) -> bool {
        matches!(self, Self::Same(place) if place.param == index && place.path.is_empty())
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
/// programs the analysis degrades to `Fresh` rather than panicking.
pub fn infer_function_flows(
    types: &TypeStore,
    program: &CoreProgram,
) -> HashMap<FunctionId, FunctionFlow> {
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
            let inferred = Analysis::new(types, function).run(&flows);
            let current = flows.get_mut(&id).expect("flow initialized per function");
            for (slot, inferred) in current.outputs.iter_mut().zip(inferred) {
                let next = slot.meet(&inferred);
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
                    .any(|output| output.is_whole_param(index));
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
                    Some(provenance) => provenance.is_whole_param(index),
                    None => false,
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
                if flow
                    .outputs
                    .get(slot)
                    .is_some_and(|output| output.is_whole_param(index))
                {
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

/// Per-function analysis state: the type store and the parameter types, which
/// are needed to resolve the type at a [`Place`] for the rebuild rules.
struct Analysis<'a> {
    types: &'a TypeStore,
    param_types: Vec<TypeId>,
    function: &'a Function,
}

type Env = HashMap<ValueId, Provenance>;

impl<'a> Analysis<'a> {
    fn new(types: &'a TypeStore, function: &'a Function) -> Self {
        Self {
            types,
            param_types: function.inputs.iter().map(|input| input.ty).collect(),
            function,
        }
    }

    fn run(&self, flows: &HashMap<FunctionId, FunctionFlow>) -> Vec<Provenance> {
        let mut env = Env::new();
        for (index, param) in self.function.inputs.iter().enumerate() {
            env.insert(param.id, Provenance::whole_param(index));
        }
        self.block(&self.function.body, &self.function.returns, env, flows)
    }

    fn block(
        &self,
        body: &[Statement],
        returns: &[ValueId],
        mut env: Env,
        flows: &HashMap<FunctionId, FunctionFlow>,
    ) -> Vec<Provenance> {
        for statement in body {
            let results = self.expr(&statement.expr, statement.results.len(), &env, flows);
            for (index, id) in statement.results.iter().enumerate() {
                let provenance = results.get(index).cloned().unwrap_or(Provenance::Fresh);
                env.insert(*id, provenance);
            }
        }
        returns
            .iter()
            .map(|id| env.get(id).cloned().unwrap_or(Provenance::Fresh))
            .collect()
    }

    fn expr(
        &self,
        expr: &Expr,
        result_count: usize,
        env: &Env,
        flows: &HashMap<FunctionId, FunctionFlow>,
    ) -> Vec<Provenance> {
        let of = |id: &ValueId| env.get(id).cloned().unwrap_or(Provenance::Fresh);
        match expr {
            // ---- identity-preserving operations ----
            Expr::Dup { value } => {
                // Both copies of an immutable value are the same version.
                let provenance = of(value);
                vec![provenance.clone(), provenance]
            }
            Expr::Zap { .. } => Vec::new(),

            // ---- take apart ----
            Expr::SplitProduct { value } => match of(value) {
                Provenance::Same(whole) => (0..result_count)
                    .map(|field| Provenance::Same(whole.extended(PathStep::Field(field))))
                    .collect(),
                Provenance::Top => vec![Provenance::Top; result_count],
                _ => vec![Provenance::Fresh; result_count],
            },
            Expr::FocusField { value, field, .. } => match of(value) {
                Provenance::Same(whole) => vec![
                    Provenance::Same(whole.extended(PathStep::Field(*field))),
                    Provenance::Context {
                        whole,
                        hole: *field,
                    },
                ],
                Provenance::Top => vec![Provenance::Top, Provenance::Top],
                _ => vec![Provenance::Fresh, Provenance::Fresh],
            },

            // ---- put back together ----
            Expr::Product { ty, fields } => vec![self.rebuild_product(*ty, fields, env)],
            Expr::PlugField {
                ty,
                field,
                part,
                context,
            } => vec![self.plug(*ty, *field, of(part), of(context))],
            Expr::SumInject {
                ty,
                variant,
                payload,
            } => vec![self.reinject(*ty, *variant, of(payload))],

            // ---- control flow ----
            Expr::Match { scrutinee, arms } => {
                let scrutinee = of(scrutinee);
                let mut joined: Option<Vec<Provenance>> = None;
                for arm in arms {
                    let payload = match &scrutinee {
                        Provenance::Same(whole) => {
                            Provenance::Same(whole.extended(PathStep::Variant(arm.variant)))
                        }
                        Provenance::Top => Provenance::Top,
                        _ => Provenance::Fresh,
                    };
                    let mut arm_env = env.clone();
                    arm_env.insert(arm.payload, payload);
                    let returned = self.block(&arm.body, &arm.returns, arm_env, flows);
                    joined = Some(match joined {
                        None => returned,
                        Some(previous) => previous
                            .iter()
                            .enumerate()
                            .map(|(index, provenance)| {
                                provenance.meet(returned.get(index).unwrap_or(&Provenance::Fresh))
                            })
                            .collect(),
                    });
                }
                joined.unwrap_or_else(|| vec![Provenance::Fresh; result_count])
            }

            // ---- calls: substitute the callee's summary ----
            Expr::Call { function, args } => match flows.get(function) {
                Some(flow) => flow
                    .outputs
                    .iter()
                    .map(|slot| self.substitute(slot, args, env))
                    .collect(),
                None => vec![Provenance::Fresh; result_count],
            },

            // ---- builtins: axiomatic flow ----
            Expr::Builtin { op, args } => {
                let arg = |index: usize| args.get(index).map(of).unwrap_or(Provenance::Fresh);
                match op {
                    // Observer ops return their operands unchanged before the
                    // fresh visible result.
                    BuiltinOp::FiniteAdd { .. }
                    | BuiltinOp::FiniteSub { .. }
                    | BuiltinOp::FiniteMul { .. }
                    | BuiltinOp::FiniteEq { .. }
                    | BuiltinOp::FiniteLt { .. } => vec![arg(0), arg(1), Provenance::Fresh],
                    // Next consumes its operand and returns a changed version.
                    BuiltinOp::FiniteNext { .. } => vec![Provenance::Fresh],
                }
            }

            // ---- everything else produces fresh values ----
            Expr::Unit
            | Expr::FiniteLiteral { .. }
            | Expr::FunctionRef { .. }
            | Expr::Global { .. }
            | Expr::CallValue { .. } => vec![Provenance::Fresh; result_count],
        }
    }

    /// build: a product constructed from exactly the same-version fields of
    /// one place, in order, at that place's own type, is the same version of
    /// that place.
    fn rebuild_product(&self, ty: TypeId, fields: &[ValueId], env: &Env) -> Provenance {
        let Some(first) = fields.first() else {
            return Provenance::Fresh;
        };
        let Some(Provenance::Same(first_place)) = env.get(first) else {
            return Provenance::Fresh;
        };
        let Some((whole, PathStep::Field(0))) = first_place.parent() else {
            return Provenance::Fresh;
        };
        for (index, field) in fields.iter().enumerate() {
            let expected = Provenance::Same(whole.extended(PathStep::Field(index)));
            if env.get(field) != Some(&expected) {
                return Provenance::Fresh;
            }
        }
        if self.type_at(&whole) == Some(ty) {
            Provenance::Same(whole)
        } else {
            Provenance::Fresh
        }
    }

    /// plug: a context of `whole` filled at its own hole with the same
    /// version of the part that came out, at `whole`'s own type, is the same
    /// version of `whole`.
    fn plug(&self, ty: TypeId, field: usize, part: Provenance, context: Provenance) -> Provenance {
        let Provenance::Context { whole, hole } = context else {
            return Provenance::Fresh;
        };
        if hole != field {
            return Provenance::Fresh;
        }
        if part != Provenance::Same(whole.extended(PathStep::Field(field))) {
            return Provenance::Fresh;
        }
        if self.type_at(&whole) == Some(ty) {
            Provenance::Same(whole)
        } else {
            Provenance::Fresh
        }
    }

    /// inject: the payload of variant k of some place, re-injected at variant
    /// k of that place's own type, is the same version of that place. (The
    /// payload provenance can only exist inside the match arm that proved the
    /// value is that variant.)
    fn reinject(&self, ty: TypeId, variant: usize, payload: Provenance) -> Provenance {
        let Provenance::Same(place) = payload else {
            return Provenance::Fresh;
        };
        let Some((whole, PathStep::Variant(step))) = place.parent() else {
            return Provenance::Fresh;
        };
        if step != variant {
            return Provenance::Fresh;
        }
        if self.type_at(&whole) == Some(ty) {
            Provenance::Same(whole)
        } else {
            Provenance::Fresh
        }
    }

    /// Substitute a callee summary slot with the caller's argument
    /// provenances: the callee's `param k at path q` becomes the caller's
    /// provenance of argument k, extended by q.
    fn substitute(&self, slot: &Provenance, args: &[ValueId], env: &Env) -> Provenance {
        let arg = |place: &Place| {
            args.get(place.param)
                .and_then(|id| env.get(id))
                .cloned()
                .unwrap_or(Provenance::Fresh)
        };
        match slot {
            Provenance::Top => Provenance::Top,
            Provenance::Fresh => Provenance::Fresh,
            Provenance::Same(callee_place) => match arg(callee_place) {
                Provenance::Top => Provenance::Top,
                Provenance::Same(whole) => Provenance::Same(append(whole, &callee_place.path)),
                // Returning an argument whole (empty path) preserves whatever
                // it was, including a context. Reaching *into* a context
                // would require remapping field indices around the hole, so
                // it degrades to Fresh.
                Provenance::Context { whole, hole } if callee_place.path.is_empty() => {
                    Provenance::Context { whole, hole }
                }
                _ => Provenance::Fresh,
            },
            Provenance::Context {
                whole: callee_place,
                hole,
            } => match arg(callee_place) {
                Provenance::Top => Provenance::Top,
                Provenance::Same(whole) => Provenance::Context {
                    whole: append(whole, &callee_place.path),
                    hole: *hole,
                },
                _ => Provenance::Fresh,
            },
        }
    }

    /// Resolve the type at a place by walking the parameter's type along the
    /// path. `None` if the path does not fit the types (malformed program).
    fn type_at(&self, place: &Place) -> Option<TypeId> {
        let mut ty = *self.param_types.get(place.param)?;
        for step in &place.path {
            let def = self.types.get(ty)?;
            ty = match (&def.kind, step) {
                (TypeKind::Product(fields), PathStep::Field(index)) => fields.get(*index)?.ty,
                (TypeKind::Sum(variants), PathStep::Variant(index)) => variants.get(*index)?.ty,
                _ => return None,
            };
        }
        Some(ty)
    }
}

fn append(mut place: Place, path: &[PathStep]) -> Place {
    place.path.extend_from_slice(path);
    place
}
