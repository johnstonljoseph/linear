use std::collections::HashMap;

use crate::TypeId;
use crate::core::{
    BuiltinOp, CoreError, CoreProgram, Expr as CoreExpr, Function, GlobalDecl, Param as CoreParam,
    Statement,
};
use crate::frontend::{
    Arg, BinaryOp, Block, Expr, Field, FunctionDef, GlobalDef as FrontendGlobalDef, Item, Module,
    Param as FrontendParam, TypeExpr, ValueFlow,
};
use crate::id::{FunctionId, GlobalId, ValueId};
use crate::types::{
    CollectionMutability, Component, DeclaredCapabilities, TypeError, TypeKind, TypeStore,
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
        types.add_uint("u8", 8)?;
        types.add_uint("u16", 16)?;
        types.add_uint("u32", 32)?;
        types.add_uint("u64", 64)?;
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
                    let ty = self.lower_type_expr(&type_def.ty)?;
                    self.types.add_alias(type_def.name.clone(), ty)?;
                }
                Item::Struct(type_def) => {
                    reject_generics(&type_def.name, &type_def.generics)?;
                    let fields = match &type_def.ty {
                        TypeExpr::Product(fields) => self.lower_components(fields)?,
                        TypeExpr::Unit => Vec::new(),
                        _ => {
                            return Err(LowerError::ExpectedStructBody {
                                name: type_def.name.clone(),
                            });
                        }
                    };
                    self.types.add_product(
                        Some(type_def.name.clone()),
                        fields,
                        DeclaredCapabilities::linear(),
                    )?;
                }
                Item::Enum(type_def) => {
                    reject_generics(&type_def.name, &type_def.generics)?;
                    let variants = match &type_def.ty {
                        TypeExpr::Sum(variants) => self.lower_components(variants)?,
                        _ => {
                            return Err(LowerError::ExpectedEnumBody {
                                name: type_def.name.clone(),
                            });
                        }
                    };
                    self.types.add_sum(
                        Some(type_def.name.clone()),
                        variants,
                        DeclaredCapabilities::linear(),
                    )?;
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
        Self {
            types,
            program,
            call_signatures,
            params: params.to_vec(),
            env,
            body: Vec::new(),
            next_value: params.len() as u32,
        }
    }

    fn lower_block(
        mut self,
        block: &Block,
        expected_result: Option<TypeId>,
    ) -> Result<(Vec<Statement>, Vec<ValueId>), LowerError> {
        for let_stmt in &block.lets {
            let value = self.lower_expr(&let_stmt.value, None)?;
            if let Some(ty) = &let_stmt.ty {
                let expected = self.resolve_existing_type(ty)?;
                if value.ty != expected {
                    return Err(LowerError::TypeMismatch {
                        expected,
                        actual: value.ty,
                    });
                }
            }
            self.bind_pattern(&let_stmt.pattern, value)?;
        }

        let explicit_result =
            self.lower_optional_result(block.result.as_deref(), expected_result)?;
        let mut returns = self.implicit_return_values()?;
        if let Some(result) = explicit_result {
            returns.push(result.id);
        }

        Ok((self.body, returns))
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

    fn implicit_return_values(&self) -> Result<Vec<ValueId>, LowerError> {
        self.params
            .iter()
            .filter(|param| param.flow != ValueFlow::NotReturned)
            .map(|param| {
                self.env
                    .get(&param.name)
                    .map(|value| value.id)
                    .ok_or_else(|| LowerError::UnknownValue(param.name.clone()))
            })
            .collect()
    }

    fn bind_pattern(
        &mut self,
        pattern: &crate::frontend::Pattern,
        value: LoweredValue,
    ) -> Result<(), LowerError> {
        match pattern {
            crate::frontend::Pattern::Name(name) => {
                if self.env.insert(name.clone(), value).is_some() {
                    return Err(LowerError::DuplicateValue(name.clone()));
                }
                Ok(())
            }
            crate::frontend::Pattern::Wildcard => {
                self.push_statement(vec![], CoreExpr::Zap { value: value.id })
            }
            crate::frontend::Pattern::Unit => {
                if value.ty != self.types.unit() {
                    return Err(LowerError::TypeMismatch {
                        expected: self.types.unit(),
                        actual: value.ty,
                    });
                }
                self.push_statement(vec![], CoreExpr::Zap { value: value.id })
            }
            crate::frontend::Pattern::Tuple(_) | crate::frontend::Pattern::Record(_) => Err(
                LowerError::UnsupportedExpression("destructuring patterns are not lowered yet"),
            ),
        }
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
            Expr::String(_)
            | Expr::Block(_)
            | Expr::MethodCall { .. }
            | Expr::FieldAccess { .. }
            | Expr::Match { .. }
            | Expr::If { .. } => Err(LowerError::UnsupportedExpression(
                "expression form is not lowered yet",
            )),
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
                .type_id("u32")
                .ok_or_else(|| LowerError::UnknownType("u32".into()))?,
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
        let mut returned_arg_names = Vec::new();
        for ((arg, input), param) in args
            .iter()
            .zip(&core_signature.inputs)
            .zip(&call_signature.params)
        {
            self.check_arg_flow(arg.flow, param.flow)?;
            let value = if param.flow == ValueFlow::NotReturned {
                self.lower_expr(&arg.value, Some(input.ty))?
            } else {
                let (name, value) = self.lower_returned_call_arg(arg)?;
                self.check_returned_arg_rebind_flow(&name, param.flow)?;
                returned_arg_names.push(name);
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

        let hidden_return_count = returned_arg_names.len();
        for (name, (id, ty)) in returned_arg_names.into_iter().zip(
            result_ids
                .iter()
                .copied()
                .zip(core_signature.outputs.iter().copied()),
        ) {
            self.env.insert(name, LoweredValue { id, ty });
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

    fn lower_returned_call_arg(&self, arg: &Arg) -> Result<(String, LoweredValue), LowerError> {
        let Expr::Name(name) = &arg.value else {
            return Err(LowerError::ExpectedNameForReturnedArgument);
        };
        let value = self
            .env
            .get(name)
            .copied()
            .ok_or_else(|| LowerError::UnknownValue(name.clone()))?;
        Ok((name.clone(), value))
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
                    vec![self.lower_expr(value, Some(fields[0].ty))?.id]
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
                    .map(|(arg, field)| self.lower_expr(&arg.value, Some(field.ty)).map(|v| v.id))
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
                self.lower_expr(&surface_field.value, Some(component.ty))?
                    .id,
            );
        }
        Ok(values)
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

    fn handle_returned_binary_operand(
        &mut self,
        operand: LoweredBinaryOperand,
        returned_id: ValueId,
    ) -> Result<(), LowerError> {
        match operand.name {
            Some(name)
                if matches!(
                    self.param_flow(&name),
                    Some(flow) if flow != ValueFlow::NotReturned
                ) =>
            {
                self.env.insert(
                    name,
                    LoweredValue {
                        id: returned_id,
                        ty: operand.value.ty,
                    },
                );
                Ok(())
            }
            Some(name) => {
                self.env.remove(&name);
                self.push_statement(vec![], CoreExpr::Zap { value: returned_id })
            }
            None => self.push_statement(vec![], CoreExpr::Zap { value: returned_id }),
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
