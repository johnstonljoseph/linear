//! Semantic core prototype for the Linear language.
//!
//! The core currently has a small type arena, linear capabilities,
//! expression/function checking, static function values, primitive collections,
//! and an interpreter. Frontends are intentionally kept out of this crate layer.

pub mod core;
pub mod eval;
pub mod flow;
pub mod frontend;
pub mod id;
pub mod types;

pub use core::{
    BuiltinOp, CoreError, CoreProgram, Expr, Function, GlobalDecl, GlobalDef, GlobalExpr, MatchArm,
    Param, Statement,
};
pub use eval::{EvalError, Evaluator, Value};
pub use flow::{
    FlowViolation, FunctionFlow, ParamContract, Provenance, check_function_contract,
    infer_function_flows,
};
pub use id::{FunctionId, GlobalId, TypeId, ValueId};
pub use types::{
    Capabilities, Component, ComponentName, DeclaredCapabilities, TypeDef, TypeError, TypeKind,
    TypeStore,
};
