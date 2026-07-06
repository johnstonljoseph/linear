#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Type(TypeDef),
    Struct(TypeDef),
    Enum(TypeDef),
    Global(GlobalDef),
    Function(FunctionDef),
    Impl(ImplBlock),
    Trait(TraitDef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDef {
    pub name: String,
    pub generics: Vec<String>,
    pub ty: TypeExpr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalDef {
    pub name: String,
    pub ty: TypeExpr,
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionDef {
    pub name: String,
    pub generics: Vec<String>,
    pub params: Vec<Param>,
    pub output: TypeExpr,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplBlock {
    pub generics: Vec<String>,
    pub trait_ref: Option<TypeExpr>,
    pub target: TypeExpr,
    pub methods: Vec<FunctionDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraitDef {
    pub name: String,
    pub generics: Vec<String>,
    pub methods: Vec<FunctionSig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSig {
    pub name: String,
    pub generics: Vec<String>,
    pub params: Vec<Param>,
    pub output: TypeExpr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub flow: ValueFlow,
    pub name: String,
    pub ty: TypeExpr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValueFlow {
    #[default]
    ReturnedUnchanged,
    ReturnedChanged,
    NotReturned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeExpr {
    Unit,
    Name(String),
    Apply {
        name: String,
        args: Vec<TypeExpr>,
    },
    Product(Vec<Field<TypeExpr>>),
    Sum(Vec<Field<TypeExpr>>),
    Function {
        input: Box<TypeExpr>,
        output: Box<TypeExpr>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field<T> {
    pub name: Option<String>,
    pub value: T,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub lets: Vec<LetStmt>,
    pub result: Option<Box<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetStmt {
    pub pattern: Pattern,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    Name(String),
    Wildcard,
    Unit,
    Tuple(Vec<Pattern>),
    Record(Vec<Field<Pattern>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Name(String),
    Int(u128),
    String(String),
    Unit,
    Block(Block),
    Product(Vec<Field<Expr>>),
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    MethodCall {
        receiver: Box<Expr>,
        receiver_flow: ValueFlow,
        method: String,
        args: Vec<Arg>,
    },
    FieldAccess {
        receiver: Box<Expr>,
        field: String,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Block,
        else_branch: Block,
    },
    Binary {
        lhs: Box<Expr>,
        op: BinaryOp,
        rhs: Box<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arg {
    pub flow: ValueFlow,
    pub label: Option<String>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArm {
    pub variant: String,
    pub payload: Option<Pattern>,
    pub body: Expr,
}
