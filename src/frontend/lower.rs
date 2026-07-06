use std::collections::HashMap;

use crate::TypeId;
use crate::core::{CoreError, CoreProgram, Function, GlobalDecl, Param as CoreParam, Statement};
use crate::frontend::{
    Block, Expr, Field, FunctionDef, GlobalDef as FrontendGlobalDef, Item, Module,
    Param as FrontendParam, TypeExpr, ValueFlow,
};
use crate::id::{FunctionId, GlobalId, ValueId};
use crate::types::{CollectionMutability, Component, DeclaredCapabilities, TypeError, TypeStore};

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
        let id = program.add_function(Function {
            name: Some(core_name.to_owned()),
            inputs: params
                .iter()
                .map(|param| CoreParam::new(param.id, param.ty))
                .collect(),
            outputs: vec![output],
            body: Vec::<Statement>::new(),
            returns: Vec::new(),
        })?;
        Ok(LoweredFunction {
            id,
            name: core_name.to_owned(),
            params,
            output,
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
