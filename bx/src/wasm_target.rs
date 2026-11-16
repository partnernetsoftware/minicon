//! Experimental `.wasm` target: emit a standard WebAssembly module for a small
//! subset of bx, executed by bx's own interpreter (`crate::wasm`). No
//! third-party wasm crates.
//!
//! Current subset: i64 numbers (`let`/assignment/`+ - *`), compilation-time
//! string literals (data segments, `env.print_utf8`), and integer arrays
//! `[a, b, c]` with `arr[i]` / `len(arr)` backed by the prototype `env.alloc`
//! bump allocator. Everything else is rejected explicitly.

use crate::ast::{BinOp, Expr, Stmt};
use crate::wasm::{Builder, Instr, ValType, WVal};
use std::collections::HashMap;

/// Static type of a value in the wasm subset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    Int,
    Str,
    Arr,
}

fn print_ty(ty: Ty) -> &'static str {
    match ty {
        Ty::Int => "int",
        Ty::Str => "string",
        Ty::Arr => "array",
    }
}

fn stmt_kind(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::Fn(..) => "fn",
        Stmt::Block(_) => "block",
        Stmt::If(..) => "if",
        Stmt::While(..) => "while",
        Stmt::ForC(..) => "for",
        Stmt::ForIn(..) => "for-in",
        Stmt::Return(..) => "return",
        Stmt::Break(_) => "break",
        Stmt::Continue(_) => "continue",
        Stmt::Use(..) => "use",
        _ => "statement",
    }
}

fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Int(_) => "int",
        Expr::Null => "null",
        Expr::Bool(_) => "bool",
        Expr::Float(_) => "float",
        Expr::Str(_) => "string",
        Expr::Ident(_) => "identifier",
        Expr::Array(_) => "array",
        Expr::Object(_) => "object",
        Expr::Neg(_) => "negation",
        Expr::Not(_) => "not",
        Expr::Bin(..) => "binary operator",
        Expr::Ternary(..) => "ternary",
        Expr::And(..) | Expr::Or(..) => "logical operator",
        Expr::Call(..) => "call",
        Expr::MethodCall(..) => "method call",
        Expr::Index(..) => "index",
        Expr::Member(..) => "member",
        Expr::Assign(..) => "assignment",
        Expr::Compound(..) => "compound assignment",
        Expr::Func(..) => "function",
    }
}

struct Binding {
    ty: Ty,
    index: u32,
}

struct Checker {
    env: HashMap<String, Binding>,
    order: Vec<String>,
}

