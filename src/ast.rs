#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    Bool,
    Unknown,
}

impl TypeKind {
    pub fn max_value(&self) -> u64 {
        match self {
            TypeKind::U8      => 255,
            TypeKind::U16     => 65535,
            TypeKind::U32     => 4294967295,
            TypeKind::U64     => u64::MAX,
            TypeKind::I8      => 127,
            TypeKind::I16     => 32767,
            TypeKind::I32     => 2147483647,
            TypeKind::I64     => i64::MAX as u64,
            TypeKind::Bool    => 1,
            TypeKind::Unknown => 4294967295,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            TypeKind::U8      => "u8",
            TypeKind::U16     => "u16",
            TypeKind::U32     => "u32",
            TypeKind::U64     => "u64",
            TypeKind::I8      => "i8",
            TypeKind::I16     => "i16",
            TypeKind::I32     => "i32",
            TypeKind::I64     => "i64",
            TypeKind::Bool    => "bool",
            TypeKind::Unknown => "u32",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Statement {
    Let(String, Expression, TypeKind),
    Root(String, Expression, TypeKind),
    Bnw(String),
    Assignment(String, Expression),
    ArrayAssign(String, Expression, Expression),
    Loop(Vec<Statement>),
    While(Expression, Vec<Statement>),
    If(Expression, Vec<Statement>, Option<Vec<Statement>>),
    FunctionDefine(String, Vec<(String, TypeKind)>, Vec<Statement>, TypeKind),
    Call(String, Vec<Expression>),
    Return(Option<Expression>),
    Asm(String),
    Outb(Expression, Expression),
    Poke(Expression, Expression),
    Break,
    CallPtr(Expression),
    ArrayDefine(String, Vec<u64>, TypeKind),
    StringDefine(String, String),
    StructDefine(String, Vec<(String, TypeKind)>),
    StructInstance(String, String), 
    IntHandler(String, Vec<Statement>),
    IntEnable(Expression, String),
    IntDisable,
    SaveContext(String),
    RestoreContext(String),
}

#[derive(Debug, Clone)]
pub enum Expression {
    Number(u64, TypeKind),
    Variable(String),
    BinaryOp(Box<Expression>, String, Box<Expression>),
    WaitKey,
    Inb(Box<Expression>),
    Peek(Box<Expression>),
    Call(String, Vec<Expression>),
    ArrayAccess(String, Box<Expression>),
    FieldAccess(String, String),
    FieldAssign(String, String, Box<Expression>),
    AddressOf(String),
    Syscall(Vec<Expression>),
}