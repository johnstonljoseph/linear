use std::collections::HashMap;

use crate::TypeId;
use crate::core::{
    BuiltinOp, CoreError, CoreProgram, Expr as CoreExpr, Function, GlobalDecl,
    MatchArm as CoreMatchArm, Param as CoreParam, Statement,
};
use crate::frontend::{
    Arg, BinaryOp, Block, Expr, Field, FunctionDef, GlobalDef as FrontendGlobalDef, Item, LetStmt,
    MatchArm as FrontendMatchArm, Module, Param as FrontendParam, Pattern, Stmt, TypeExpr,
    ValueFlow,
};
use crate::id::{FunctionId, GlobalId, ValueId};
use crate::types::{
    CollectionMutability, Component, ComponentName, DeclaredCapabilities, TypeError, TypeKind,
    TypeStore,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredTypes {
    pub types: TypeStore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredModule {
    pub types: TypeStore,
    pub program: CoreProgram,
    pub globals: Vec<LoweredGlobal>,
    pub functions: Vec<LoweredFunction>,
    pub methods: Vec<LoweredMethod>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredGlobal {
    pub id: GlobalId,
    pub name: String,
    pub ty: TypeId,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredFunction {
    pub id: FunctionId,
    pub name: String,
    pub params: Vec<LoweredParam>,
    pub output: TypeId,
    pub core_outputs: Vec<TypeId>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredMethod {
    pub owner: TypeId,
    pub method: String,
    pub function: LoweredFunction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredParam {
    pub id: ValueId,
    pub flow: ValueFlow,
    pub name: String,
    pub ty: TypeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LowerError {
    Type(TypeError),
    Core(CoreError),
    UnsupportedGenericDecl {
        name: String,
    },
    UnknownType(String),
    BadGenericArity {
        name: String,
        expected: usize,
        actual: usize,
    },
    UnknownCapability(String),
    UnsupportedAliasCapabilities {
        name: String,
    },
    ExpectedStructBody {
        name: String,
    },
    ExpectedEnumBody {
        name: String,
    },
    UnsupportedAnonymousImplTarget,
    UnsupportedExpression(&'static str),
    UnknownValue(String),
    DuplicateValue(String),
    TypeMismatch {
        expected: TypeId,
        actual: TypeId,
    },
    FunctionOutputArity {
        name: String,
        expected: usize,
        actual: usize,
    },
    MissingResult {
        expected: TypeId,
    },
    FlowMismatch {
        expected: ValueFlow,
        actual: ValueFlow,
    },
    ExpectedNameForReturnedArgument,
    DuplicateLinearUse(String),
    ValueMovedDuringExpression(String),
    DeadLinearLocal {
        name: String,
        ty: TypeId,
    },
}

pub fn lower_type_items(module: &Module) -> Result<LoweredTypes, LowerError> {
    let mut lowerer = TypeLowerer::new_with_standard_types()?;
    lowerer.lower_module(module)?;
    Ok(LoweredTypes {
        types: lowerer.types,
    })
}

pub fn lower_module_signatures(module: &Module) -> Result<LoweredModule, LowerError> {
    let mut lowerer = TypeLowerer::new_with_standard_types()?;
    lowerer.lower_module(module)?;
    lowerer.lower_signatures(module)
}

pub fn lower_module_bodies(module: &Module) -> Result<LoweredModule, LowerError> {
    let mut lowered = lower_module_signatures(module)?;
    let call_signatures = build_call_signatures(&lowered);

    for function in lowered.functions.clone() {
        let expected_output = visible_output(&lowered.types, function.output);
        let (body, returns) = BodyLowerer::new(
            &lowered.types,
            &lowered.program,
            &call_signatures,
            &function.params,
        )
        .lower_block(&function.body, expected_output)?;
        lowered
            .program
            .replace_function_body(function.id, body, returns)?;
    }

    for method in lowered.methods.clone() {
        let function = method.function;
        let expected_output = visible_output(&lowered.types, function.output);
        let (body, returns) = BodyLowerer::new(
            &lowered.types,
            &lowered.program,
            &call_signatures,
            &function.params,
        )
        .lower_block(&function.body, expected_output)?;
        lowered
            .program
            .replace_function_body(function.id, body, returns)?;
    }

    lowered.program.check(&lowered.types)?;
    Ok(lowered)
}

struct TypeLowerer {
    types: TypeStore,
    anonymous: HashMap<AnonymousTypeKey, TypeId>,
}

impl TypeLowerer {
    fn new_with_standard_types() -> Result<Self, LowerError> {
        let mut types = TypeStore::new();
        types.add_uint("U8", 8)?;
        types.add_uint("U16", 16)?;
        types.add_uint("U32", 32)?;
        types.add_uint("U64", 64)?;
        types.add_symbol("Symbol")?;
        types.add_text("Text")?;

        let unit = types.unit();
        types.add_sum(
            Some("Bool".into()),
            vec![
                Component::named("false", unit),
                Component::named("true", unit),
            ],
            DeclaredCapabilities::linear(),
        )?;

        Ok(Self {
            types,
            anonymous: HashMap::new(),
        })
    }

    fn lower_module(&mut self, module: &Module) -> Result<(), LowerError> {
        for item in &module.items {
            match item {
                Item::Type(type_def) => {
                    reject_generics(&type_def.name, &type_def.generics)?;
                    reject_alias_capabilities(&type_def.name, &type_def.capabilities)?;
                    let ty = self.lower_type_expr(&type_def.ty)?;
                    self.types.add_alias(type_def.name.clone(), ty)?;
                }
                Item::Struct(type_def) => {
                    reject_generics(&type_def.name, &type_def.generics)?;
                    let declared = declared_capabilities(&type_def.capabilities)?;
                    let fields = match &type_def.ty {
                        TypeExpr::Product(fields) => self.lower_components(fields)?,
                        TypeExpr::Unit => Vec::new(),
                        _ => {
                            return Err(LowerError::ExpectedStructBody {
                                name: type_def.name.clone(),
                            });
                        }
                    };
                    self.types
                        .add_product(Some(type_def.name.clone()), fields, declared)?;
                }
                Item::Enum(type_def) => {
                    reject_generics(&type_def.name, &type_def.generics)?;
                    let declared = declared_capabilities(&type_def.capabilities)?;
                    let variants = match &type_def.ty {
                        TypeExpr::Sum(variants) => self.lower_components(variants)?,
                        _ => {
                            return Err(LowerError::ExpectedEnumBody {
                                name: type_def.name.clone(),
                            });
                        }
                    };
                    self.types
                        .add_sum(Some(type_def.name.clone()), variants, declared)?;
                }
                Item::Global(_) | Item::Function(_) | Item::Impl(_) | Item::Trait(_) => {}
            }
        }
        Ok(())
    }

    fn lower_signatures(mut self, module: &Module) -> Result<LoweredModule, LowerError> {
        let mut program = CoreProgram::new();
        let mut globals = Vec::new();
        let mut functions = Vec::new();
        let mut methods = Vec::new();

        for item in &module.items {
            match item {
                Item::Global(global) => {
                    let lowered = self.lower_global_signature(&mut program, global)?;
                    globals.push(lowered);
                }
                Item::Function(function) => {
                    let lowered =
                        self.lower_function_signature(&mut program, function, &function.name)?;
                    functions.push(lowered);
                }
                Item::Impl(impl_block) => {
                    reject_generics("impl", &impl_block.generics)?;
                    let owner = self.lower_type_expr(&impl_block.target)?;
                    let trait_name = impl_block
                        .trait_ref
                        .as_ref()
                        .map(type_expr_core_name)
                        .transpose()?;
                    for method in &impl_block.methods {
                        let core_name = match &trait_name {
                            Some(trait_name) => {
                                format!(
                                    "{}.{}.{}",
                                    type_expr_core_name(&impl_block.target)?,
                                    trait_name,
                                    method.name
                                )
                            }
                            None => {
                                format!(
                                    "{}.{}",
                                    type_expr_core_name(&impl_block.target)?,
                                    method.name
                                )
                            }
                        };
                        let function =
                            self.lower_function_signature(&mut program, method, &core_name)?;
                        methods.push(LoweredMethod {
                            owner,
                            method: method.name.clone(),
                            function,
                        });
                    }
                }
                Item::Type(_) | Item::Struct(_) | Item::Enum(_) | Item::Trait(_) => {}
            }
        }

        Ok(LoweredModule {
            types: self.types,
            program,
            globals,
            functions,
            methods,
        })
    }

    fn lower_global_signature(
        &mut self,
        program: &mut CoreProgram,
        global: &FrontendGlobalDef,
    ) -> Result<LoweredGlobal, LowerError> {
        let ty = self.lower_type_expr(&global.ty)?;
        let id = program.add_global_decl(GlobalDecl::new(global.name.clone(), ty))?;
        Ok(LoweredGlobal {
            id,
            name: global.name.clone(),
            ty,
            value: global.value.clone(),
        })
    }

    fn lower_function_signature(
        &mut self,
        program: &mut CoreProgram,
        function: &FunctionDef,
        core_name: &str,
    ) -> Result<LoweredFunction, LowerError> {
        reject_generics(&function.name, &function.generics)?;
        let params = self.lower_params(&function.params)?;
        let output = self.lower_type_expr(&function.output)?;
        let core_outputs = core_output_types(self.types.unit(), &params, output);
        let id = program.add_function(Function {
            name: Some(core_name.to_owned()),
            inputs: params
                .iter()
                .map(|param| CoreParam::new(param.id, param.ty))
                .collect(),
            outputs: core_outputs.clone(),
            body: Vec::<Statement>::new(),
            returns: Vec::new(),
        })?;
        Ok(LoweredFunction {
            id,
            name: core_name.to_owned(),
            params,
            output,
            core_outputs,
            body: function.body.clone(),
        })
    }

    fn lower_params(&mut self, params: &[FrontendParam]) -> Result<Vec<LoweredParam>, LowerError> {
        params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                Ok(LoweredParam {
                    id: ValueId(index as u32),
                    flow: param.flow,
                    name: param.name.clone(),
                    ty: self.lower_type_expr(&param.ty)?,
                })
            })
            .collect()
    }

    fn lower_type_expr(&mut self, ty: &TypeExpr) -> Result<TypeId, LowerError> {
        match ty {
            TypeExpr::Unit => Ok(self.types.unit()),
            TypeExpr::Name(name) => self
                .types
                .type_id(name)
                .ok_or_else(|| LowerError::UnknownType(name.clone())),
            TypeExpr::Apply { name, args } => self.lower_type_apply(name, args),
            TypeExpr::Product(fields) => {
                let components = self.lower_components(fields)?;
                self.intern_anonymous(AnonymousTypeKey::Product(components))
            }
            TypeExpr::Sum(variants) => {
                let components = self.lower_components(variants)?;
                self.intern_anonymous(AnonymousTypeKey::Sum(components))
            }
            TypeExpr::Function { input, output } => {
                let input = self.lower_type_expr(input)?;
                let output = self.lower_type_expr(output)?;
                self.intern_anonymous(AnonymousTypeKey::Function { input, output })
            }
        }
    }

    fn lower_type_apply(&mut self, name: &str, args: &[TypeExpr]) -> Result<TypeId, LowerError> {
        match name {
            "List" | "Vector" | "Vec" => {
                let [element] = expect_arity(name, args, 1)?;
                let element = self.lower_type_expr(element)?;
                self.intern_anonymous(AnonymousTypeKey::List {
                    element,
                    mutability: CollectionMutability::Immutable,
                })
            }
            "MutList" | "MutableList" | "MutVector" | "MutableVector" | "MutVec" => {
                let [element] = expect_arity(name, args, 1)?;
                let element = self.lower_type_expr(element)?;
                self.intern_anonymous(AnonymousTypeKey::List {
                    element,
                    mutability: CollectionMutability::Mutable,
                })
            }
            "HashMap" => {
                let [key, value] = expect_arity(name, args, 2)?;
                let key = self.lower_type_expr(key)?;
                let value = self.lower_type_expr(value)?;
                self.intern_anonymous(AnonymousTypeKey::HashMap {
                    key,
                    value,
                    mutability: CollectionMutability::Immutable,
                })
            }
            "MutHashMap" | "MutableHashMap" => {
                let [key, value] = expect_arity(name, args, 2)?;
                let key = self.lower_type_expr(key)?;
                let value = self.lower_type_expr(value)?;
                self.intern_anonymous(AnonymousTypeKey::HashMap {
                    key,
                    value,
                    mutability: CollectionMutability::Mutable,
                })
            }
            _ => Err(LowerError::UnknownType(name.to_owned())),
        }
    }

    fn lower_components(
        &mut self,
        fields: &[Field<TypeExpr>],
    ) -> Result<Vec<Component>, LowerError> {
        fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let ty = self.lower_type_expr(&field.value)?;
                Ok(match &field.name {
                    Some(name) => Component::named(name.clone(), ty),
                    None => Component::positional(index, ty),
                })
            })
            .collect()
    }

    fn intern_anonymous(&mut self, key: AnonymousTypeKey) -> Result<TypeId, LowerError> {
        if let Some(id) = self.anonymous.get(&key) {
            return Ok(*id);
        }

        let id = match key.clone() {
            AnonymousTypeKey::Product(fields) => {
                self.types
                    .add_product(None, fields, DeclaredCapabilities::linear())?
            }
            AnonymousTypeKey::Sum(variants) => {
                self.types
                    .add_sum(None, variants, DeclaredCapabilities::linear())?
            }
            AnonymousTypeKey::Function { input, output } => {
                self.types.add_function(None, input, output)?
            }
            AnonymousTypeKey::List {
                element,
                mutability,
            } => self.types.add_list(None, element, mutability)?,
            AnonymousTypeKey::HashMap {
                key,
                value,
                mutability,
            } => self.types.add_hashmap(None, key, value, mutability)?,
        };
        self.anonymous.insert(key, id);
        Ok(id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LoweredValue {
    id: ValueId,
    ty: TypeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoweredBinaryOperand {
    name: Option<String>,
    value: LoweredValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThreadedName {
    name: String,
    ty: TypeId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReturnedCallArg {
    Rebind(String),
    Zap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CallableSignature {
    params: Vec<LoweredParam>,
    visible_output: Option<TypeId>,
}

struct BodyLowerer<'a> {
    types: &'a TypeStore,
    program: &'a CoreProgram,
    call_signatures: &'a HashMap<FunctionId, CallableSignature>,
    params: Vec<LoweredParam>,
    env: HashMap<String, LoweredValue>,
    name_order: Vec<String>,
    body: Vec<Statement>,
    next_value: u32,
}

impl<'a> BodyLowerer<'a> {
    fn new(
        types: &'a TypeStore,
        program: &'a CoreProgram,
        call_signatures: &'a HashMap<FunctionId, CallableSignature>,
        params: &[LoweredParam],
    ) -> Self {
        let env = params
            .iter()
            .map(|param| {
                (
                    param.name.clone(),
                    LoweredValue {
                        id: param.id,
                        ty: param.ty,
                    },
                )
            })
            .collect();
        let name_order = params.iter().map(|param| param.name.clone()).collect();
        Self {
            types,
            program,
            call_signatures,
            params: params.to_vec(),
            env,
            name_order,
            body: Vec::new(),
            next_value: params.len() as u32,
        }
    }

    fn lower_block(
        self,
        block: &Block,
        expected_result: Option<TypeId>,
    ) -> Result<(Vec<Statement>, Vec<ValueId>), LowerError> {
        let returned_names = self.function_return_names();
        self.lower_block_returning(block, expected_result, returned_names)
    }

    fn lower_block_returning(
        mut self,
        block: &Block,
        expected_result: Option<TypeId>,
        returned_names: Vec<ThreadedName>,
    ) -> Result<(Vec<Statement>, Vec<ValueId>), LowerError> {
        for statement in &block.statements {
            match statement {
                Stmt::Let(let_stmt) => self.lower_let_stmt(let_stmt)?,
                Stmt::Expr(expr) => self.lower_expr_stmt(expr)?,
            }
        }

        let explicit_result =
            self.lower_optional_result(block.result.as_deref(), expected_result)?;
        let mut returns = self.returned_name_values(&returned_names)?;
        if let Some(result) = explicit_result {
            returns.push(result.id);
        }
        self.zap_dead_names(&returned_names, explicit_result)?;

        Ok((self.body, returns))
    }

    fn lower_let_stmt(&mut self, let_stmt: &LetStmt) -> Result<(), LowerError> {
        let expected = let_stmt
            .ty
            .as_ref()
            .map(|ty| self.resolve_existing_type(ty))
            .transpose()?;
        let moved_name = self.local_name(&let_stmt.value);
        let value = self.lower_expr(&let_stmt.value, expected)?;
        if let Some(name) = moved_name {
            self.env.remove(&name);
        }
        if let Some(expected) = expected {
            if value.ty != expected {
                return Err(LowerError::TypeMismatch {
                    expected,
                    actual: value.ty,
                });
            }
        }
        self.bind_pattern(&let_stmt.pattern, value)
    }

    fn lower_expr_stmt(&mut self, expr: &Expr) -> Result<(), LowerError> {
        let value = self.lower_expr(expr, Some(self.types.unit()))?;
        if value.ty != self.types.unit() {
            return Err(LowerError::TypeMismatch {
                expected: self.types.unit(),
                actual: value.ty,
            });
        }
        self.push_statement(vec![], CoreExpr::Zap { value: value.id })
    }

    fn lower_optional_result(
        &mut self,
        result: Option<&Expr>,
        expected_result: Option<TypeId>,
    ) -> Result<Option<LoweredValue>, LowerError> {
        match (result, expected_result) {
            (Some(result), Some(expected)) => {
                let value = self.lower_expr(result, Some(expected))?;
                if value.ty != expected {
                    return Err(LowerError::TypeMismatch {
                        expected,
                        actual: value.ty,
                    });
                }
                Ok(Some(value))
            }
            (Some(result), None) => {
                let value = self.lower_expr(result, Some(self.types.unit()))?;
                if value.ty != self.types.unit() {
                    return Err(LowerError::TypeMismatch {
                        expected: self.types.unit(),
                        actual: value.ty,
                    });
                }
                self.push_statement(vec![], CoreExpr::Zap { value: value.id })?;
                Ok(None)
            }
            (None, Some(expected)) => Err(LowerError::MissingResult { expected }),
            (None, None) => Ok(None),
        }
    }

    fn function_return_names(&self) -> Vec<ThreadedName> {
        self.params
            .iter()
            .filter(|param| param.flow != ValueFlow::NotReturned)
            .map(|param| ThreadedName {
                name: param.name.clone(),
                ty: param.ty,
            })
            .collect()
    }

    fn returned_name_values(&self, names: &[ThreadedName]) -> Result<Vec<ValueId>, LowerError> {
        names
            .iter()
            .map(|threaded| {
                let value = self
                    .env
                    .get(&threaded.name)
                    .ok_or_else(|| LowerError::UnknownValue(threaded.name.clone()))?;
                if value.ty != threaded.ty {
                    return Err(LowerError::TypeMismatch {
                        expected: threaded.ty,
                        actual: value.ty,
                    });
                }
                Ok(value.id)
            })
            .collect()
    }

    fn live_names_except(&self, excluded: Option<&str>) -> Vec<ThreadedName> {
        self.name_order
            .iter()
            .filter(|name| excluded != Some(name.as_str()))
            .filter_map(|name| {
                self.env.get(name).map(|value| ThreadedName {
                    name: name.clone(),
                    ty: value.ty,
                })
            })
            .collect()
    }

    fn zap_dead_names(
        &mut self,
        returned_names: &[ThreadedName],
        explicit_result: Option<LoweredValue>,
    ) -> Result<(), LowerError> {
        let protected_result = explicit_result.map(|value| value.id);
        let live_names = self
            .name_order
            .iter()
            .filter(|name| {
                !returned_names
                    .iter()
                    .any(|returned| returned.name == **name)
            })
            .filter_map(|name| {
                self.env
                    .get(name)
                    .copied()
                    .map(|value| (name.clone(), value))
            })
            .collect::<Vec<_>>();

        for (name, value) in live_names {
            if Some(value.id) != protected_result {
                if !self.types.can_zap(value.ty).map_err(LowerError::Type)? {
                    return Err(LowerError::DeadLinearLocal { name, ty: value.ty });
                }
                self.push_statement(vec![], CoreExpr::Zap { value: value.id })?;
            }
            self.env.remove(&name);
        }
        Ok(())
    }

    fn bind_pattern(&mut self, pattern: &Pattern, value: LoweredValue) -> Result<(), LowerError> {
        match pattern {
            Pattern::Name(name) => {
                if self.env.insert(name.clone(), value).is_some() {
                    return Err(LowerError::DuplicateValue(name.clone()));
                }
                self.name_order.push(name.clone());
                Ok(())
            }
            Pattern::Wildcard => self.push_statement(vec![], CoreExpr::Zap { value: value.id }),
            Pattern::Unit => {
                if value.ty != self.types.unit() {
                    return Err(LowerError::TypeMismatch {
                        expected: self.types.unit(),
                        actual: value.ty,
                    });
                }
                self.push_statement(vec![], CoreExpr::Zap { value: value.id })
            }
            Pattern::Tuple(patterns) => self.bind_tuple_pattern(patterns, value),
            Pattern::Record(fields) => self.bind_record_pattern(fields, value),
        }
    }

    fn bind_tuple_pattern(
        &mut self,
        patterns: &[Pattern],
        value: LoweredValue,
    ) -> Result<(), LowerError> {
        let fields = self.product_fields(value.ty)?;
        if patterns.len() != fields.len() {
            return Err(LowerError::Core(CoreError::ResultArity {
                expected: fields.len(),
                actual: patterns.len(),
            }));
        }
        let result_ids = fields
            .iter()
            .map(|_| self.fresh_value())
            .collect::<Vec<_>>();
        self.push_statement(
            result_ids.clone(),
            CoreExpr::SplitProduct { value: value.id },
        )?;
        for ((pattern, field), id) in patterns.iter().zip(fields).zip(result_ids) {
            self.bind_pattern(pattern, LoweredValue { id, ty: field.ty })?;
        }
        Ok(())
    }

    fn bind_record_pattern(
        &mut self,
        patterns: &[Field<Pattern>],
        value: LoweredValue,
    ) -> Result<(), LowerError> {
        let fields = self.product_fields(value.ty)?;
        if patterns.len() != fields.len() {
            return Err(LowerError::Core(CoreError::ResultArity {
                expected: fields.len(),
                actual: patterns.len(),
            }));
        }
        let result_ids = fields
            .iter()
            .map(|_| self.fresh_value())
            .collect::<Vec<_>>();
        self.push_statement(
            result_ids.clone(),
            CoreExpr::SplitProduct { value: value.id },
        )?;

        for (field, id) in fields.into_iter().zip(result_ids) {
            let ComponentName::Named(name) = &field.name else {
                return Err(LowerError::UnsupportedExpression(
                    "record patterns require named product fields",
                ));
            };
            let pattern = patterns
                .iter()
                .find(|pattern| pattern.name.as_deref() == Some(name.as_str()))
                .ok_or_else(|| LowerError::UnknownValue(name.clone()))?;
            self.bind_pattern(&pattern.value, LoweredValue { id, ty: field.ty })?;
        }
        Ok(())
    }

    fn lower_expr(
        &mut self,
        expr: &Expr,
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        match expr {
            Expr::Name(name) => self.lower_name(name),
            Expr::Int(value) => self.lower_int(*value, expected),
            Expr::Unit => {
                let id = self.fresh_value();
                self.push_statement(vec![id], CoreExpr::Unit)?;
                Ok(LoweredValue {
                    id,
                    ty: self.types.unit(),
                })
            }
            Expr::Call { callee, args } => self.lower_call(callee, args, expected),
            Expr::Binary { lhs, op, rhs } => self.lower_binary(lhs, *op, rhs),
            Expr::Product(_) => Err(LowerError::UnsupportedExpression(
                "product literals need constructor context",
            )),
            Expr::FieldAccess { receiver, field } => {
                self.lower_field_access(receiver, field, expected)
            }
            Expr::MethodCall {
                receiver,
                receiver_flow,
                method,
                args,
            } => self.lower_method_call(receiver, *receiver_flow, method, args, expected),
            Expr::Match { scrutinee, arms } => self.lower_match(scrutinee, arms, expected),
            Expr::String(_) | Expr::Block(_) | Expr::If { .. } => Err(
                LowerError::UnsupportedExpression("expression form is not lowered yet"),
            ),
        }
    }

    fn lower_name(&mut self, name: &str) -> Result<LoweredValue, LowerError> {
        if let Some(value) = self.env.get(name).copied() {
            return Ok(value);
        }
        if let Some(global) = self.program.global_decl_id(name) {
            let ty = self
                .program
                .get_global_decl(global)
                .ok_or(CoreError::UnknownGlobal(global))?
                .ty;
            let id = self.fresh_value();
            self.push_statement(vec![id], CoreExpr::Global { global })?;
            return Ok(LoweredValue { id, ty });
        }
        Err(LowerError::UnknownValue(name.to_owned()))
    }

    fn local_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Name(name) if self.env.contains_key(name) => Some(name.clone()),
            _ => None,
        }
    }

    fn lower_consumed_expr(
        &mut self,
        expr: &Expr,
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        let name = self.local_name(expr);
        let value = self.lower_expr(expr, expected)?;
        if let Some(name) = name {
            self.env.remove(&name);
        }
        Ok(value)
    }

    fn lower_int(
        &mut self,
        value: u128,
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        let ty = match expected {
            Some(ty) => {
                self.require_finite(ty)?;
                ty
            }
            None => self
                .types
                .type_id("U32")
                .ok_or_else(|| LowerError::UnknownType("U32".into()))?,
        };
        let id = self.fresh_value();
        self.push_statement(vec![id], CoreExpr::FiniteLiteral { ty, value })?;
        Ok(LoweredValue { id, ty })
    }

    fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        if let Expr::FieldAccess { receiver, field } = callee {
            if let Expr::Name(type_name) = receiver.as_ref() {
                if let Some(ty) = self.types.type_id(type_name) {
                    return self.lower_enum_constructor_call(type_name, ty, field, args, expected);
                }
            }
        }

        let Expr::Name(name) = callee else {
            return Err(LowerError::UnsupportedExpression(
                "only direct calls are lowered yet",
            ));
        };

        if let Some(function) = self.program.function_id(name) {
            return self.lower_function_call(name, function, args);
        }

        if let Some(ty) = self.types.type_id(name) {
            return self.lower_constructor_call(name, ty, args, expected);
        }

        Err(LowerError::UnknownValue(name.clone()))
    }

    fn lower_field_access(
        &mut self,
        receiver: &Expr,
        field: &str,
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        if let Expr::Name(type_name) = receiver {
            if let Some(ty) = self.types.type_id(type_name) {
                return self.lower_unit_enum_constructor(type_name, ty, field, expected);
            }
        }
        Err(LowerError::UnsupportedExpression(
            "field access is not lowered yet",
        ))
    }

    fn lower_method_call(
        &mut self,
        receiver: &Expr,
        receiver_flow: ValueFlow,
        method: &str,
        args: &[Arg],
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        if let Expr::Name(type_name) = receiver {
            if let Some(ty) = self.types.type_id(type_name) {
                if receiver_flow != ValueFlow::ReturnedUnchanged {
                    return Err(LowerError::FlowMismatch {
                        expected: ValueFlow::ReturnedUnchanged,
                        actual: receiver_flow,
                    });
                }
                return self.lower_enum_constructor_call(type_name, ty, method, args, expected);
            }
        }
        Err(LowerError::UnsupportedExpression(
            "method calls are not lowered yet",
        ))
    }

    fn lower_function_call(
        &mut self,
        name: &str,
        function: FunctionId,
        args: &[Arg],
    ) -> Result<LoweredValue, LowerError> {
        let core_signature = self
            .program
            .get(function)
            .ok_or(CoreError::UnknownFunction(function))?;
        let call_signature =
            self.call_signatures
                .get(&function)
                .ok_or_else(|| LowerError::FunctionOutputArity {
                    name: name.to_owned(),
                    expected: 1,
                    actual: core_signature.outputs.len(),
                })?;

        if args.len() != core_signature.inputs.len() {
            return Err(LowerError::Core(CoreError::ResultArity {
                expected: core_signature.inputs.len(),
                actual: args.len(),
            }));
        }

        let mut arg_values = Vec::with_capacity(args.len());
        let mut returned_args = Vec::new();
        for ((arg, input), param) in args
            .iter()
            .zip(&core_signature.inputs)
            .zip(&call_signature.params)
        {
            self.check_arg_flow(arg.flow, param.flow)?;
            let value = if param.flow == ValueFlow::NotReturned {
                self.lower_consumed_expr(&arg.value, Some(input.ty))?
            } else {
                let (returned, value) = self.lower_returned_call_arg(arg, input.ty)?;
                if let ReturnedCallArg::Rebind(name) = &returned {
                    self.check_returned_arg_rebind_flow(name, param.flow)?;
                }
                returned_args.push(returned);
                value
            };
            if value.ty != input.ty {
                return Err(LowerError::TypeMismatch {
                    expected: input.ty,
                    actual: value.ty,
                });
            }
            arg_values.push(value.id);
        }

        let result_ids = core_signature
            .outputs
            .iter()
            .map(|_| self.fresh_value())
            .collect::<Vec<_>>();
        self.push_statement(
            result_ids.clone(),
            CoreExpr::Call {
                function,
                args: arg_values,
            },
        )?;

        let hidden_return_count = returned_args.len();
        for (returned, (id, ty)) in returned_args.into_iter().zip(
            result_ids
                .iter()
                .copied()
                .zip(core_signature.outputs.iter().copied()),
        ) {
            match returned {
                ReturnedCallArg::Rebind(name) => {
                    self.env.insert(name, LoweredValue { id, ty });
                }
                ReturnedCallArg::Zap => {
                    self.push_statement(vec![], CoreExpr::Zap { value: id })?;
                }
            }
        }

        if let Some(ty) = call_signature.visible_output {
            let id = result_ids[hidden_return_count];
            Ok(LoweredValue { id, ty })
        } else {
            let id = self.fresh_value();
            self.push_statement(vec![id], CoreExpr::Unit)?;
            Ok(LoweredValue {
                id,
                ty: self.types.unit(),
            })
        }
    }

    fn lower_returned_call_arg(
        &mut self,
        arg: &Arg,
        expected: TypeId,
    ) -> Result<(ReturnedCallArg, LoweredValue), LowerError> {
        if let Expr::Name(name) = &arg.value {
            if let Some(value) = self.env.get(name).copied() {
                return Ok((ReturnedCallArg::Rebind(name.clone()), value));
            }
        }
        let value = self.lower_consumed_expr(&arg.value, Some(expected))?;
        Ok((ReturnedCallArg::Zap, value))
    }

    fn check_arg_flow(&self, actual: ValueFlow, expected: ValueFlow) -> Result<(), LowerError> {
        if actual != ValueFlow::ReturnedUnchanged && actual != expected {
            return Err(LowerError::FlowMismatch { expected, actual });
        }
        Ok(())
    }

    fn check_returned_arg_rebind_flow(
        &self,
        name: &str,
        callee_flow: ValueFlow,
    ) -> Result<(), LowerError> {
        if self.param_flow(name) == Some(ValueFlow::ReturnedUnchanged)
            && callee_flow == ValueFlow::ReturnedChanged
        {
            return Err(LowerError::FlowMismatch {
                expected: ValueFlow::ReturnedUnchanged,
                actual: ValueFlow::ReturnedChanged,
            });
        }
        Ok(())
    }

    fn lower_constructor_call(
        &mut self,
        name: &str,
        ty: TypeId,
        args: &[Arg],
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        if let Some(expected) = expected {
            if expected != ty {
                return Err(LowerError::TypeMismatch {
                    expected,
                    actual: ty,
                });
            }
        }
        let TypeKind::Product(fields) = self
            .types
            .get(ty)
            .ok_or(LowerError::Type(TypeError::UnknownType(ty)))?
            .kind
            .clone()
        else {
            return Err(LowerError::UnsupportedExpression(
                "only product constructors are lowered yet",
            ));
        };

        let field_values = match args {
            [arg] => match &arg.value {
                Expr::Product(surface_fields) => {
                    self.lower_product_fields(name, &fields, surface_fields)?
                }
                value => {
                    if fields.len() != 1 {
                        return Err(LowerError::Core(CoreError::ResultArity {
                            expected: fields.len(),
                            actual: 1,
                        }));
                    }
                    vec![self.lower_consumed_expr(value, Some(fields[0].ty))?.id]
                }
            },
            _ => {
                if args.len() != fields.len() {
                    return Err(LowerError::Core(CoreError::ResultArity {
                        expected: fields.len(),
                        actual: args.len(),
                    }));
                }
                args.iter()
                    .zip(&fields)
                    .map(|(arg, field)| {
                        self.lower_consumed_expr(&arg.value, Some(field.ty))
                            .map(|v| v.id)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };

        let id = self.fresh_value();
        self.push_statement(
            vec![id],
            CoreExpr::Product {
                ty,
                fields: field_values,
            },
        )?;
        Ok(LoweredValue { id, ty })
    }

    fn lower_product_fields(
        &mut self,
        constructor: &str,
        expected: &[Component],
        surface: &[Field<Expr>],
    ) -> Result<Vec<ValueId>, LowerError> {
        let mut values = Vec::with_capacity(expected.len());
        for (index, component) in expected.iter().enumerate() {
            let surface_field = match &component.name {
                crate::types::ComponentName::Named(name) => surface
                    .iter()
                    .find(|field| field.name.as_deref() == Some(name.as_str())),
                crate::types::ComponentName::Index(index) => surface.get(*index),
            }
            .ok_or_else(|| LowerError::UnknownValue(format!("{constructor}.field{index}")))?;
            values.push(
                self.lower_consumed_expr(&surface_field.value, Some(component.ty))?
                    .id,
            );
        }
        Ok(values)
    }

    fn lower_unit_enum_constructor(
        &mut self,
        type_name: &str,
        ty: TypeId,
        variant_name: &str,
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        let (variant, payload_ty) = self.enum_variant(type_name, ty, variant_name)?;
        if payload_ty != self.types.unit() {
            return Err(LowerError::Core(CoreError::ResultArity {
                expected: 1,
                actual: 0,
            }));
        }
        let payload = self.lower_unit_payload()?;
        self.lower_enum_inject(ty, variant, payload, expected)
    }

    fn lower_enum_constructor_call(
        &mut self,
        type_name: &str,
        ty: TypeId,
        variant_name: &str,
        args: &[Arg],
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        let (variant, payload_ty) = self.enum_variant(type_name, ty, variant_name)?;
        let payload = self.lower_variant_payload(type_name, payload_ty, args)?;
        self.lower_enum_inject(ty, variant, payload, expected)
    }

    fn lower_enum_inject(
        &mut self,
        ty: TypeId,
        variant: usize,
        payload: LoweredValue,
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        if let Some(expected) = expected {
            if expected != ty {
                return Err(LowerError::TypeMismatch {
                    expected,
                    actual: ty,
                });
            }
        }
        let id = self.fresh_value();
        self.push_statement(
            vec![id],
            CoreExpr::SumInject {
                ty,
                variant,
                payload: payload.id,
            },
        )?;
        Ok(LoweredValue { id, ty })
    }

    fn lower_variant_payload(
        &mut self,
        constructor: &str,
        payload_ty: TypeId,
        args: &[Arg],
    ) -> Result<LoweredValue, LowerError> {
        if payload_ty == self.types.unit() {
            if !args.is_empty() {
                return Err(LowerError::Core(CoreError::ResultArity {
                    expected: 0,
                    actual: args.len(),
                }));
            }
            return self.lower_unit_payload();
        }

        if let TypeKind::Product(fields) = self
            .types
            .get(payload_ty)
            .ok_or(LowerError::Type(TypeError::UnknownType(payload_ty)))?
            .kind
            .clone()
        {
            let field_values = match args {
                [arg] => match &arg.value {
                    Expr::Product(surface_fields) => {
                        self.lower_product_fields(constructor, &fields, surface_fields)?
                    }
                    value => {
                        if fields.len() != 1 {
                            return Err(LowerError::Core(CoreError::ResultArity {
                                expected: fields.len(),
                                actual: 1,
                            }));
                        }
                        vec![self.lower_consumed_expr(value, Some(fields[0].ty))?.id]
                    }
                },
                _ => {
                    if args.len() != fields.len() {
                        return Err(LowerError::Core(CoreError::ResultArity {
                            expected: fields.len(),
                            actual: args.len(),
                        }));
                    }
                    args.iter()
                        .zip(&fields)
                        .map(|(arg, field)| {
                            self.lower_consumed_expr(&arg.value, Some(field.ty))
                                .map(|value| value.id)
                        })
                        .collect::<Result<Vec<_>, _>>()?
                }
            };

            let id = self.fresh_value();
            self.push_statement(
                vec![id],
                CoreExpr::Product {
                    ty: payload_ty,
                    fields: field_values,
                },
            )?;
            return Ok(LoweredValue { id, ty: payload_ty });
        }

        let [arg] = args else {
            return Err(LowerError::Core(CoreError::ResultArity {
                expected: 1,
                actual: args.len(),
            }));
        };
        self.lower_consumed_expr(&arg.value, Some(payload_ty))
    }

    fn lower_unit_payload(&mut self) -> Result<LoweredValue, LowerError> {
        let id = self.fresh_value();
        self.push_statement(vec![id], CoreExpr::Unit)?;
        Ok(LoweredValue {
            id,
            ty: self.types.unit(),
        })
    }

    fn enum_variant(
        &self,
        type_name: &str,
        ty: TypeId,
        variant_name: &str,
    ) -> Result<(usize, TypeId), LowerError> {
        let TypeKind::Sum(variants) = self
            .types
            .get(ty)
            .ok_or(LowerError::Type(TypeError::UnknownType(ty)))?
            .kind
            .clone()
        else {
            return Err(LowerError::UnsupportedExpression(
                "only enum variants can be constructed with dot syntax",
            ));
        };
        variants
            .iter()
            .enumerate()
            .find_map(|(index, variant)| match &variant.name {
                ComponentName::Named(name) if name == variant_name => Some((index, variant.ty)),
                _ => None,
            })
            .ok_or_else(|| LowerError::UnknownValue(format!("{type_name}.{variant_name}")))
    }

    fn lower_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[FrontendMatchArm],
        expected: Option<TypeId>,
    ) -> Result<LoweredValue, LowerError> {
        let scrutinee_name = match scrutinee {
            Expr::Name(name) if self.env.contains_key(name) => Some(name.clone()),
            _ => None,
        };
        let scrutinee = self.lower_expr(scrutinee, None)?;
        let variants = self.sum_variants(scrutinee.ty)?;
        let expected = match expected {
            Some(expected) => Some(expected),
            None => self.infer_match_result_type(&variants, arms)?,
        };
        let threaded_names = self.live_names_except(scrutinee_name.as_deref());
        let payload_id = self.fresh_value();
        let mut core_arms = Vec::with_capacity(arms.len());

        for arm in arms {
            let (variant, payload_ty) = find_variant(&variants, &arm.variant)
                .ok_or_else(|| LowerError::UnknownValue(arm.variant.clone()))?;
            let mut arm_lowerer = self.arm_lowerer(scrutinee_name.as_deref(), payload_id);
            let payload = LoweredValue {
                id: payload_id,
                ty: payload_ty,
            };
            match &arm.payload {
                Some(pattern) => arm_lowerer.bind_pattern(pattern, payload)?,
                None => {
                    arm_lowerer.push_statement(vec![], CoreExpr::Zap { value: payload.id })?;
                }
            }
            let arm_block = expr_as_block(&arm.body);
            let (body, returns) =
                arm_lowerer.lower_block_returning(&arm_block, expected, threaded_names.clone())?;
            core_arms.push(CoreMatchArm::new(variant, payload_id, body, returns));
        }

        if let Some(name) = &scrutinee_name {
            self.env.remove(name);
        }

        let result_count = threaded_names.len() + usize::from(expected.is_some());
        let result_ids = (0..result_count)
            .map(|_| self.fresh_value())
            .collect::<Vec<_>>();
        self.push_statement(
            result_ids.clone(),
            CoreExpr::Match {
                scrutinee: scrutinee.id,
                arms: core_arms,
            },
        )?;

        for (threaded, id) in threaded_names.iter().zip(result_ids.iter().copied()) {
            self.env.insert(
                threaded.name.clone(),
                LoweredValue {
                    id,
                    ty: threaded.ty,
                },
            );
        }

        if let Some(ty) = expected {
            let id = result_ids[result_count - 1];
            Ok(LoweredValue { id, ty })
        } else {
            let id = self.fresh_value();
            self.push_statement(vec![id], CoreExpr::Unit)?;
            Ok(LoweredValue {
                id,
                ty: self.types.unit(),
            })
        }
    }

    fn arm_lowerer(&self, consumed_name: Option<&str>, payload_id: ValueId) -> BodyLowerer<'a> {
        let mut env = self.env.clone();
        if let Some(name) = consumed_name {
            env.remove(name);
        }
        BodyLowerer {
            types: self.types,
            program: self.program,
            call_signatures: self.call_signatures,
            params: self.params.clone(),
            env,
            name_order: self.name_order.clone(),
            body: Vec::new(),
            next_value: payload_id.0 + 1,
        }
    }

    fn lower_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOp,
        rhs: &Expr,
    ) -> Result<LoweredValue, LowerError> {
        let lhs = self.lower_binary_operand(lhs, None)?;
        self.require_finite(lhs.value.ty)?;
        let rhs = self.lower_binary_operand(rhs, Some(lhs.value.ty))?;
        self.ensure_binary_operand_still_live(&lhs)?;
        if lhs.name.is_some() && lhs.name == rhs.name {
            return Err(LowerError::DuplicateLinearUse(
                lhs.name.expect("checked").clone(),
            ));
        }
        if rhs.value.ty != lhs.value.ty {
            return Err(LowerError::TypeMismatch {
                expected: lhs.value.ty,
                actual: rhs.value.ty,
            });
        }

        let bool_ty = self
            .types
            .type_id("Bool")
            .ok_or_else(|| LowerError::UnknownType("Bool".into()))?;
        let (core_op, args, ty, returned_operands) = match op {
            BinaryOp::Add => (
                BuiltinOp::FiniteAdd { ty: lhs.value.ty },
                vec![lhs.value.id, rhs.value.id],
                lhs.value.ty,
                vec![lhs, rhs],
            ),
            BinaryOp::Sub => (
                BuiltinOp::FiniteSub { ty: lhs.value.ty },
                vec![lhs.value.id, rhs.value.id],
                lhs.value.ty,
                vec![lhs, rhs],
            ),
            BinaryOp::Mul => (
                BuiltinOp::FiniteMul { ty: lhs.value.ty },
                vec![lhs.value.id, rhs.value.id],
                lhs.value.ty,
                vec![lhs, rhs],
            ),
            BinaryOp::Eq => (
                BuiltinOp::FiniteEq {
                    ty: lhs.value.ty,
                    bool_ty,
                },
                vec![lhs.value.id, rhs.value.id],
                bool_ty,
                vec![lhs, rhs],
            ),
            BinaryOp::Lt => (
                BuiltinOp::FiniteLt {
                    ty: lhs.value.ty,
                    bool_ty,
                },
                vec![lhs.value.id, rhs.value.id],
                bool_ty,
                vec![lhs, rhs],
            ),
            BinaryOp::Gt => (
                BuiltinOp::FiniteLt {
                    ty: lhs.value.ty,
                    bool_ty,
                },
                vec![rhs.value.id, lhs.value.id],
                bool_ty,
                vec![rhs, lhs],
            ),
            BinaryOp::Div | BinaryOp::NotEq | BinaryOp::Lte | BinaryOp::Gte => {
                return Err(LowerError::UnsupportedExpression(
                    "binary operator is not lowered yet",
                ));
            }
        };

        let returned_lhs = self.fresh_value();
        let returned_rhs = self.fresh_value();
        let id = self.fresh_value();
        self.push_statement(
            vec![returned_lhs, returned_rhs, id],
            CoreExpr::Builtin { op: core_op, args },
        )?;
        for (operand, returned_id) in returned_operands
            .into_iter()
            .zip([returned_lhs, returned_rhs])
        {
            self.handle_returned_binary_operand(operand, returned_id)?;
        }
        Ok(LoweredValue { id, ty })
    }

    fn lower_binary_operand(
        &mut self,
        expr: &Expr,
        expected: Option<TypeId>,
    ) -> Result<LoweredBinaryOperand, LowerError> {
        let name = match expr {
            Expr::Name(name) if self.env.contains_key(name) => Some(name.clone()),
            _ => None,
        };
        let value = self.lower_expr(expr, expected)?;
        Ok(LoweredBinaryOperand { name, value })
    }

    fn ensure_binary_operand_still_live(
        &self,
        operand: &LoweredBinaryOperand,
    ) -> Result<(), LowerError> {
        let Some(name) = &operand.name else {
            return Ok(());
        };
        match self.env.get(name) {
            Some(current) if current.id == operand.value.id => Ok(()),
            _ => Err(LowerError::ValueMovedDuringExpression(name.clone())),
        }
    }

    fn handle_returned_binary_operand(
        &mut self,
        operand: LoweredBinaryOperand,
        returned_id: ValueId,
    ) -> Result<(), LowerError> {
        match operand.name {
            Some(name) => {
                self.env.insert(
                    name,
                    LoweredValue {
                        id: returned_id,
                        ty: operand.value.ty,
                    },
                );
                Ok(())
            }
            None => self.push_statement(vec![], CoreExpr::Zap { value: returned_id }),
        }
    }

    fn infer_match_result_type(
        &self,
        variants: &[Component],
        arms: &[FrontendMatchArm],
    ) -> Result<Option<TypeId>, LowerError> {
        let mut inferred = None;
        for arm in arms {
            let (_, payload_ty) = find_variant(variants, &arm.variant)
                .ok_or_else(|| LowerError::UnknownValue(arm.variant.clone()))?;
            let mut env = self.inference_env();
            if let Some(pattern) = &arm.payload {
                self.bind_pattern_type(pattern, payload_ty, &mut env)?;
            }

            let ty = match self.infer_expr_type_with_env(&arm.body, &env) {
                Ok(ty) => ty,
                Err(LowerError::UnknownValue(_)) => continue,
                Err(err) => return Err(err),
            };
            match (inferred, ty) {
                (None, ty) => inferred = ty,
                (Some(expected), Some(actual)) if expected == actual => {}
                (Some(expected), Some(actual)) => {
                    return Err(LowerError::TypeMismatch { expected, actual });
                }
                (Some(expected), None) => {
                    return Err(LowerError::MissingResult { expected });
                }
            }
        }
        Ok(inferred)
    }

    fn inference_env(&self) -> HashMap<String, TypeId> {
        self.env
            .iter()
            .map(|(name, value)| (name.clone(), value.ty))
            .collect()
    }

    fn infer_expr_type_with_env(
        &self,
        expr: &Expr,
        env: &HashMap<String, TypeId>,
    ) -> Result<Option<TypeId>, LowerError> {
        match expr {
            Expr::Name(name) => env
                .get(name)
                .map(|ty| Some(*ty))
                .ok_or_else(|| LowerError::UnknownValue(name.clone())),
            Expr::Int(_) => self
                .types
                .type_id("U32")
                .map(Some)
                .ok_or_else(|| LowerError::UnknownType("U32".into())),
            Expr::Unit => Ok(None),
            Expr::Block(block) => block
                .result
                .as_deref()
                .map(|result| self.infer_expr_type_with_env(result, env))
                .unwrap_or(Ok(None)),
            Expr::Binary { lhs, op, .. } => {
                let lhs_ty = self.infer_expr_type_with_env(lhs, env)?.ok_or_else(|| {
                    LowerError::TypeMismatch {
                        expected: self.types.unit(),
                        actual: self.types.unit(),
                    }
                })?;
                match op {
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                        Ok(Some(lhs_ty))
                    }
                    BinaryOp::Eq
                    | BinaryOp::NotEq
                    | BinaryOp::Lt
                    | BinaryOp::Lte
                    | BinaryOp::Gt
                    | BinaryOp::Gte => self
                        .types
                        .type_id("Bool")
                        .map(Some)
                        .ok_or_else(|| LowerError::UnknownType("Bool".into())),
                }
            }
            Expr::Call { callee, .. } => self.infer_call_type(callee),
            Expr::Match { scrutinee, arms } => {
                let Some(scrutinee_ty) = self.infer_expr_type_with_env(scrutinee, env)? else {
                    return Ok(None);
                };
                let variants = self.sum_variants(scrutinee_ty)?;
                self.infer_match_result_type(&variants, arms)
            }
            Expr::String(_)
            | Expr::Product(_)
            | Expr::FieldAccess { .. }
            | Expr::MethodCall { .. }
            | Expr::If { .. } => Ok(None),
        }
    }

    fn bind_pattern_type(
        &self,
        pattern: &Pattern,
        ty: TypeId,
        env: &mut HashMap<String, TypeId>,
    ) -> Result<(), LowerError> {
        match pattern {
            Pattern::Name(name) => {
                if env.insert(name.clone(), ty).is_some() {
                    return Err(LowerError::DuplicateValue(name.clone()));
                }
                Ok(())
            }
            Pattern::Wildcard => Ok(()),
            Pattern::Unit => {
                if ty == self.types.unit() {
                    Ok(())
                } else {
                    Err(LowerError::TypeMismatch {
                        expected: self.types.unit(),
                        actual: ty,
                    })
                }
            }
            Pattern::Tuple(patterns) => {
                let fields = self.product_fields(ty)?;
                if patterns.len() != fields.len() {
                    return Err(LowerError::Core(CoreError::ResultArity {
                        expected: fields.len(),
                        actual: patterns.len(),
                    }));
                }
                for (pattern, field) in patterns.iter().zip(fields) {
                    self.bind_pattern_type(pattern, field.ty, env)?;
                }
                Ok(())
            }
            Pattern::Record(patterns) => {
                let fields = self.product_fields(ty)?;
                for field in fields {
                    let ComponentName::Named(name) = &field.name else {
                        return Err(LowerError::UnsupportedExpression(
                            "record patterns require named product fields",
                        ));
                    };
                    let pattern = patterns
                        .iter()
                        .find(|pattern| pattern.name.as_deref() == Some(name.as_str()))
                        .ok_or_else(|| LowerError::UnknownValue(name.clone()))?;
                    self.bind_pattern_type(&pattern.value, field.ty, env)?;
                }
                Ok(())
            }
        }
    }

    fn infer_call_type(&self, callee: &Expr) -> Result<Option<TypeId>, LowerError> {
        match callee {
            Expr::Name(name) => {
                if let Some(function) = self.program.function_id(name) {
                    let signature = self
                        .call_signatures
                        .get(&function)
                        .ok_or_else(|| LowerError::UnknownValue(name.clone()))?;
                    return Ok(signature.visible_output);
                }
                if let Some(ty) = self.types.type_id(name) {
                    return Ok(Some(ty));
                }
                Ok(None)
            }
            Expr::FieldAccess { receiver, .. } => {
                if let Expr::Name(type_name) = receiver.as_ref() {
                    if let Some(ty) = self.types.type_id(type_name) {
                        return Ok(Some(ty));
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn resolve_existing_type(&self, ty: &TypeExpr) -> Result<TypeId, LowerError> {
        match ty {
            TypeExpr::Unit => Ok(self.types.unit()),
            TypeExpr::Name(name) => self
                .types
                .type_id(name)
                .ok_or_else(|| LowerError::UnknownType(name.clone())),
            TypeExpr::Apply { .. }
            | TypeExpr::Product(_)
            | TypeExpr::Sum(_)
            | TypeExpr::Function { .. } => Err(LowerError::UnsupportedExpression(
                "complex let type annotations are not lowered yet",
            )),
        }
    }

    fn require_finite(&self, ty: TypeId) -> Result<(), LowerError> {
        match &self
            .types
            .get(ty)
            .ok_or(LowerError::Type(TypeError::UnknownType(ty)))?
            .kind
        {
            TypeKind::Finite { .. } => Ok(()),
            _ => Err(LowerError::Core(CoreError::NotFinite(ty))),
        }
    }

    fn fresh_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    fn push_statement(&mut self, results: Vec<ValueId>, expr: CoreExpr) -> Result<(), LowerError> {
        self.body.push(Statement::new(results, expr));
        Ok(())
    }

    fn product_fields(&self, ty: TypeId) -> Result<Vec<Component>, LowerError> {
        let TypeKind::Product(fields) = self
            .types
            .get(ty)
            .ok_or(LowerError::Type(TypeError::UnknownType(ty)))?
            .kind
            .clone()
        else {
            return Err(LowerError::Core(CoreError::NotProduct(ty)));
        };
        Ok(fields)
    }

    fn sum_variants(&self, ty: TypeId) -> Result<Vec<Component>, LowerError> {
        let TypeKind::Sum(variants) = self
            .types
            .get(ty)
            .ok_or(LowerError::Type(TypeError::UnknownType(ty)))?
            .kind
            .clone()
        else {
            return Err(LowerError::Core(CoreError::NotSum(ty)));
        };
        Ok(variants)
    }

    fn param_flow(&self, name: &str) -> Option<ValueFlow> {
        self.params
            .iter()
            .find(|param| param.name == name)
            .map(|param| param.flow)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum AnonymousTypeKey {
    Product(Vec<Component>),
    Sum(Vec<Component>),
    Function {
        input: TypeId,
        output: TypeId,
    },
    List {
        element: TypeId,
        mutability: CollectionMutability,
    },
    HashMap {
        key: TypeId,
        value: TypeId,
        mutability: CollectionMutability,
    },
}

fn reject_generics(name: &str, generics: &[String]) -> Result<(), LowerError> {
    if generics.is_empty() {
        Ok(())
    } else {
        Err(LowerError::UnsupportedGenericDecl {
            name: name.to_owned(),
        })
    }
}

fn reject_alias_capabilities(name: &str, capabilities: &[String]) -> Result<(), LowerError> {
    if capabilities.is_empty() {
        Ok(())
    } else {
        Err(LowerError::UnsupportedAliasCapabilities {
            name: name.to_owned(),
        })
    }
}

fn declared_capabilities(capabilities: &[String]) -> Result<DeclaredCapabilities, LowerError> {
    let mut declared = DeclaredCapabilities::linear();
    for capability in capabilities {
        match capability.as_str() {
            "Dup" => declared.dup = true,
            "Zap" => declared.zap = true,
            _ => return Err(LowerError::UnknownCapability(capability.clone())),
        }
    }
    Ok(declared)
}

fn expect_arity<'a, const N: usize>(
    name: &str,
    args: &'a [TypeExpr],
    expected: usize,
) -> Result<&'a [TypeExpr; N], LowerError> {
    if args.len() != expected {
        return Err(LowerError::BadGenericArity {
            name: name.to_owned(),
            expected,
            actual: args.len(),
        });
    }
    Ok(args.try_into().expect("arity checked"))
}

fn build_call_signatures(lowered: &LoweredModule) -> HashMap<FunctionId, CallableSignature> {
    let mut signatures = HashMap::new();
    for function in &lowered.functions {
        signatures.insert(function.id, callable_signature(&lowered.types, function));
    }
    for method in &lowered.methods {
        signatures.insert(
            method.function.id,
            callable_signature(&lowered.types, &method.function),
        );
    }
    signatures
}

fn callable_signature(types: &TypeStore, function: &LoweredFunction) -> CallableSignature {
    CallableSignature {
        params: function.params.clone(),
        visible_output: visible_output(types, function.output),
    }
}

fn core_output_types(unit: TypeId, params: &[LoweredParam], output: TypeId) -> Vec<TypeId> {
    let mut outputs = params
        .iter()
        .filter(|param| param.flow != ValueFlow::NotReturned)
        .map(|param| param.ty)
        .collect::<Vec<_>>();
    if output != unit {
        outputs.push(output);
    }
    outputs
}

fn visible_output(types: &TypeStore, output: TypeId) -> Option<TypeId> {
    (output != types.unit()).then_some(output)
}

fn find_variant(variants: &[Component], name: &str) -> Option<(usize, TypeId)> {
    variants
        .iter()
        .enumerate()
        .find_map(|(index, variant)| match &variant.name {
            ComponentName::Named(variant_name) if variant_name == name => Some((index, variant.ty)),
            _ => None,
        })
}

fn expr_as_block(expr: &Expr) -> Block {
    match expr {
        Expr::Block(block) => block.clone(),
        expr => Block {
            statements: Vec::new(),
            result: Some(Box::new(expr.clone())),
        },
    }
}

fn type_expr_core_name(ty: &TypeExpr) -> Result<String, LowerError> {
    match ty {
        TypeExpr::Name(name) => Ok(name.clone()),
        TypeExpr::Apply { name, args } => {
            let args = args
                .iter()
                .map(type_expr_core_name)
                .collect::<Result<Vec<_>, _>>()?
                .join(",");
            Ok(format!("{name}<{args}>"))
        }
        TypeExpr::Unit | TypeExpr::Product(_) | TypeExpr::Sum(_) | TypeExpr::Function { .. } => {
            Err(LowerError::UnsupportedAnonymousImplTarget)
        }
    }
}

impl From<TypeError> for LowerError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

impl From<CoreError> for LowerError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}