impl Checker {
    fn new() -> Checker {
        Checker {
            env: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn declare(&mut self, name: &str, ty: Ty) -> Result<u32, String> {
        if self.env.contains_key(name) {
            return Err(format!(
                "wasm target does not support redeclaring '{}'",
                name
            ));
        }
        let index = self.order.len() as u32;
        self.env.insert(name.to_string(), Binding { ty, index });
        self.order.push(name.to_string());
        Ok(index)
    }

    fn lookup(&self, name: &str) -> Result<Ty, String> {
        self.env
            .get(name)
            .map(|binding| binding.ty)
            .ok_or_else(|| format!("undefined variable '{}'", name))
    }

    fn index_of(&self, name: &str) -> Result<u32, String> {
        self.env
            .get(name)
            .map(|binding| binding.index)
            .ok_or_else(|| format!("undefined variable '{}'", name))
    }

    fn infer(&self, expr: &Expr) -> Result<Ty, String> {
        match expr {
            Expr::Int(_) => Ok(Ty::Int),
            Expr::Str(_) => Ok(Ty::Str),
            Expr::Ident(name) => self.lookup(name),
            Expr::Neg(inner) => {
                let ty = self.infer(inner)?;
                if ty != Ty::Int {
                    return Err(format!(
                        "wasm target supports arithmetic on ints only, got {}",
                        print_ty(ty)
                    ));
                }
                Ok(Ty::Int)
            }
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), left, right) => {
                let lt = self.infer(left)?;
                let rt = self.infer(right)?;
                if lt == Ty::Str || rt == Ty::Str {
                    // String concatenation is only allowed directly inside
                    // print(), where it can be folded at compile time.
                    return Err(
                        "wasm target supports string '+' only inside print() for now".to_string(),
                    );
                }
                if lt != Ty::Int || rt != Ty::Int {
                    return Err(format!(
                        "wasm target does not support {:?} on {} values yet",
                        op,
                        print_ty(if lt != Ty::Int { lt } else { rt })
                    ));
                }
                Ok(Ty::Int)
            }
            Expr::Array(items) => {
                for item in items {
                    let ty = self.infer(item)?;
                    if ty != Ty::Int {
                        return Err(format!(
                            "wasm target supports arrays of ints only, got {}",
                            print_ty(ty)
                        ));
                    }
                }
                Ok(Ty::Arr)
            }
            Expr::Index(array, index, _) => {
                let arr_ty = self.infer(array)?;
                if arr_ty != Ty::Arr {
                    return Err("wasm target can index arrays only".to_string());
                }
                let idx_ty = self.infer(index)?;
                if idx_ty != Ty::Int {
                    return Err("wasm target needs an int index".to_string());
                }
                Ok(Ty::Int)
            }
            Expr::Call(callee, args, _) => {
                if let Expr::Ident(name) = &**callee {
                    if name == "len" && args.len() == 1 {
                        let ty = self.infer(&args[0])?;
                        if ty == Ty::Arr {
                            return Ok(Ty::Int);
                        }
                    }
                }
                Err(format!(
                    "wasm target does not support this call yet: {}",
                    expr_kind(expr)
                ))
            }
            other => Err(format!(
                "wasm target does not support this expression yet: {}",
                expr_kind(other)
            )),
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let(name, init, _) => {
                let ty = match init {
                    Some(expr) => self.infer(expr)?,
                    None => Ty::Int,
                };
                if ty == Ty::Str {
                    return Err(
                        "wasm target cannot bind runtime strings yet; print them directly"
                            .to_string(),
                    );
                }
                self.declare(name, ty)?;
            }
            Stmt::Expr(Expr::Assign(target, value, _), _) => {
                let Expr::Ident(name) = &**target else {
                    return Err("wasm target supports assignment to a global name only".to_string());
                };
                let expected = self.lookup(name)?;
                let actual = self.infer(value)?;
                if expected != actual {
                    return Err(format!(
                        "wasm target requires stable types: '{}' is {}, assigned {}",
                        name,
                        print_ty(expected),
                        print_ty(actual)
                    ));
                }
            }
            Stmt::Expr(Expr::Call(callee, args, _), _) => {
                let Expr::Ident(name) = &**callee else {
                    return Err("wasm target only supports calls to print()".to_string());
                };
                if name != "print" {
                    return Err(format!(
                        "wasm target does not support calling '{}' yet",
                        name
                    ));
                }
                if args.len() != 1 {
                    return Err("wasm target supports print(<value>) with one argument".to_string());
                }
                if string_value(&args[0]).is_none() {
                    let ty = self.infer(&args[0])?;
                    if ty != Ty::Int {
                        return Err(format!(
                            "wasm target cannot print {} values yet",
                            print_ty(ty)
                        ));
                    }
                }
            }
            Stmt::Expr(expr, _) => {
                self.infer(expr)?;
            }
            other => {
                return Err(format!(
                    "wasm target does not support this statement yet: {}",
                    stmt_kind(other)
                ))
            }
        }
        Ok(())
    }
}

/// Statically known string value: literals and `+` of string values.
fn string_value(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Str(text) => Some(text.clone()),
        Expr::Bin(BinOp::Add, left, right) => {
            Some(format!("{}{}", string_value(left)?, string_value(right)?))
        }
        _ => None,
    }
}

struct Emitter<'a> {
    checker: &'a Checker,
    builder: &'a mut Builder,
    local: u32,
}

