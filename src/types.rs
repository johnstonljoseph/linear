use std::collections::HashMap;

use crate::id::TypeId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeStore {
    types: Vec<TypeDef>,
    names: HashMap<String, TypeId>,
    never: TypeId,
    unit: TypeId,
}

impl TypeStore {
    pub fn new() -> Self {
        let mut store = Self {
            types: Vec::new(),
            names: HashMap::new(),
            never: TypeId(0),
            unit: TypeId(0),
        };
        store.never = store
            .add_named_builtin("Never", TypeKind::Never, DeclaredCapabilities::dup_zap())
            .expect("fresh store should accept Never");
        store.unit = store
            .add_named_builtin("Unit", TypeKind::Unit, DeclaredCapabilities::dup_zap())
            .expect("fresh store should accept Unit");
        store
    }

    pub fn never(&self) -> TypeId {
        self.never
    }

    pub fn unit(&self) -> TypeId {
        self.unit
    }

    pub fn get(&self, id: TypeId) -> Option<&TypeDef> {
        self.types.get(id.index())
    }

    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.names.get(name).copied()
    }

    pub fn add_alias(&mut self, name: impl Into<String>, target: TypeId) -> Result<(), TypeError> {
        self.validate_type(target)?;
        let name = name.into();
        validate_name(&name)?;
        if self.names.contains_key(&name) {
            return Err(TypeError::DuplicateName(name));
        }
        self.names.insert(name, target);
        Ok(())
    }

    pub fn types(&self) -> impl Iterator<Item = (TypeId, &TypeDef)> {
        self.types
            .iter()
            .enumerate()
            .map(|(index, def)| (TypeId(index as u32), def))
    }

    pub fn add_finite(
        &mut self,
        name: Option<String>,
        values: u128,
        declared: DeclaredCapabilities,
    ) -> Result<TypeId, TypeError> {
        if values == 0 {
            return Err(TypeError::ZeroFiniteCardinality);
        }
        self.add_type(name, TypeKind::Finite { values }, declared)
    }

    pub fn add_uint(&mut self, name: impl Into<String>, bits: u32) -> Result<TypeId, TypeError> {
        if bits >= 128 {
            return Err(TypeError::UIntTooWide(bits));
        }
        self.add_finite(
            Some(name.into()),
            1u128 << bits,
            DeclaredCapabilities::dup_zap(),
        )
    }

    pub fn add_sum(
        &mut self,
        name: Option<String>,
        variants: Vec<Component>,
        declared: DeclaredCapabilities,
    ) -> Result<TypeId, TypeError> {
        self.validate_components(&variants)?;
        self.add_type(name, TypeKind::Sum(variants), declared)
    }

    pub fn add_product(
        &mut self,
        name: Option<String>,
        fields: Vec<Component>,
        declared: DeclaredCapabilities,
    ) -> Result<TypeId, TypeError> {
        self.validate_components(&fields)?;
        self.add_type(name, TypeKind::Product(fields), declared)
    }

    pub fn add_function(
        &mut self,
        name: Option<String>,
        input: TypeId,
        output: TypeId,
    ) -> Result<TypeId, TypeError> {
        self.validate_type(input)?;
        self.validate_type(output)?;
        self.add_type(
            name,
            TypeKind::Function { input, output },
            DeclaredCapabilities::dup_zap(),
        )
    }

    pub fn add_symbol(&mut self, name: impl Into<String>) -> Result<TypeId, TypeError> {
        self.add_type(
            Some(name.into()),
            TypeKind::Symbol,
            DeclaredCapabilities::dup_zap(),
        )
    }

    pub fn add_text(&mut self, name: impl Into<String>) -> Result<TypeId, TypeError> {
        self.add_type(
            Some(name.into()),
            TypeKind::Text,
            DeclaredCapabilities::dup_zap(),
        )
    }

    pub fn add_list(
        &mut self,
        name: Option<String>,
        element: TypeId,
        mutability: CollectionMutability,
    ) -> Result<TypeId, TypeError> {
        self.validate_type(element)?;
        self.add_type(
            name,
            TypeKind::List {
                element,
                mutability,
            },
            DeclaredCapabilities::linear(),
        )
    }

    pub fn add_vector(
        &mut self,
        name: Option<String>,
        element: TypeId,
        mutability: CollectionMutability,
    ) -> Result<TypeId, TypeError> {
        self.add_list(name, element, mutability)
    }

    pub fn add_hashmap(
        &mut self,
        name: Option<String>,
        key: TypeId,
        value: TypeId,
        mutability: CollectionMutability,
    ) -> Result<TypeId, TypeError> {
        self.validate_type(key)?;
        self.validate_type(value)?;
        self.add_type(
            name,
            TypeKind::HashMap {
                key,
                value,
                mutability,
            },
            DeclaredCapabilities::linear(),
        )
    }

    pub fn add_primitive(
        &mut self,
        name: impl Into<String>,
        declared: DeclaredCapabilities,
    ) -> Result<TypeId, TypeError> {
        self.add_type(Some(name.into()), TypeKind::Primitive, declared)
    }

    pub fn capabilities(&self, id: TypeId) -> Result<Capabilities, TypeError> {
        self.validate_type(id)?;
        let mut visiting = Vec::new();
        self.capabilities_inner(id, &mut visiting)
    }

    pub fn can_dup(&self, id: TypeId) -> Result<bool, TypeError> {
        self.capabilities(id).map(|caps| caps.dup)
    }

    pub fn can_zap(&self, id: TypeId) -> Result<bool, TypeError> {
        self.capabilities(id).map(|caps| caps.zap)
    }

    fn add_named_builtin(
        &mut self,
        name: impl Into<String>,
        kind: TypeKind,
        declared: DeclaredCapabilities,
    ) -> Result<TypeId, TypeError> {
        self.add_type(Some(name.into()), kind, declared)
    }

    fn add_type(
        &mut self,
        name: Option<String>,
        kind: TypeKind,
        declared: DeclaredCapabilities,
    ) -> Result<TypeId, TypeError> {
        if let Some(name) = &name {
            validate_name(name)?;
            if self.names.contains_key(name) {
                return Err(TypeError::DuplicateName(name.clone()));
            }
        }
        self.validate_declared_capabilities(&kind, declared)?;
        let id = TypeId(self.types.len() as u32);
        self.types.push(TypeDef {
            name: name.clone(),
            kind,
            declared,
        });
        if let Some(name) = name {
            self.names.insert(name, id);
        }
        Ok(id)
    }

    fn validate_declared_capabilities(
        &self,
        kind: &TypeKind,
        declared: DeclaredCapabilities,
    ) -> Result<(), TypeError> {
        if matches!(kind, TypeKind::Primitive) {
            return Ok(());
        }
        let structural = self.structural_capabilities_for_kind(kind)?;
        let declared = declared.into_capabilities();
        if structural.allows(declared) {
            Ok(())
        } else {
            Err(TypeError::DeclaredCapabilityExceedsStructural {
                declared,
                structural,
            })
        }
    }

    fn structural_capabilities_for_kind(&self, kind: &TypeKind) -> Result<Capabilities, TypeError> {
        match kind {
            TypeKind::Never
            | TypeKind::Unit
            | TypeKind::Finite { .. }
            | TypeKind::Function { .. }
            | TypeKind::Symbol
            | TypeKind::Text => Ok(Capabilities::dup_zap()),
            TypeKind::Primitive => Ok(Capabilities::linear()),
            TypeKind::List {
                element,
                mutability,
            } => match mutability {
                CollectionMutability::Mutable => Ok(Capabilities::linear()),
                CollectionMutability::Immutable => self.capabilities(*element),
            },
            TypeKind::HashMap {
                key,
                value,
                mutability,
            } => match mutability {
                CollectionMutability::Mutable => Ok(Capabilities::linear()),
                CollectionMutability::Immutable => {
                    let key_caps = self.capabilities(*key)?;
                    let value_caps = self.capabilities(*value)?;
                    Ok(Capabilities {
                        dup: key_caps.dup && value_caps.dup,
                        zap: key_caps.zap && value_caps.zap,
                    })
                }
            },
            TypeKind::Sum(components) | TypeKind::Product(components) => {
                let mut caps = Capabilities::dup_zap();
                for component in components {
                    let component_caps = self.capabilities(component.ty)?;
                    caps.dup &= component_caps.dup;
                    caps.zap &= component_caps.zap;
                }
                Ok(caps)
            }
        }
    }

    fn validate_type(&self, id: TypeId) -> Result<(), TypeError> {
        if self.types.get(id.index()).is_some() {
            Ok(())
        } else {
            Err(TypeError::UnknownType(id))
        }
    }

    fn validate_components(&self, components: &[Component]) -> Result<(), TypeError> {
        for component in components {
            self.validate_type(component.ty)?;
            if let ComponentName::Named(name) = &component.name {
                validate_name(name)?;
            }
        }
        Ok(())
    }

    fn capabilities_inner(
        &self,
        id: TypeId,
        visiting: &mut Vec<TypeId>,
    ) -> Result<Capabilities, TypeError> {
        if visiting.contains(&id) {
            return Err(TypeError::RecursiveType(id));
        }
        let def = self.get(id).ok_or(TypeError::UnknownType(id))?;
        let declared = def.declared.into_capabilities();
        let structural = match &def.kind {
            TypeKind::Never
            | TypeKind::Unit
            | TypeKind::Finite { .. }
            | TypeKind::Function { .. }
            | TypeKind::Symbol
            | TypeKind::Text => Capabilities::dup_zap(),
            TypeKind::Primitive => Capabilities::linear(),
            TypeKind::List {
                element,
                mutability,
            } => match mutability {
                CollectionMutability::Mutable => Capabilities::linear(),
                CollectionMutability::Immutable => {
                    visiting.push(id);
                    let caps = self.capabilities_inner(*element, visiting)?;
                    visiting.pop();
                    caps
                }
            },
            TypeKind::HashMap {
                key,
                value,
                mutability,
            } => match mutability {
                CollectionMutability::Mutable => Capabilities::linear(),
                CollectionMutability::Immutable => {
                    visiting.push(id);
                    let key_caps = self.capabilities_inner(*key, visiting)?;
                    let value_caps = self.capabilities_inner(*value, visiting)?;
                    visiting.pop();
                    Capabilities {
                        dup: key_caps.dup && value_caps.dup,
                        zap: key_caps.zap && value_caps.zap,
                    }
                }
            },
            TypeKind::Sum(variants) | TypeKind::Product(variants) => {
                visiting.push(id);
                let mut caps = Capabilities::dup_zap();
                for component in variants {
                    let component_caps = self.capabilities_inner(component.ty, visiting)?;
                    caps.dup &= component_caps.dup;
                    caps.zap &= component_caps.zap;
                }
                visiting.pop();
                caps
            }
        };
        if matches!(&def.kind, TypeKind::Primitive) {
            Ok(declared)
        } else {
            Ok(structural)
        }
    }
}

