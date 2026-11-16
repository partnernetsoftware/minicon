//! Experimental `.wasm` target: emit a standard WebAssembly module for a small
//! numeric subset of bx, and execute modules through an embedded wasmi host.
//!
//! The subset is deliberately vertical: integer literals, globals (`let` /
//! assignment), `+ - *`, and `print(<int>)` via the `env.print_i64` import.
//! Everything else is rejected with a clear "unsupported in the wasm target"
//! error until the data model (strings/arrays/objects) lands.

use crate::ast::{BinOp, Expr, Stmt};
use wasm_encoder::{
    CodeSection, ConstExpr, EntityType, ExportKind, ExportSection, Function, FunctionSection,
    GlobalSection, GlobalType, ImportSection, Instruction, Module, TypeSection, ValType,
};

const PRINT_IMPORT: &str = "print_i64";

fn collect_globals(stmts: &[Stmt], names: &mut Vec<String>) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Stmt::Let(name, _, _) => add_name(names, name),
            Stmt::Expr(Expr::Assign(target, _, _), _) => match &**target {
                Expr::Ident(name) => add_name(names, name),
                _ => {
                    return Err(
                        "wasm target supports assignment to a global name only".to_string(),
                    )
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

struct Emitter<'a> {
    names: &'a [String],
    print_ty: u32,
    start_ty: u32,
}

impl<'a> Emitter<'a> {
    fn global(&self, name: &str) -> Result<u32, String> {
        self.names
            .iter()
            .position(|known| known == name)
            .map(|index| index as u32)
            .ok_or_else(|| format!("undefined variable '{}'", name))
    }

    fn emit_expr(&self, expr: &Expr, body: &mut Function) -> Result<(), String> {
        match expr {
            Expr::Int(value) => {
                body.instruction(&Instruction::I64Const(*value));
            }
            Expr::Ident(name) => {
                body.instruction(&Instruction::GlobalGet(self.global(name)?));
            }
            Expr::Neg(inner) => {
                body.instruction(&Instruction::I64Const(0));
                self.emit_expr(inner, body)?;
                body.instruction(&Instruction::I64Sub);
            }
            Expr::Bin(BinOp::Add, left, right) => {
                self.emit_expr(left, body)?;
                self.emit_expr(right, body)?;
                body.instruction(&Instruction::I64Add);
            }
            Expr::Bin(BinOp::Sub, left, right) => {
                self.emit_expr(left, body)?;
                self.emit_expr(right, body)?;
                body.instruction(&Instruction::I64Sub);
            }
            Expr::Bin(BinOp::Mul, left, right) => {
                self.emit_expr(left, body)?;
                self.emit_expr(right, body)?;
                body.instruction(&Instruction::I64Mul);
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

    fn emit_stmt(&self, stmt: &Stmt, body: &mut Function) -> Result<(), String> {
        match stmt {
            Stmt::Let(name, init, _) => {
                match init {
                    Some(expr) => self.emit_expr(expr, body)?,
                    None => {
                        body.instruction(&Instruction::I64Const(0));
                    }
                }
                body.instruction(&Instruction::GlobalSet(self.global(name)?));
            }
            Stmt::Expr(Expr::Assign(target, value, _), _) => {
                let Expr::Ident(name) = &**target else {
                    return Err("wasm target supports assignment to a global name only".to_string());
                };
                self.emit_expr(value, body)?;
                body.instruction(&Instruction::GlobalSet(self.global(name)?));
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
                    return Err("wasm target supports print(<int>) with one argument".to_string());
                }
                self.emit_expr(&args[0], body)?;
                body.instruction(&Instruction::Call(0));
                body.instruction(&Instruction::Drop);
            }
            Stmt::Expr(expr, _) => {
                self.emit_expr(expr, body)?;
                body.instruction(&Instruction::Drop);
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

/// Compile bx source to a standard WebAssembly module (core subset only).
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, String> {
    let stmts = crate::parser::parse(source)?;
    let mut names: Vec<String> = Vec::new();
    collect_globals(&stmts, &mut names)?;

    let mut types = TypeSection::new();
    // Type indices follow declaration order.
    let print_ty: u32 = 0;
    let start_ty: u32 = 1;
    types.ty().function([ValType::I64], [ValType::I64]);
    types.ty().function([], []);

    let mut imports = ImportSection::new();
    imports.import("env", PRINT_IMPORT, EntityType::Function(print_ty));

    let mut functions = FunctionSection::new();
    functions.function(start_ty);

    let mut globals = GlobalSection::new();
    for _ in &names {
        globals.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i64_const(0),
        );
    }

    let mut exports = ExportSection::new();
    exports.export("_start", ExportKind::Func, 1);

    let emitter = Emitter {
        names: &names,
        print_ty,
        start_ty,
    };
    let mut body = Function::new([]);
    for stmt in &stmts {
        emitter.emit_stmt(stmt, &mut body)?;
    }
    body.instruction(&Instruction::End);

    let mut code = CodeSection::new();
    code.function(&body);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&functions);
    module.section(&globals);
    module.section(&exports);
    module.section(&code);
    let _ = (print_ty, start_ty);
    Ok(module.finish())
}

/// Parse and validate a module without running it.
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    let engine = wasmi::Engine::default();
    wasmi::Module::new(&engine, bytes)
        .map(|_| ())
        .map_err(|error| format!("invalid wasm module: {error}"))
}

/// Instantiate and run `_start`, forwarding every `env.print_i64` call.
pub fn execute<F>(bytes: &[u8], sink: F) -> Result<(), String>
where
    F: FnMut(i64) + Send + Sync + 'static,
{
    use std::sync::{Arc, Mutex};
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, bytes)
        .map_err(|error| format!("invalid wasm module: {error}"))?;
    let mut store = wasmi::Store::new(&engine, ());
    let mut linker = wasmi::Linker::new(&engine);
    let sink = Arc::new(Mutex::new(sink));
    let callback = sink.clone();
    linker
        .func_wrap(
            "env",
            PRINT_IMPORT,
            move |_caller: wasmi::Caller<'_, ()>, value: i64| -> i64 {
                if let Ok(mut sink) = callback.lock() {
                    sink(value);
                }
                // Echo the value so the import type is (i64) -> (i64).
                value
            },
        )
        .map_err(|error| format!("cannot register env.print_i64: {error}"))?;
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|error| format!("cannot instantiate wasm module: {error}"))?
        .start(&mut store)
        .map_err(|error| format!("wasm start section trapped: {error}"))?;
    let start = instance
        .get_typed_func::<(), ()>(&store, "_start")
        .map_err(|error| format!("module has no _start export: {error}"))?;
    start
        .call(&mut store, ())
        .map_err(|error| format!("wasm trap: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn run_source(source: &str) -> Vec<i64> {
        let bytes = compile_to_wasm(source).expect("compile");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        execute(&bytes, move |value| {
            sink.lock().expect("lock").push(value);
        })
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
    fn module_is_standard_wasm() {
        let bytes = compile_to_wasm("print(7)").expect("compile");
        assert_eq!(&bytes[0..4], b"\0asm");
        validate(&bytes).expect("validate");
    }

    #[test]
    fn unsupported_constructs_are_rejected() {
        assert!(compile_to_wasm("print(\"hi\")").is_err());
        assert!(compile_to_wasm("fn f() { return 1 }").is_err());
        assert!(compile_to_wasm("print(unknown)").is_err());
    }
}