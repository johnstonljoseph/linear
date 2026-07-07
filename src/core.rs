//! The semantic core: a small linear IR that all surface syntax lowers into.
//!
//! # The model
//!
//! A [`CoreProgram`] is a set of functions plus non-function globals. A
//! [`Function`] body is a list of single-assignment [`Statement`]s: each
//! statement evaluates one [`Expr`] and binds its results to fresh
//! [`ValueId`]s, and the function ends by returning a list of ids. There is
//! no other control flow; branching is the [`Expr::Match`] expression, whose
//! arms are blocks of the same shape.
//!
//! Values are linear: every id is defined once and consumed exactly once —
//! by an expression that uses it, by being returned, or by an explicit
//! [`Expr::Zap`]. Copying is the explicit [`Expr::Dup`]. Both are gated by
//! the type's capabilities (see `types.rs`). The checker
//! ([`Function::check`]) enforces definition-before-use, single assignment,
//! single consumption, no live values left at a block end, and full type
//! agreement. It is a single forward pass; expressions carry enough type
//! annotations that nothing is inferred.
//!
//! # Taking values apart and putting them back
//!
//! Construction and deconstruction come in symmetric pairs:
//!
//! ```text
//! construct                 deconstruct
//! Product    (all fields)   SplitProduct  (all fields)
//! PlugField  (one field +   FocusField    (one field +
//!             its context)                 its one-hole context)
//! SumInject  (variant)      Match         (variant, per arm)
//! ```
//!
//! `FocusField`/`PlugField` are the one-hole-context (type derivative)
//! operations: focusing consumes a product and yields the chosen field plus
//! the *context* — an ordinary product of the remaining fields, i.e. the
//! derivative of the product type at that field; plugging is the inverse.
//! The value-flow analysis in `flow.rs` recognizes take-apart/put-back
//! round trips as returning the same version of the original value.
//!
//! # Functions as values
//!
//! Function types are unary `A -> B`. A multi-input/multi-output function is
//! referenced as a value ([`Expr::FunctionRef`]) by packing: zero
//! inputs/outputs pack as unit, one is used directly, several pack into a
//! product in declaration order. [`Expr::CallValue`] applies such a value.
//! No closures exist at this level; closure syntax is surface sugar over
//! product values plus static apply functions.

use std::collections::HashMap;

use crate::id::{FunctionId, GlobalId, TypeId, ValueId};
use crate::types::{TypeError, TypeKind, TypeStore};

/// A checkable core program: functions, global declarations, and literal
/// global definitions. Function and global names share one namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreProgram {
    functions: Vec<Function>,
    global_decls: Vec<GlobalDecl>,
    global_defs: Vec<Option<GlobalExpr>>,
    function_names: HashMap<String, FunctionId>,
    global_names: HashMap<String, GlobalId>,
}