impl Default for TypeStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDef {
    pub name: Option<String>,
    pub kind: TypeKind,
    pub declared: DeclaredCapabilities,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Never,
    Unit,
    Finite {
        values: u128,
    },
    Sum(Vec<Component>),
    Product(Vec<Component>),
    Function {
        input: TypeId,
        output: TypeId,
    },
    Symbol,
    Text,
    List {
        element: TypeId,
        mutability: CollectionMutability,
    },
    HashMap {
        key: TypeId,
        value: TypeId,
        mutability: CollectionMutability,
    },
    Primitive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CollectionMutability {
    Immutable,
    Mutable,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Component {
    pub name: ComponentName,
    pub ty: TypeId,
}

impl Component {
    pub fn named(name: impl Into<String>, ty: TypeId) -> Self {
        Self {
            name: ComponentName::Named(name.into()),
            ty,
        }
    }

    pub fn positional(index: usize, ty: TypeId) -> Self {
        Self {
            name: ComponentName::Index(index),
            ty,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ComponentName {
    Named(String),
    Index(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeclaredCapabilities {
    pub dup: bool,
    pub zap: bool,
}

impl DeclaredCapabilities {
    pub fn linear() -> Self {
        Self::default()
    }

    pub fn dup() -> Self {
        Self {
            dup: true,
            ..Self::default()
        }
    }

    pub fn zap() -> Self {
        Self {
            zap: true,
            ..Self::default()
        }
    }

    pub fn dup_zap() -> Self {
        Self {
            dup: true,
            zap: true,
        }
    }

    fn into_capabilities(self) -> Capabilities {
        Capabilities {
            dup: self.dup,
            zap: self.zap,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub dup: bool,
    pub zap: bool,
}

impl Capabilities {
    pub fn linear() -> Self {
        Self {
            dup: false,
            zap: false,
        }
    }

    pub fn dup_zap() -> Self {
        Self {
            dup: true,
            zap: true,
        }
    }

    fn allows(self, requested: Self) -> bool {
        (!requested.dup || self.dup) && (!requested.zap || self.zap)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeError {
    DuplicateName(String),
    EmptyName,
    UnknownType(TypeId),
    RecursiveType(TypeId),
    ZeroFiniteCardinality,
    UIntTooWide(u32),
    DeclaredCapabilityExceedsStructural {
        declared: Capabilities,
        structural: Capabilities,
    },
}

fn validate_name(name: &str) -> Result<(), TypeError> {
    if name.is_empty() {
        Err(TypeError::EmptyName)
    } else {
        Ok(())
    }
}
