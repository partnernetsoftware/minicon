//! Experimental `.wasm` target: emit a standard WebAssembly module for a small
//! numeric subset of bx, and execute modules through bx's own interpreter
//! (`crate::wasm`). No third-party wasm crates.
//!
//! The subset is deliberately vertical: integer literals, globals (`let` /
//! assignment), `+ - *`, and `print(<int>)` via the `env.print_i64` import.
//! Everything else is rejected with a clear "unsupported in the wasm target"
//! error until the data model (strings/arrays/objects) lands.

use crate::ast::{BinOp, Expr, Stmt};
use crate::wasm::{Builder, Instr, ValType, WVal};

const PRINT_IMPORT: &str = "print_i64";

fn collect_globals(stmts: &[Stmt], names: &mut Vec<String>) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Stmt::Let(name, _, _) => add_name(names, name),
            Stmt::Expr(Expr::Assign(target, _, _), _) => match &**target {
                Expr::Ident(name) => add_name(names, name),
                _ => {
                    return Err("wasm target supports assignment to a global name only".to_string())
                }
            },
            Stmt::Expr(_, _) => {}
            _ => {
                return Err(format!(
                    "wasm target does not support this statement yet: {}",
                    stmt_kind(stmt)
                ))
            }
        }
    }
    Ok(())
}

fn add_name(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|known| known == name) {
        names.push(name.to_string());
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
        Expr::Str(_) => "string",
        Expr::Bool(_) => "bool",
        Expr::Float(_) => "float",
        Expr::Array(_) => "array",
        Expr::Object(_) => "object",
        Expr::Call(..) => "call",
        Expr::MethodCall(..) => "method call",
        Expr::Index(..) => "index",
        Expr::Member(..) => "member",
        Expr::Func(..) => "function",
        Expr::Ternary(..) => "ternary",
        Expr::And(..) | Expr::Or(..) => "logical operator",
        Expr::Compound(..) => "compound assignment",
        Expr::Null => "null",
        _ => "expression",
    }
}

/// Statically known string value: literals and `+` of string values. Anything
/// that needs runtime allocation is rejected until the heap lands (M2.2).
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
    names: &'a [String],
    builder: &'a mut Builder,
}

impl<'a> Emitter<'a> {
    fn global(&self, name: &str) -> Result<u32, String> {
        self.names
            .iter()
            .position(|known| known == name)
            .map(|index| index as u32)
            .ok_or_else(|| format!("undefined variable '{}'", name))
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Int(value) => self.builder.push(Instr::Const(0x42, WVal::I64(*value))),
            Expr::Ident(name) => {
                self.builder.push(Instr::Global(0x23, self.global(name)?));
            }
            Expr::Neg(inner) => {
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.emit_expr(inner)?;
                self.builder.push(Instr::Op(0x7d));
            }
            Expr::Bin(BinOp::Add, left, right) => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                self.builder.push(Instr::Op(0x7c));
            }
            Expr::Bin(BinOp::Sub, left, right) => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                self.builder.push(Instr::Op(0x7d));
            }
            Expr::Bin(BinOp::Mul, left, right) => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                self.builder.push(Instr::Op(0x7e));
            }
            other => {
                return Err(format!(
                    "wasm target does not support this expression yet: {}",
                    expr_kind(other)
                ))
            }
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let(name, init, _) => {
                match init {
                    Some(expr) => self.emit_expr(expr)?,
                    None => self.builder.push(Instr::Const(0x42, WVal::I64(0))),
                }
                self.builder.push(Instr::Global(0x24, self.global(name)?));
            }
            Stmt::Expr(Expr::Assign(target, value, _), _) => {
                let Expr::Ident(name) = &**target else {
                    return Err("wasm target supports assignment to a global name only".to_string());
                };
                self.emit_expr(value)?;
                self.builder.push(Instr::Global(0x24, self.global(name)?));
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
                    return Err(
                        "wasm target supports print(<int>) or print(<string>) with one argument"
                            .to_string(),
                    );
                }
                if let Some(text) = string_value(&args[0]) {
                    let offset = self.builder.add_data(text.as_bytes());
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32(offset as i32)));
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32(text.len() as i32)));
                    self.builder.push(Instr::Call(1));
                } else {
                    self.emit_expr(&args[0])?;
                    self.builder.push(Instr::Call(0));
                    self.builder.push(Instr::Op(0x1a));
                }
            }
            Stmt::Expr(expr, _) => {
                self.emit_expr(expr)?;
                self.builder.push(Instr::Op(0x1a));
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

/// Compile bx source to a standard WebAssembly module (core subset only).
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, String> {
    let stmts = crate::parser::parse(source)?;
    let mut names: Vec<String> = Vec::new();
    collect_globals(&stmts, &mut names)?;

    let mut builder = Builder::new();
    let print_ty = builder.add_type(vec![ValType::I64], vec![ValType::I64]);
    let print_utf8_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![]);
    let start_ty = builder.add_type(vec![], vec![]);
    builder.import_func("env", PRINT_IMPORT, print_ty);
    builder.import_func("env", "print_utf8", print_utf8_ty);
    builder.enable_memory(1);
    for _ in &names {
        builder.add_global(ValType::I64, true, WVal::I64(0));
    }
    builder.set_body_type(start_ty);
    {
        let mut emitter = Emitter {
            names: &names,
            builder: &mut builder,
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

/// Instantiate and run `_start`, forwarding `env.print_i64` and
/// `env.print_utf8` calls to their sinks.
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
    fn strings_print_through_memory() {
        let bytes = compile_to_wasm("print(\"ab\" + \"cd\")").expect("compile");
        validate(&bytes).expect("validate");
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
    fn module_is_standard_wasm() {
        let bytes = compile_to_wasm("print(7)").expect("compile");
        assert_eq!(&bytes[0..4], b"\0asm");
        validate(&bytes).expect("validate");
    }

    #[test]
    fn unsupported_constructs_are_rejected() {
        assert!(compile_to_wasm("let s = \"x\"\nprint(s)").is_err());
        assert!(compile_to_wasm("fn f() { return 1 }").is_err());
        assert!(compile_to_wasm("print(unknown)").is_err());
    }
}