impl CoreProgram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_function(&mut self, function: Function) -> Result<FunctionId, CoreError> {
        if let Some(name) = &function.name {
            validate_name(name)?;
            if self.name_exists(name) {
                return Err(CoreError::DuplicateFunctionName(name.clone()));
            }
        }
        let id = FunctionId(self.functions.len() as u32);
        if let Some(name) = &function.name {
            self.function_names.insert(name.clone(), id);
        }
        self.functions.push(function);
        Ok(id)
    }

    pub fn add_global_decl(&mut self, global: GlobalDecl) -> Result<GlobalId, CoreError> {
        validate_name(&global.name)?;
        if self.name_exists(&global.name) {
            return Err(CoreError::DuplicateGlobalName(global.name.clone()));
        }
        let id = GlobalId(self.global_decls.len() as u32);
        self.global_names.insert(global.name.clone(), id);
        self.global_decls.push(global);
        self.global_defs.push(None);
        Ok(id)
    }

    pub fn add_global_def(&mut self, global: GlobalDef) -> Result<GlobalId, CoreError> {
        validate_name(&global.name)?;
        if self.name_exists(&global.name) {
            return Err(CoreError::DuplicateGlobalName(global.name.clone()));
        }
        let id = GlobalId(self.global_decls.len() as u32);
        self.global_names.insert(global.name.clone(), id);
        self.global_decls.push(GlobalDecl {
            name: global.name,
            ty: global.ty,
        });
        self.global_defs.push(Some(global.value));
        Ok(id)
    }

    pub fn get(&self, id: FunctionId) -> Option<&Function> {
        self.functions.get(id.index())
    }

    pub fn get_mut(&mut self, id: FunctionId) -> Option<&mut Function> {
        self.functions.get_mut(id.index())
    }

    pub fn replace_function_body(
        &mut self,
        id: FunctionId,
        body: Vec<Statement>,
        returns: Vec<ValueId>,
    ) -> Result<(), CoreError> {
        let function = self.get_mut(id).ok_or(CoreError::UnknownFunction(id))?;
        function.body = body;
        function.returns = returns;
        Ok(())
    }

    pub fn get_global_decl(&self, id: GlobalId) -> Option<&GlobalDecl> {
        self.global_decls.get(id.index())
    }

    pub fn get_global_def(&self, id: GlobalId) -> Option<&GlobalExpr> {
        self.global_defs.get(id.index()).and_then(Option::as_ref)
    }

    pub fn function_id(&self, name: &str) -> Option<FunctionId> {
        self.function_names.get(name).copied()
    }

    pub fn global_decl_id(&self, name: &str) -> Option<GlobalId> {
        self.global_names.get(name).copied()
    }

    pub fn functions(&self) -> impl Iterator<Item = (FunctionId, &Function)> {
        self.functions
            .iter()
            .enumerate()
            .map(|(index, function)| (FunctionId(index as u32), function))
    }

    pub fn global_decls(&self) -> impl Iterator<Item = (GlobalId, &GlobalDecl)> {
        self.global_decls
            .iter()
            .enumerate()
            .map(|(index, global)| (GlobalId(index as u32), global))
    }

    pub fn global_defs(&self) -> impl Iterator<Item = (GlobalId, &GlobalDecl, &GlobalExpr)> {
        self.global_decls
            .iter()
            .zip(&self.global_defs)
            .enumerate()
            .filter_map(|(index, (decl, value))| {
                value
                    .as_ref()
                    .map(|value| (GlobalId(index as u32), decl, value))
            })
    }

    pub fn check(&self, types: &TypeStore) -> Result<(), CoreError> {
        for (_, global) in self.global_decls() {
            global.check(types)?;
        }
        for (_, global, value) in self.global_defs() {
            let actual = value.check(types, self)?;
            if actual != global.ty {
                return Err(CoreError::TypeMismatch {
                    expected: global.ty,
                    actual,
                });
            }
        }
        for (_, function) in self.functions() {
            function.check(types, self)?;
        }
        Ok(())
    }

    fn name_exists(&self, name: &str) -> bool {
        self.function_names.contains_key(name) || self.global_names.contains_key(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalDecl {
    pub name: String,
    pub ty: TypeId,
}

impl GlobalDecl {
    pub fn new(name: impl Into<String>, ty: TypeId) -> Self {
        Self {
            name: name.into(),
            ty,
        }
    }

    pub fn check(&self, types: &TypeStore) -> Result<(), CoreError> {
        if types.get(self.ty).is_some() {
            Ok(())
        } else {
            Err(CoreError::UnknownType(self.ty))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalDef {
    pub name: String,
    pub ty: TypeId,
    pub value: GlobalExpr,
}

impl GlobalDef {
    pub fn new(name: impl Into<String>, ty: TypeId, value: GlobalExpr) -> Self {
        Self {
            name: name.into(),
            ty,
            value,
        }
    }
}

/// A literal value tree for a global definition. Globals cannot reference
/// other globals, so definitions are acyclic by construction; if that changes
/// the checker must add cycle detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalExpr {
    Unit,
    FiniteLiteral {
        ty: TypeId,
        value: u128,
    },
    SymbolLiteral {
        ty: TypeId,
        value: String,
    },
    TextLiteral {
        ty: TypeId,
        value: String,
    },
    FunctionRef {
        ty: TypeId,
        function: FunctionId,
    },
    Product {
        ty: TypeId,
        fields: Vec<GlobalExpr>,
    },
    SumInject {
        ty: TypeId,
        variant: usize,
        payload: Box<GlobalExpr>,
    },
}

impl GlobalExpr {
    pub fn infer_type(&self, types: &TypeStore) -> Result<TypeId, CoreError> {
        self.check_inner(types, None)
    }

    pub fn check(&self, types: &TypeStore, program: &CoreProgram) -> Result<TypeId, CoreError> {
        self.check_inner(types, Some(program))
    }

    fn check_inner(
        &self,
        types: &TypeStore,
        program: Option<&CoreProgram>,
    ) -> Result<TypeId, CoreError> {
        match self {
            Self::Unit => Ok(types.unit()),
            Self::FiniteLiteral { ty, value } => {
                let values = finite_cardinality(types, *ty)?;
                if *value >= values {
                    return Err(CoreError::FiniteLiteralOutOfRange {
                        ty: *ty,
                        value: *value,
                        values,
                    });
                }
                Ok(*ty)
            }
            Self::SymbolLiteral { ty, .. } => match type_kind(types, *ty)? {
                TypeKind::Symbol => Ok(*ty),
                _ => Err(CoreError::NotSymbol(*ty)),
            },
            Self::TextLiteral { ty, .. } => match type_kind(types, *ty)? {
                TypeKind::Text => Ok(*ty),
                _ => Err(CoreError::NotText(*ty)),
            },
            Self::FunctionRef { ty, function } => {
                match type_kind(types, *ty)? {
                    TypeKind::Function { .. } => {}
                    _ => return Err(CoreError::NotFunction(*ty)),
                }
                if let Some(program) = program {
                    check_function_value_type(types, program, *function, *ty)?;
                }
                Ok(*ty)
            }
            Self::Product { ty, fields } => {
                let expected_fields = match type_kind(types, *ty)? {
                    TypeKind::Product(expected_fields) => expected_fields.clone(),
                    _ => return Err(CoreError::NotProduct(*ty)),
                };
                if fields.len() != expected_fields.len() {
                    return Err(CoreError::ResultArity {
                        expected: expected_fields.len(),
                        actual: fields.len(),
                    });
                }
                for (field, expected) in fields.iter().zip(expected_fields) {
                    let actual = field.check_inner(types, program)?;
                    if actual != expected.ty {
                        return Err(CoreError::TypeMismatch {
                            expected: expected.ty,
                            actual,
                        });
                    }
                }
                Ok(*ty)
            }
            Self::SumInject {
                ty,
                variant,
                payload,
            } => {
                let expected_payload = match type_kind(types, *ty)? {
                    TypeKind::Sum(variants) => {
                        variants
                            .get(*variant)
                            .ok_or(CoreError::BadVariant {
                                ty: *ty,
                                variant: *variant,
                            })?
                            .ty
                    }
                    _ => return Err(CoreError::NotSum(*ty)),
                };
                let actual_payload = payload.check_inner(types, program)?;
                if actual_payload != expected_payload {
                    return Err(CoreError::TypeMismatch {
                        expected: expected_payload,
                        actual: actual_payload,
                    });
                }
                Ok(*ty)
            }
        }
    }
}

/// A core function: typed input ids, output types, a straight-line body of
/// single-assignment statements, and the ids it returns.
///
/// The frontend threading convention (borrowed/`mut` parameters returned
/// before the visible result) is invisible here: outputs are just a list.
/// `flow.rs` relates outputs back to inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: Option<String>,
    pub inputs: Vec<Param>,
    pub outputs: Vec<TypeId>,
    pub body: Vec<Statement>,
    pub returns: Vec<ValueId>,
}

impl Function {
    pub fn check(&self, types: &TypeStore, program: &CoreProgram) -> Result<(), CoreError> {
        let mut checker = FunctionChecker::new(types, program);
        checker.check_function(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub id: ValueId,
    pub ty: TypeId,
}

impl Param {
    pub fn new(id: ValueId, ty: TypeId) -> Self {
        Self { id, ty }
    }
}

/// One single-assignment step: evaluate `expr`, bind its results (in order)
/// to the fresh ids in `results`. An expression with no results (like `Zap`)
/// has an empty result list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub results: Vec<ValueId>,
    pub expr: Expr,
}

impl Statement {
    pub fn new(results: Vec<ValueId>, expr: Expr) -> Self {
        Self { results, expr }
    }
}

/// One arm of a [`Expr::Match`]. The arm's block starts with the variant
/// payload bound to `payload` plus every value live before the match (arms
/// are control-flow joins: the match consumes the entire environment, and
/// each arm must account for all of it). All arms must return the same types,
/// which become the match's results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArm {
    pub variant: usize,
    pub payload: ValueId,
    pub body: Vec<Statement>,
    pub returns: Vec<ValueId>,
}

impl MatchArm {
    pub fn new(
        variant: usize,
        payload: ValueId,
        body: Vec<Statement>,
        returns: Vec<ValueId>,
    ) -> Self {
        Self {
            variant,
            payload,
            body,
            returns,
        }
    }
}

/// A core expression. Every id an expression mentions is consumed; the
/// comment on each variant gives its result shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// The unit value. Results: `(unit)`.
    Unit,
    /// A constant of a finite type; `value` must be below the cardinality.
    /// Results: `(ty)`.
    FiniteLiteral { ty: TypeId, value: u128 },
    /// A static function as a value, at function type `ty`; the function's
    /// packed inputs/outputs must match `ty` (see module doc). Results:
    /// `(ty)`.
    FunctionRef { ty: TypeId, function: FunctionId },
    /// Construct a product from all of its fields, in order. Results: `(ty)`.
    Product { ty: TypeId, fields: Vec<ValueId> },
    /// Deconstruct a product into all of its fields. Results: one per field.
    SplitProduct { value: ValueId },
    /// Take one field out of a product. `context_ty` is the one-hole context:
    /// the product of the remaining fields, names and order preserved.
    /// Results: `(field type, context_ty)`.
    FocusField {
        value: ValueId,
        field: usize,
        context_ty: TypeId,
    },
    /// Put a field back into a one-hole context, rebuilding product `ty`.
    /// Inverse of [`Expr::FocusField`]. Results: `(ty)`.
    PlugField {
        ty: TypeId,
        field: usize,
        part: ValueId,
        context: ValueId,
    },
    /// Construct a sum by injecting a payload at `variant`. Results: `(ty)`.
    SumInject {
        ty: TypeId,
        variant: usize,
        payload: ValueId,
    },
    /// Deconstruct a sum. Exactly one arm per variant; the match consumes the
    /// scrutinee and every live value (see [`MatchArm`]). Results: whatever
    /// the arms agree on returning.
    Match {
        scrutinee: ValueId,
        arms: Vec<MatchArm>,
    },
    /// Reference a global. This mints a fresh linear local of the global's
    /// type without consuming anything; the global itself is a static name,
    /// not a runtime resource. Results: `(global's type)`.
    Global { global: GlobalId },
    /// Call a known function. Results: the callee's output types.
    Call {
        function: FunctionId,
        args: Vec<ValueId>,
    },
    /// Call through a function value, with the packing convention: `arg` is
    /// the packed input, the result is the packed output. Results: `(B)` for
    /// a function value of type `A -> B`.
    CallValue { function: ValueId, arg: ValueId },
    /// A primitive operation; see [`BuiltinOp`] for each result shape.
    Builtin { op: BuiltinOp, args: Vec<ValueId> },
    /// Explicitly duplicate a value; requires the `Dup` capability. Both
    /// results are the same version of the input. Results: `(T, T)`.
    Dup { value: ValueId },
    /// Explicitly drop a value; requires the `Zap` capability. Results: none.
    Zap { value: ValueId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreError {
    Type(TypeError),
    EmptyName,
    DuplicateFunctionName(String),
    DuplicateGlobalName(String),
    UnknownFunction(FunctionId),
    UnknownGlobal(GlobalId),
    UnknownType(TypeId),
    DuplicateValue(ValueId),
    UnknownValue(ValueId),
    ConsumedValue(ValueId),
    LiveValueAtEnd(ValueId),
    ResultArity {
        expected: usize,
        actual: usize,
    },
    ReturnArity {
        expected: usize,
        actual: usize,
    },
    TypeMismatch {
        expected: TypeId,
        actual: TypeId,
    },
    NotFinite(TypeId),
    FiniteLiteralOutOfRange {
        ty: TypeId,
        value: u128,
        values: u128,
    },
    NotProduct(TypeId),
    BadField {
        ty: TypeId,
        field: usize,
    },
    BadProductResidual {
        product: TypeId,
        field: usize,
        context: TypeId,
    },
    NotSum(TypeId),
    NotSymbol(TypeId),
    NotText(TypeId),
    NotFunction(TypeId),
    BadVariant {
        ty: TypeId,
        variant: usize,
    },
    DuplicateMatchArm {
        ty: TypeId,
        variant: usize,
    },
    MissingMatchArm {
        ty: TypeId,
        variant: usize,
    },
    CannotDup(TypeId),
    CannotZap(TypeId),
    FunctionTypeMismatch {
        function: FunctionId,
        ty: TypeId,
    },
}

/// Primitive operations. The arithmetic/comparison ops are observer-style:
/// they consume both operands and return them *unchanged* (same version)
/// before the fresh visible result, so reading a value never forces a `dup`.
///
/// Result shapes:
///
/// ```text
/// add/sub/mul (lhs, rhs) -> (lhs, rhs, result)     modular over cardinality
/// eq/lt       (lhs, rhs) -> (lhs, rhs, bool)       bool_ty: any 2-unit sum
/// next        (x)        -> (x')                   changed version, +1 mod N
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuiltinOp {
    FiniteAdd {
        ty: TypeId,
    },
    FiniteSub {
        ty: TypeId,
    },
    FiniteMul {
        ty: TypeId,
    },
    FiniteEq {
        ty: TypeId,
        bool_ty: TypeId,
    },
    FiniteLt {
        ty: TypeId,
        bool_ty: TypeId,
    },
    /// Toy update builtin: exists so value-flow checking has an axiomatic
    /// "output is a changed version" primitive to recurse to until real
    /// update builtins (collections, handles) land.
    FiniteNext {
        ty: TypeId,
    },
}

impl From<TypeError> for CoreError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

/// One tracked value during checking: its type, and whether it is still
/// live (defined but not yet consumed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    ty: TypeId,
    live: bool,
}

/// The linearity/type checker for one function. A single forward pass:
/// `define` adds a live slot, `consume` kills it, and at every block end all
/// slots must be dead. Match arms are checked as fresh blocks whose inputs
/// are the payload plus everything live at the match (which the match itself
/// consumes).
struct FunctionChecker<'a> {
    types: &'a TypeStore,
    program: &'a CoreProgram,
    values: HashMap<ValueId, Slot>,
}

impl<'a> FunctionChecker<'a> {
    fn new(types: &'a TypeStore, program: &'a CoreProgram) -> Self {
        Self {
            types,
            program,
            values: HashMap::new(),
        }
    }

    fn check_function(&mut self, function: &Function) -> Result<(), CoreError> {
        for output in &function.outputs {
            self.validate_type(*output)?;
        }
        let returned = self.check_block(&function.inputs, &function.body, &function.returns)?;
        if returned.len() != function.outputs.len() {
            return Err(CoreError::ReturnArity {
                expected: function.outputs.len(),
                actual: returned.len(),
            });
        }
        for (actual, expected) in returned.into_iter().zip(function.outputs.iter().copied()) {
            if actual != expected {
                return Err(CoreError::TypeMismatch { expected, actual });
            }
        }
        Ok(())
    }

    fn check_block(
        &mut self,
        inputs: &[Param],
        body: &[Statement],
        returns: &[ValueId],
    ) -> Result<Vec<TypeId>, CoreError> {
        for input in inputs {
            self.validate_type(input.ty)?;
            self.define(input.id, input.ty)?;
        }
        for statement in body {
            let result_tys = self.infer_expr(&statement.expr)?;
            if statement.results.len() != result_tys.len() {
                return Err(CoreError::ResultArity {
                    expected: result_tys.len(),
                    actual: statement.results.len(),
                });
            }
            for (id, ty) in statement.results.iter().copied().zip(result_tys) {
                self.define(id, ty)?;
            }
        }

        let returned = returns
            .iter()
            .copied()
            .map(|value| self.consume(value))
            .collect::<Result<Vec<_>, _>>()?;
        for (id, slot) in &self.values {
            if slot.live {
                return Err(CoreError::LiveValueAtEnd(*id));
            }
        }
        Ok(returned)
    }

    fn infer_expr(&mut self, expr: &Expr) -> Result<Vec<TypeId>, CoreError> {
        match expr {
            Expr::Unit => Ok(vec![self.types.unit()]),
            Expr::FiniteLiteral { ty, value } => {
                let values = finite_cardinality(self.types, *ty)?;
                if *value >= values {
                    return Err(CoreError::FiniteLiteralOutOfRange {
                        ty: *ty,
                        value: *value,
                        values,
                    });
                }
                Ok(vec![*ty])
            }
            Expr::FunctionRef { ty, function } => {
                self.check_function_value_type(*function, *ty)?;
                Ok(vec![*ty])
            }
            Expr::Product { ty, fields } => {
                let expected_fields = match type_kind(self.types, *ty)? {
                    TypeKind::Product(expected_fields) => expected_fields.clone(),
                    _ => return Err(CoreError::NotProduct(*ty)),
                };
                if fields.len() != expected_fields.len() {
                    return Err(CoreError::ResultArity {
                        expected: expected_fields.len(),
                        actual: fields.len(),
                    });
                }
                for (field, expected) in fields.iter().copied().zip(expected_fields) {
                    let actual = self.consume(field)?;
                    if actual != expected.ty {
                        return Err(CoreError::TypeMismatch {
                            expected: expected.ty,
                            actual,
                        });
                    }
                }
                Ok(vec![*ty])
            }
            Expr::SplitProduct { value } => {
                let ty = self.consume(*value)?;
                match type_kind(self.types, ty)? {
                    TypeKind::Product(fields) => Ok(fields.iter().map(|field| field.ty).collect()),
                    _ => Err(CoreError::NotProduct(ty)),
                }
            }
            Expr::FocusField {
                value,
                field,
                context_ty,
            } => {
                let product_ty = self.consume(*value)?;
                let field_ty = check_product_residual(self.types, product_ty, *field, *context_ty)?;
                Ok(vec![field_ty, *context_ty])
            }
            Expr::PlugField {
                ty,
                field,
                part,
                context,
            } => {
                let expected_field_ty =
                    check_product_residual(self.types, *ty, *field, self.value_type(*context)?)?;
                let actual_field_ty = self.consume(*part)?;
                if actual_field_ty != expected_field_ty {
                    return Err(CoreError::TypeMismatch {
                        expected: expected_field_ty,
                        actual: actual_field_ty,
                    });
                }
                self.consume(*context)?;
                Ok(vec![*ty])
            }
            Expr::SumInject {
                ty,
                variant,
                payload,
            } => {
                let expected_payload = match type_kind(self.types, *ty)? {
                    TypeKind::Sum(variants) => {
                        variants
                            .get(*variant)
                            .ok_or(CoreError::BadVariant {
                                ty: *ty,
                                variant: *variant,
                            })?
                            .ty
                    }
                    _ => return Err(CoreError::NotSum(*ty)),
                };
                let actual_payload = self.consume(*payload)?;
                if actual_payload != expected_payload {
                    return Err(CoreError::TypeMismatch {
                        expected: expected_payload,
                        actual: actual_payload,
                    });
                }
                Ok(vec![*ty])
            }
            Expr::Match { scrutinee, arms } => {
                let ty = self.consume(*scrutinee)?;
                let variants = match type_kind(self.types, ty)? {
                    TypeKind::Sum(variants) => variants.clone(),
                    _ => return Err(CoreError::NotSum(ty)),
                };
                let captured = self.capture_live_values()?;
                let mut seen = vec![false; variants.len()];
                let mut result_tys = None;

                for arm in arms {
                    let variant = variants.get(arm.variant).ok_or(CoreError::BadVariant {
                        ty,
                        variant: arm.variant,
                    })?;
                    if seen[arm.variant] {
                        return Err(CoreError::DuplicateMatchArm {
                            ty,
                            variant: arm.variant,
                        });
                    }
                    seen[arm.variant] = true;

                    let mut arm_checker = FunctionChecker::new(self.types, self.program);
                    let mut inputs = Vec::with_capacity(1 + captured.len());
                    inputs.push(Param::new(arm.payload, variant.ty));
                    inputs.extend(captured.iter().map(|(id, ty)| Param::new(*id, *ty)));
                    let arm_result_tys =
                        arm_checker.check_block(&inputs, &arm.body, &arm.returns)?;

                    match &result_tys {
                        None => result_tys = Some(arm_result_tys),
                        Some(expected) => {
                            if arm_result_tys.len() != expected.len() {
                                return Err(CoreError::ResultArity {
                                    expected: expected.len(),
                                    actual: arm_result_tys.len(),
                                });
                            }
                            for (actual, expected) in
                                arm_result_tys.into_iter().zip(expected.iter().copied())
                            {
                                if actual != expected {
                                    return Err(CoreError::TypeMismatch { expected, actual });
                                }
                            }
                        }
                    }
                }

                for (variant, was_seen) in seen.into_iter().enumerate() {
                    if !was_seen {
                        return Err(CoreError::MissingMatchArm { ty, variant });
                    }
                }

                Ok(result_tys.unwrap_or_default())
            }
            Expr::Global { global } => {
                let global = self
                    .program
                    .get_global_decl(*global)
                    .ok_or(CoreError::UnknownGlobal(*global))?;
                self.validate_type(global.ty)?;
                Ok(vec![global.ty])
            }
            Expr::Call { function, args } => {
                self.infer_known_call(*function, self.program.get(*function), args)
            }
            Expr::CallValue { function, arg } => {
                let function_ty = self.consume(*function)?;
                let (input, output) = function_parts(self.types, function_ty)?;
                let actual = self.consume(*arg)?;
                if actual != input {
                    return Err(CoreError::TypeMismatch {
                        expected: input,
                        actual,
                    });
                }
                Ok(vec![output])
            }
            Expr::Builtin { op, args } => self.infer_builtin(op, args),
            Expr::Dup { value } => {
                let ty = self.consume(*value)?;
                if !self.types.can_dup(ty)? {
                    return Err(CoreError::CannotDup(ty));
                }
                Ok(vec![ty, ty])
            }
            Expr::Zap { value } => {
                let ty = self.consume(*value)?;
                if !self.types.can_zap(ty)? {
                    return Err(CoreError::CannotZap(ty));
                }
                Ok(vec![])
            }
        }
    }

    fn capture_live_values(&mut self) -> Result<Vec<(ValueId, TypeId)>, CoreError> {
        let captured = self
            .values
            .iter()
            .filter_map(|(id, slot)| slot.live.then_some((*id, slot.ty)))
            .collect::<Vec<_>>();
        for (id, _) in &captured {
            self.consume(*id)?;
        }
        Ok(captured)
    }

    fn infer_builtin(
        &mut self,
        op: &BuiltinOp,
        args: &[ValueId],
    ) -> Result<Vec<TypeId>, CoreError> {
        match op {
            BuiltinOp::FiniteAdd { ty }
            | BuiltinOp::FiniteSub { ty }
            | BuiltinOp::FiniteMul { ty } => {
                finite_cardinality(self.types, *ty)?;
                self.consume_args(args, &[*ty, *ty])?;
                Ok(vec![*ty, *ty, *ty])
            }
            BuiltinOp::FiniteEq { ty, bool_ty } | BuiltinOp::FiniteLt { ty, bool_ty } => {
                finite_cardinality(self.types, *ty)?;
                validate_bool_type(self.types, *bool_ty)?;
                self.consume_args(args, &[*ty, *ty])?;
                Ok(vec![*ty, *ty, *bool_ty])
            }
            BuiltinOp::FiniteNext { ty } => {
                finite_cardinality(self.types, *ty)?;
                self.consume_args(args, &[*ty])?;
                Ok(vec![*ty])
            }
        }
    }

    fn infer_known_call(
        &mut self,
        function: FunctionId,
        callee: Option<&Function>,
        args: &[ValueId],
    ) -> Result<Vec<TypeId>, CoreError> {
        let callee = callee.ok_or(CoreError::UnknownFunction(function))?;
        if args.len() != callee.inputs.len() {
            return Err(CoreError::ResultArity {
                expected: callee.inputs.len(),
                actual: args.len(),
            });
        }
        for (arg, input) in args.iter().copied().zip(&callee.inputs) {
            let actual = self.consume(arg)?;
            if actual != input.ty {
                return Err(CoreError::TypeMismatch {
                    expected: input.ty,
                    actual,
                });
            }
        }
        Ok(callee.outputs.clone())
    }

    fn check_function_value_type(
        &self,
        function: FunctionId,
        function_ty: TypeId,
    ) -> Result<(), CoreError> {
        check_function_value_type(self.types, self.program, function, function_ty)
    }

    fn consume_args(&mut self, args: &[ValueId], expected: &[TypeId]) -> Result<(), CoreError> {
        if args.len() != expected.len() {
            return Err(CoreError::ResultArity {
                expected: expected.len(),
                actual: args.len(),
            });
        }
        for (arg, expected) in args.iter().copied().zip(expected.iter().copied()) {
            let actual = self.consume(arg)?;
            if actual != expected {
                return Err(CoreError::TypeMismatch { expected, actual });
            }
        }
        Ok(())
    }

    fn define(&mut self, id: ValueId, ty: TypeId) -> Result<(), CoreError> {
        self.validate_type(ty)?;
        if self.values.contains_key(&id) {
            return Err(CoreError::DuplicateValue(id));
        }
        self.values.insert(id, Slot { ty, live: true });
        Ok(())
    }

    fn consume(&mut self, id: ValueId) -> Result<TypeId, CoreError> {
        let slot = self
            .values
            .get_mut(&id)
            .ok_or(CoreError::UnknownValue(id))?;
        if !slot.live {
            return Err(CoreError::ConsumedValue(id));
        }
        slot.live = false;
        Ok(slot.ty)
    }

    fn value_type(&self, id: ValueId) -> Result<TypeId, CoreError> {
        let slot = self.values.get(&id).ok_or(CoreError::UnknownValue(id))?;
        if !slot.live {
            return Err(CoreError::ConsumedValue(id));
        }
        Ok(slot.ty)
    }

    fn validate_type(&self, id: TypeId) -> Result<(), CoreError> {
        if self.types.get(id).is_some() {
            Ok(())
        } else {
            Err(CoreError::UnknownType(id))
        }
    }
}

fn finite_cardinality(types: &TypeStore, id: TypeId) -> Result<u128, CoreError> {
    match type_kind(types, id)? {
        TypeKind::Finite { values } => Ok(*values),
        _ => Err(CoreError::NotFinite(id)),
    }
}

fn validate_bool_type(types: &TypeStore, id: TypeId) -> Result<(), CoreError> {
    match type_kind(types, id)? {
        TypeKind::Sum(variants) if variants.len() == 2 => {
            for variant in variants {
                if variant.ty != types.unit() {
                    return Err(CoreError::NotSum(id));
                }
            }
            Ok(())
        }
        _ => Err(CoreError::NotSum(id)),
    }
}

fn function_parts(types: &TypeStore, id: TypeId) -> Result<(TypeId, TypeId), CoreError> {
    match type_kind(types, id)? {
        TypeKind::Function { input, output } => Ok((*input, *output)),
        _ => Err(CoreError::NotFunction(id)),
    }
}

fn check_function_value_type(
    types: &TypeStore,
    program: &CoreProgram,
    function: FunctionId,
    function_ty: TypeId,
) -> Result<(), CoreError> {
    let (input, output) = function_parts(types, function_ty)?;
    let callee = program
        .get(function)
        .ok_or(CoreError::UnknownFunction(function))?;
    let input_parts = callee
        .inputs
        .iter()
        .map(|input| input.ty)
        .collect::<Vec<_>>();
    if !packed_signature_matches(types, input, &input_parts)?
        || !packed_signature_matches(types, output, &callee.outputs)?
    {
        return Err(CoreError::FunctionTypeMismatch {
            function,
            ty: function_ty,
        });
    }
    Ok(())
}

fn packed_signature_matches(
    types: &TypeStore,
    packed_ty: TypeId,
    parts: &[TypeId],
) -> Result<bool, CoreError> {
    match parts {
        [] => Ok(packed_ty == types.unit()),
        [part] => Ok(packed_ty == *part),
        parts => match type_kind(types, packed_ty)? {
            TypeKind::Product(fields) => Ok(fields.len() == parts.len()
                && fields
                    .iter()
                    .zip(parts.iter().copied())
                    .all(|(field, part)| field.ty == part)),
            _ => Ok(false),
        },
    }
}

fn check_product_residual(
    types: &TypeStore,
    product: TypeId,
    field: usize,
    context: TypeId,
) -> Result<TypeId, CoreError> {
    let product_fields = match type_kind(types, product)? {
        TypeKind::Product(fields) => fields,
        _ => return Err(CoreError::NotProduct(product)),
    };
    let selected = product_fields
        .get(field)
        .ok_or(CoreError::BadField { ty: product, field })?;
    let residual_fields = match type_kind(types, context)? {
        TypeKind::Product(fields) => fields,
        _ => return Err(CoreError::NotProduct(context)),
    };

    if residual_fields.len() + 1 != product_fields.len() {
        return Err(CoreError::BadProductResidual {
            product,
            field,
            context,
        });
    }

    for (expected, actual) in product_fields
        .iter()
        .enumerate()
        .filter_map(|(index, component)| (index != field).then_some(component))
        .zip(residual_fields)
    {
        if expected != actual {
            return Err(CoreError::BadProductResidual {
                product,
                field,
                context,
            });
        }
    }

    Ok(selected.ty)
}

fn type_kind(types: &TypeStore, id: TypeId) -> Result<&TypeKind, CoreError> {
    types
        .get(id)
        .map(|def| &def.kind)
        .ok_or(CoreError::UnknownType(id))
}

fn validate_name(name: &str) -> Result<(), CoreError> {
    if name.is_empty() {
        Err(CoreError::EmptyName)
    } else {
        Ok(())
    }
}
