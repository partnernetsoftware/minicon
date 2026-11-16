#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>, u32),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>, u32),
    MethodCall(Box<Expr>, String, Vec<Expr>, u32),
    Index(Box<Expr>, Box<Expr>, u32),
    Member(Box<Expr>, String, u32),
    Assign(Box<Expr>, Box<Expr>, u32),
    Compound(Box<Expr>, BinOp, Box<Expr>, u32),
    Func(Vec<String>, Vec<Stmt>, u32),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Expr(Expr, u32),
    Let(String, Option<Expr>, u32),
    Fn(String, Vec<String>, Vec<Stmt>, u32),
    Block(Vec<Stmt>),
    If(Expr, Vec<Stmt>, Option<Vec<Stmt>>, u32),
    While(Expr, Vec<Stmt>, u32),
    ForC(
        Option<Box<Stmt>>,
        Option<Expr>,
        Option<Expr>,
        Vec<Stmt>,
        u32,
    ),
    ForIn(String, Option<String>, Expr, Vec<Stmt>, u32),
    Return(Option<Expr>, u32),
    Break(u32),
    Continue(u32),
    Use(String, u32),
}

impl Expr {
    pub fn is_lvalue(&self) -> bool {
        matches!(self, Expr::Ident(_) | Expr::Member(..) | Expr::Index(..))
    }
}