impl<'a> Emitter<'a> {
    fn push_global(&mut self, name: &str, set: bool) -> Result<(), String> {
        let index = self.checker.index_of(name)?;
        self.builder
            .push(Instr::Global(if set { 0x24 } else { 0x23 }, index));
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<Ty, String> {
        match expr {
            Expr::Int(value) => {
                self.builder.push(Instr::Const(0x42, WVal::I64(*value)));
                Ok(Ty::Int)
            }
            Expr::Str(_) => Err("wasm target string values need print() context".to_string()),
            Expr::Ident(name) => {
                let ty = self.checker.lookup(name)?;
                if ty == Ty::Str {
                    return Err("wasm target cannot use runtime strings yet".to_string());
                }
                self.push_global(name, false)?;
                Ok(ty)
            }
            Expr::Neg(inner) => {
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.emit_expr(inner)?;
                self.builder.push(Instr::Op(0x7d));
                Ok(Ty::Int)
            }
            Expr::Bin(op, left, right) => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                let opcode = match op {
                    BinOp::Add => 0x7c,
                    BinOp::Sub => 0x7d,
                    BinOp::Mul => 0x7e,
                    _ => return Err("wasm target supports only + - *".to_string()),
                };
                self.builder.push(Instr::Op(opcode));
                Ok(Ty::Int)
            }
            Expr::Array(items) => {
                let count = items.len() as u32;
                let size = 4 + 8 * count;
                self.builder
                    .push(Instr::Const(0x41, WVal::I32(size as i32)));
                self.builder.push(Instr::Call(2));
                self.builder.push(Instr::Local(0x21, self.local));
                self.builder.push(Instr::Local(0x20, self.local));
                self.builder
                    .push(Instr::Const(0x41, WVal::I32(count as i32)));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                for (index, item) in items.iter().enumerate() {
                    self.builder.push(Instr::Local(0x20, self.local));
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32((4 + 8 * index as u32) as i32)));
                    self.builder.push(Instr::Op(0x6a));
                    self.emit_expr(item)?;
                    self.builder.push(Instr::Mem(0x37, 3, 0));
                }
                self.builder.push(Instr::Local(0x20, self.local));
                Ok(Ty::Arr)
            }
            Expr::Index(array, index, _) => {
                self.emit_expr(array)?;
                self.emit_expr(index)?;
                self.builder.push(Instr::Op(0xa7));
                self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                self.builder.push(Instr::Op(0x6c));
                self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Mem(0x29, 3, 0));
                Ok(Ty::Int)
            }
            Expr::Call(callee, args, _) => {
                if let Expr::Ident(name) = &**callee {
                    if name == "len" && args.len() == 1 {
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                }
                Err("wasm target does not support this call yet".to_string())
            }
            other => Err(format!(
                "wasm target does not support this expression yet: {}",
                expr_kind(other)
            )),
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let(name, init, _) => {
                match init {
                    Some(expr) => {
                        self.emit_expr(expr)?;
                    }
                    None => self.builder.push(Instr::Const(0x42, WVal::I64(0))),
                }
                self.push_global(name, true)
            }
            Stmt::Expr(Expr::Assign(target, value, _), _) => {
                let Expr::Ident(name) = &**target else {
                    return Err("wasm target supports assignment to a global name only".to_string());
                };
                self.emit_expr(value)?;
                self.push_global(name, true)
            }
            Stmt::Expr(Expr::Call(callee, args, _), _) => {
                let Expr::Ident(name) = &**callee else {
                    return Err("wasm target only supports calls to print()".to_string());
                };
                if name != "print" || args.len() != 1 {
                    return Err("wasm target supports print(<value>) with one argument".to_string());
                }
                if let Some(text) = string_value(&args[0]) {
                    let offset = self.builder.add_data(text.as_bytes());
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32(offset as i32)));
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32(text.len() as i32)));
                    self.builder.push(Instr::Call(1));
                    return Ok(());
                }
                self.emit_expr(&args[0])?;
                self.builder.push(Instr::Call(0));
                self.builder.push(Instr::Op(0x1a));
                Ok(())
            }
            Stmt::Expr(expr, _) => {
                self.emit_expr(expr)?;
                let ty = self.checker.infer(expr)?;
                match ty {
                    Ty::Int => self.builder.push(Instr::Op(0x1a)),
                    Ty::Arr => self.builder.push(Instr::Op(0x1a)),
                    Ty::Str => return Err("wasm target cannot use runtime strings yet".to_string()),
                }
                Ok(())
            }
            other => Err(format!(
                "wasm target does not support this statement yet: {}",
                stmt_kind(other)
            )),
        }
    }
}

