//! Semantic core prototype for the Linear language.
//!
//! Reading order for the core:
//!
//! 1. `types`: the type arena and `Dup`/`Zap` capabilities.
//! 2. `core`: the linear IR — programs, expressions, and the checker.
//! 3. `flow`: value-flow analysis — which outputs are provably the same
//!    version of which inputs, and the flow-marker contracts checked
//!    against it.
//! 4. `eval`: a reference interpreter for checked programs.
//!
//! `frontend` (parser, lowering, diagnostics) sits on top and is deliberately
//! not part of the semantic core.

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
    FlowViolation, FunctionFlow, ParamContract, PathStep, Place, Provenance,
    check_function_contract, infer_function_flows,
};
pub use id::{FunctionId, GlobalId, TypeId, ValueId};
pub use types::{
    Capabilities, Component, ComponentName, DeclaredCapabilities, TypeDef, TypeError, TypeKind,
    TypeStore,
};