/// Compile bx source to a standard WebAssembly module (current subset only).
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, String> {
    let stmts = crate::parser::parse(source)?;
    let mut checker = Checker::new();
    for stmt in &stmts {
        checker.check_stmt(stmt)?;
    }

    let mut builder = Builder::new();
    let print_i64_ty = builder.add_type(vec![ValType::I64], vec![ValType::I64]);
    let print_utf8_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![]);
    let alloc_ty = builder.add_type(vec![ValType::I32], vec![ValType::I32]);
    let start_ty = builder.add_type(vec![], vec![]);
    builder.import_func("env", "print_i64", print_i64_ty);
    builder.import_func("env", "print_utf8", print_utf8_ty);
    builder.import_func("env", "alloc", alloc_ty);
    builder.enable_memory(1);
    for name in &checker.order {
        let ty = checker.lookup(name)?;
        match ty {
            Ty::Int => builder.add_global(ValType::I64, true, WVal::I64(0)),
            Ty::Arr => builder.add_global(ValType::I32, true, WVal::I32(0)),
            Ty::Str => return Err("wasm target cannot bind runtime strings yet".to_string()),
        }
    }
    builder.set_body_type(start_ty);
    let local = builder.add_local(ValType::I32);
    {
        let mut emitter = Emitter {
            checker: &checker,
            builder: &mut builder,
            local,
        };
        for stmt in &stmts {
            emitter.emit_stmt(stmt)?;
        }
    }
    builder.finish("_start")
}

/// Parse and validate a module without running it.
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    crate::wasm::validate(bytes).map(|_| ())
}

/// Instantiate and run `_start`, forwarding `env.print_i64` / `env.print_utf8`.
pub fn execute<F, G>(bytes: &[u8], on_i64: F, on_str: G) -> Result<(), String>
where
    F: FnMut(i64),
    G: FnMut(&str),
{
    crate::wasm::run_start(bytes, on_i64, on_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn run_source(source: &str) -> Vec<i64> {
        let bytes = compile_to_wasm(source).expect("compile");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        execute(
            &bytes,
            move |value| {
                sink.lock().expect("lock").push(value);
            },
            |_text| {},
        )
        .expect("execute");
        let out = seen.lock().expect("lock").clone();
        out
    }

    #[test]
    fn numeric_script_prints_in_order() {
        let prints = run_source("let x = 2\nprint(x + 3)\nx = x * 2\nprint(x)\nprint(-x + 1)");
        assert_eq!(prints, vec![5, 4, -3]);
    }

    #[test]
    fn arrays_use_linear_memory() {
        let prints = run_source("let a = [10, 20, 30]\nprint(a[0] + a[2])\nprint(len(a))");
        assert_eq!(prints, vec![40, 3]);
    }

    #[test]
    fn module_is_standard_wasm() {
        let bytes = compile_to_wasm("print(7)").expect("compile");
        assert_eq!(&bytes[0..4], b"\0asm");
        validate(&bytes).expect("validate");
    }

    #[test]
    fn strings_print_through_memory() {
        let bytes = compile_to_wasm("print(\"ab\" + \"cd\")").expect("compile");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        execute(
            &bytes,
            |_value| {},
            move |text| {
                sink.lock().expect("lock").push(text.to_string());
            },
        )
        .expect("execute");
        assert_eq!(*seen.lock().expect("lock"), vec!["abcd".to_string()]);
    }

    #[test]
    fn unsupported_constructs_are_rejected() {
        assert!(compile_to_wasm("fn f() { return 1 }").is_err());
        assert!(compile_to_wasm("print(unknown)").is_err());
        assert!(compile_to_wasm("let s = \"x\"\nprint(s)").is_err());
        assert!(compile_to_wasm("let a = [1, \"x\"]").is_err());
    }
}
