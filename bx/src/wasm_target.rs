//! Experimental `.wasm` target: emit a standard WebAssembly module for a small
//! subset of bx, executed by bx's own interpreter (`crate::wasm`). No
//! third-party wasm crates.
//!
//! Current subset: i64 numbers (`let`/assignment/`+ - *`), strings (literals,
//! bindings and runtime concatenation over linear memory), and integer arrays
//! `[a, b, c]` with `arr[i]` / `len(arr)` backed by the prototype `env.alloc`
//! bump allocator. Everything else is rejected explicitly.

use crate::ast::{BinOp, Expr, Stmt};
use crate::wasm::{Builder, Instr, ValType, WVal};
use std::cell::RefCell;
use std::collections::HashMap;

/// Static type of a value in the wasm subset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    Int,
    Float,
    Str,
    Arr,
    /// Fixed-layout record; the payload indexes `Checker::shapes`.
    Obj(u32),
}

fn ty_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Int => "int",
        Ty::Float => "float",
        Ty::Str => "string",
        Ty::Arr => "array",
        Ty::Obj(_) => "object",
    }
}

fn is_number(ty: Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float)
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
    /// First global index; a string binding owns two slots (ptr, len).
    base: u32,
}

#[derive(Clone, Debug)]
struct Shape {
    keys: Vec<String>,
}

#[derive(Clone, Copy)]
struct FnSig {
    params: usize,
    returns: bool,
    /// Index in the wasm function index space (imports first).
    index: u32,
}

struct Checker {
    env: HashMap<String, Binding>,
    order: Vec<String>,
    globals_used: u32,
    shapes: RefCell<Vec<Shape>>,
    fns: HashMap<String, FnSig>,
    /// Overlay for the function body currently being checked.
    local_scope: HashMap<String, Ty>,
    fn_depth: usize,
}

impl Checker {
    fn new() -> Checker {
        Checker {
            env: HashMap::new(),
            order: Vec::new(),
            globals_used: 0,
            shapes: RefCell::new(Vec::new()),
            fns: HashMap::new(),
            local_scope: HashMap::new(),
            fn_depth: 0,
        }
    }

    fn declare_fn(&mut self, name: &str, params: usize, returns: bool) -> Result<(), String> {
        if self.fns.contains_key(name) {
            return Err(format!(
                "wasm target does not support redeclaring function '{}'",
                name
            ));
        }
        if self.env.contains_key(name) {
            return Err(format!(
                "wasm target name '{}' is already a value binding",
                name
            ));
        }
        // imports + start (1) + previously declared user functions.
        let index = IMPORT_COUNT + 1 + self.fns.len() as u32;
        self.fns.insert(
            name.to_string(),
            FnSig {
                params,
                returns,
                index,
            },
        );
        Ok(())
    }

    fn fn_sig(&self, name: &str) -> Option<FnSig> {
        self.fns.get(name).copied()
    }

    fn declare_let(&mut self, name: &str, ty: Ty) -> Result<(), String> {
        if self.fn_depth > 0 {
            self.local_scope.insert(name.to_string(), ty);
            return Ok(());
        }
        self.declare(name, ty)
    }

    fn shape_id(&self, keys: &[String]) -> u32 {
        let mut shapes = self.shapes.borrow_mut();
        if let Some(index) = shapes.iter().position(|shape| shape.keys == keys) {
            return index as u32;
        }
        shapes.push(Shape {
            keys: keys.to_vec(),
        });
        (shapes.len() - 1) as u32
    }

    fn field_index(&self, shape: u32, key: &str) -> Result<usize, String> {
        let shapes = self.shapes.borrow();
        let shape = shapes
            .get(shape as usize)
            .ok_or_else(|| "unknown object shape".to_string())?;
        shape
            .keys
            .iter()
            .position(|known| known == key)
            .ok_or_else(|| format!("object has no field '{}'", key))
    }

    fn declare(&mut self, name: &str, ty: Ty) -> Result<(), String> {
        if self.env.contains_key(name) {
            return Err(format!(
                "wasm target does not support redeclaring '{}'",
                name
            ));
        }
        let base = self.globals_used;
        self.globals_used += if ty == Ty::Str { 2 } else { 1 };
        self.env.insert(name.to_string(), Binding { ty, base });
        self.order.push(name.to_string());
        Ok(())
    }

    fn binding(&self, name: &str) -> Result<&Binding, String> {
        self.env
            .get(name)
            .ok_or_else(|| format!("undefined variable '{}'", name))
    }

    fn lookup(&self, name: &str) -> Result<Ty, String> {
        if self.fn_depth > 0 {
            if let Some(ty) = self.local_scope.get(name) {
                return Ok(*ty);
            }
            if let Some(binding) = self.env.get(name) {
                return Ok(binding.ty);
            }
            return Err(format!("undefined variable '{}'", name));
        }
        self.binding(name).map(|binding| binding.ty)
    }

    fn infer(&self, expr: &Expr) -> Result<Ty, String> {
        match expr {
            Expr::Int(_) => Ok(Ty::Int),
            Expr::Float(_) => Ok(Ty::Float),
            Expr::Str(_) => Ok(Ty::Str),
            Expr::Ident(name) => self.lookup(name),
            Expr::Neg(inner) => {
                let ty = self.infer(inner)?;
                if ty != Ty::Int {
                    return Err(format!(
                        "wasm target supports arithmetic on ints only, got {}",
                        ty_name(ty)
                    ));
                }
                Ok(Ty::Int)
            }
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), left, right) => {
                let lt = self.infer(left)?;
                let rt = self.infer(right)?;
                if op == &BinOp::Add && lt == Ty::Str && rt == Ty::Str {
                    return Ok(Ty::Str);
                }
                match (lt, rt) {
                    (Ty::Int, Ty::Int) => Ok(Ty::Int),
                    (Ty::Int | Ty::Float, Ty::Int | Ty::Float) => Ok(Ty::Float),
                    (Ty::Arr, _) | (_, Ty::Arr) => Err(format!(
                        "wasm target does not support {:?} on arrays yet",
                        op
                    )),
                    _ => Err(format!(
                        "wasm target cannot mix {} and {} in {:?} yet",
                        ty_name(lt),
                        ty_name(rt),
                        op
                    )),
                }
            }
            Expr::Bin(BinOp::Div, left, right) => {
                let lt = self.infer(left)?;
                let rt = self.infer(right)?;
                if !is_number(lt) || !is_number(rt) {
                    return Err("wasm target divides numbers only".to_string());
                }
                Ok(Ty::Float)
            }
            Expr::Bin(BinOp::Mod, left, right) => {
                let lt = self.infer(left)?;
                let rt = self.infer(right)?;
                if !is_number(lt) || !is_number(rt) {
                    return Err("wasm target takes % of numbers only".to_string());
                }
                if lt == Ty::Int && rt == Ty::Int {
                    Ok(Ty::Int)
                } else {
                    Ok(Ty::Float)
                }
            }
            Expr::Bin(..) => Err("wasm target supports + - * / %".to_string()),
            Expr::Object(fields) => {
                if fields.is_empty() {
                    return Err("wasm target does not support empty objects yet".to_string());
                }
                let mut keys: Vec<String> = Vec::new();
                for (key, value) in fields {
                    if keys.iter().any(|seen| seen == key) {
                        return Err(format!("duplicate object key '{}'", key));
                    }
                    let ty = self.infer(value)?;
                    if ty != Ty::Int {
                        return Err(format!(
                            "wasm target supports int object fields only, got {}",
                            ty_name(ty)
                        ));
                    }
                    keys.push(key.clone());
                }
                Ok(Ty::Obj(self.shape_id(&keys)))
            }
            Expr::Member(object, key, _) => {
                let ty = self.infer(object)?;
                match ty {
                    Ty::Obj(shape) => {
                        self.field_index(shape, key)?;
                        Ok(Ty::Int)
                    }
                    other => Err(format!(
                        "wasm target can read fields of objects only, got {}",
                        ty_name(other)
                    )),
                }
            }
            Expr::Array(items) => {
                for item in items {
                    let ty = self.infer(item)?;
                    if ty != Ty::Int {
                        return Err(format!(
                            "wasm target supports arrays of ints only, got {}",
                            ty_name(ty)
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
                    if let Some(sig) = self.fn_sig(name) {
                        if !sig.returns {
                            return Err(format!(
                                "wasm target function '{}' returns no value",
                                name
                            ));
                        }
                        if args.len() != sig.params {
                            return Err(format!(
                                "wasm target function '{}' takes {} arguments",
                                name, sig.params
                            ));
                        }
                        for arg in args {
                            if self.infer(arg)? != Ty::Int {
                                return Err(
                                    "wasm target function arguments must be ints".to_string()
                                );
                            }
                        }
                        return Ok(Ty::Int);
                    }

                    if name == "run" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target run(cmd) needs a string".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "capture" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target capture(cmd) needs a string".to_string());
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "read_file" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target read_file(path) needs a string".to_string());
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "write_file" && args.len() == 2 {
                        if self.infer(&args[0])? != Ty::Str || self.infer(&args[1])? != Ty::Str {
                            return Err(
                                "wasm target write_file(path, data) needs strings".to_string()
                            );
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "argc" && args.is_empty() {
                        return Ok(Ty::Int);
                    }
                    if name == "arg" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Int {
                            return Err("wasm target arg() takes an int index".to_string());
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "len" && args.len() == 1 {
                        return match self.infer(&args[0])? {
                            Ty::Arr | Ty::Str => Ok(Ty::Int),
                            other => {
                                Err(format!("wasm target cannot take len of {}", ty_name(other)))
                            }
                        };
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

    fn check_fn_body(&mut self, params: &[String], body: &[Stmt]) -> Result<(), String> {
        let saved_scope = std::mem::take(&mut self.local_scope);
        let saved_depth = self.fn_depth;
        self.fn_depth += 1;
        for param in params {
            self.local_scope.insert(param.clone(), Ty::Int);
        }
        let outcome = (|| -> Result<(), String> {
            for stmt in body {
                self.check_stmt(stmt)?;
            }
            Ok(())
        })();
        self.local_scope = saved_scope;
        self.fn_depth = saved_depth;
        outcome
    }

    fn check_cond(&self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Bin(
                op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge),
                left,
                right,
            ) => {
                let _ = op;
                for side in [left, right] {
                    let ty = self.infer(side)?;
                    if !is_number(ty) {
                        return Err(format!(
                            "wasm target compares numbers only, got {}",
                            ty_name(ty)
                        ));
                    }
                }
                Ok(())
            }
            Expr::Not(inner) => self.check_cond(inner),
            other => Err(format!(
                "wasm target supports comparison conditions only, got {}",
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
                self.declare_let(name, ty)?;
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
                        ty_name(expected),
                        ty_name(actual)
                    ));
                }
            }
            Stmt::Expr(Expr::Call(callee, args, _), _) => {
                let Expr::Ident(name) = &**callee else {
                    return Err("wasm target only supports calls to print()".to_string());
                };
                if let Some(sig) = self.fn_sig(name) {
                    if args.len() != sig.params {
                        return Err(format!(
                            "wasm target function '{}' takes {} arguments",
                            name, sig.params
                        ));
                    }
                    for arg in args {
                        if self.infer(arg)? != Ty::Int {
                            return Err("wasm target function arguments must be ints".to_string());
                        }
                    }
                    return Ok(());
                } else if name != "print" {
                    return Err(format!(
                        "wasm target does not support calling '{}' yet",
                        name
                    ));
                }
                if args.len() != 1 {
                    return Err("wasm target supports print(<value>) with one argument".to_string());
                }
                let ty = self.infer(&args[0])?;
                if ty == Ty::Arr {
                    return Err("wasm target cannot print arrays yet".to_string());
                }
            }

            Stmt::Expr(Expr::Compound(target, _op, value, _), _) => {
                let Expr::Ident(name) = &**target else {
                    return Err(
                        "wasm target supports compound assignment to a global only".to_string()
                    );
                };
                let target_ty = self.lookup(name)?;
                let value_ty = self.infer(value)?;
                if !is_number(target_ty) || !is_number(value_ty) {
                    return Err(
                        "wasm target supports compound assignment on numbers only".to_string()
                    );
                }
                if target_ty == Ty::Int && value_ty == Ty::Float {
                    return Err(
                        "wasm target requires stable types for compound assignment".to_string()
                    );
                }
            }
            Stmt::Expr(expr, _) => {
                self.infer(expr)?;
            }
            Stmt::Block(stmts) => {
                for stmt in stmts {
                    self.check_stmt(stmt)?;
                }
            }
            Stmt::If(cond, then_body, else_body, _) => {
                self.check_cond(cond)?;
                for stmt in then_body {
                    self.check_stmt(stmt)?;
                }
                if let Some(else_body) = else_body {
                    for stmt in else_body {
                        self.check_stmt(stmt)?;
                    }
                }
            }
            Stmt::While(cond, body, _) => {
                self.check_cond(cond)?;
                for stmt in body {
                    self.check_stmt(stmt)?;
                }
            }
            Stmt::ForC(init, cond, step, body, _) => {
                if let Some(init) = init {
                    self.check_stmt(init)?;
                }
                if let Some(cond) = cond {
                    self.check_cond(cond)?;
                }
                if let Some(step) = step {
                    match step {
                        Expr::Assign(target, value, _) => {
                            let Expr::Ident(name) = &**target else {
                                return Err(
                                    "wasm target supports assignment to a global name only"
                                        .to_string(),
                                );
                            };
                            let expected = self.lookup(name)?;
                            let actual = self.infer(value)?;
                            if expected != actual {
                                return Err(format!(
                                    "wasm target requires stable types: '{}' is {}, assigned {}",
                                    name,
                                    ty_name(expected),
                                    ty_name(actual)
                                ));
                            }
                        }
                        Expr::Compound(target, _op, value, _) => {
                            let Expr::Ident(name) = &**target else {
                                return Err(
                                    "wasm target supports compound assignment to a global only"
                                        .to_string(),
                                );
                            };
                            let target_ty = self.lookup(name)?;
                            let value_ty = self.infer(value)?;
                            if !is_number(target_ty) || !is_number(value_ty) {
                                return Err(
                                    "wasm target supports compound assignment on numbers only"
                                        .to_string(),
                                );
                            }
                            if target_ty == Ty::Int && value_ty == Ty::Float {
                                return Err(
                                    "wasm target requires stable types for compound assignment"
                                        .to_string(),
                                );
                            }
                        }
                        other => {
                            self.infer(other)?;
                        }
                    }
                }
                for stmt in body {
                    self.check_stmt(stmt)?;
                }
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Return(expr, _) => {
                if self.fn_depth == 0 {
                    return Err("wasm target has no function to return from".to_string());
                }
                if let Some(value) = expr {
                    if self.infer(value)? != Ty::Int {
                        return Err("wasm target returns ints only".to_string());
                    }
                }
            }
            Stmt::Fn(name, params, body, _) => {
                if self.fn_depth > 0 {
                    return Err("wasm target does not support nested functions".to_string());
                }
                let returns = body
                    .iter()
                    .any(|stmt| matches!(stmt, Stmt::Return(Some(_), _)));
                if self.fn_sig(name).is_none() {
                    self.declare_fn(name, params.len(), returns)?;
                }
                self.check_fn_body(params, body)?;
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

/// Local slots used by the emitted body: 0 = array temporary, 1..=5 = string
/// concatenation temporaries (ptr_a, len_a, ptr_b, len_b, dst).
const ARR_TMP: u32 = 0;
const PTR_A: u32 = 1;
const LEN_A: u32 = 2;
const PTR_B: u32 = 3;
const LEN_B: u32 = 4;
const DST: u32 = 5;
const ALLOC_TMP: u32 = 6;
const ARG_TMP: u32 = 7;

/// Imported host functions: print_i64, print_utf8, argc, arg, read_file, write_file,
/// run, capture, print_f64, fmod.
const IMPORT_COUNT: u32 = 10;

#[derive(Clone, Copy)]
struct LocalSlot {
    ty: Ty,
    /// First wasm local index; strings own two slots (ptr, len).
    slot: u32,
}

struct Emitter<'a> {
    checker: &'a Checker,
    builder: &'a mut Builder,
    /// Index of the bump-allocator global (module-internal heap).
    heap: u32,
    /// Structured-control stack: 0x02 block (loop exit), 0x03 loop, 0x04 if.
    labels: Vec<u8>,
    /// Wasm local slots while emitting a user function body.
    locals: HashMap<String, LocalSlot>,
    fn_mode: bool,
    /// First temporary-local index for the function currently being emitted.
    temp_base: u32,
}

impl Emitter<'_> {
    fn temp(&self, offset: u32) -> u32 {
        self.temp_base + offset
    }
}

impl<'a> Emitter<'a> {
    fn push_global(&mut self, name: &str, set: bool) -> Result<(), String> {
        let index = self.checker.binding(name)?.base;
        self.builder
            .push(Instr::Global(if set { 0x24 } else { 0x23 }, index));
        Ok(())
    }

    /// Bump-allocate `size` constant bytes: leaves the old heap pointer on the
    /// stack. The heap lives in a mutable module global, so no host import.
    fn emit_alloc_const(&mut self, size: u32) {
        self.builder.push(Instr::Global(0x23, self.heap));
        self.builder.push(Instr::Global(0x23, self.heap));
        self.builder
            .push(Instr::Const(0x41, WVal::I32(size as i32)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Global(0x24, self.heap));
    }

    /// Bump-allocate `size` bytes where the size is already the top stack value.
    fn emit_alloc_dynamic(&mut self) {
        self.builder.push(Instr::Local(0x21, self.temp(ALLOC_TMP)));
        self.builder.push(Instr::Global(0x23, self.heap));
        self.builder.push(Instr::Global(0x23, self.heap));
        self.builder.push(Instr::Local(0x20, self.temp(ALLOC_TMP)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Global(0x24, self.heap));
    }

    fn local_of(&self, name: &str) -> Option<LocalSlot> {
        if self.fn_mode {
            self.locals.get(name).copied()
        } else {
            None
        }
    }

    /// Push a local (or a string's ptr/len pair) onto the operand stack.
    fn push_local(&mut self, slot: LocalSlot) {
        self.builder.push(Instr::Local(0x20, slot.slot));
        if slot.ty == Ty::Str {
            self.builder.push(Instr::Local(0x20, slot.slot + 1));
        }
    }

    /// Pop the value (or ptr/len pair) into a local.
    fn pop_local(&mut self, slot: LocalSlot) {
        if slot.ty == Ty::Str {
            self.builder.push(Instr::Local(0x21, slot.slot + 1));
        }
        self.builder.push(Instr::Local(0x21, slot.slot));
    }

    fn emit_load(&mut self, name: &str) -> Result<(), String> {
        match self.local_of(name) {
            Some(slot) => self.push_local(slot),
            None => self.push_global(name, false)?,
        }
        Ok(())
    }

    fn emit_store(&mut self, name: &str) -> Result<(), String> {
        match self.local_of(name) {
            Some(slot) => self.pop_local(slot),
            None => self.store_ident(name)?,
        }
        Ok(())
    }

    fn break_depth(&self) -> Result<u32, String> {
        for (depth, kind) in self.labels.iter().rev().enumerate() {
            if *kind == 0x02 {
                return Ok(depth as u32);
            }
        }
        Err("wasm target has no loop to break out of".to_string())
    }

    fn continue_depth(&self) -> Result<u32, String> {
        for (depth, kind) in self.labels.iter().rev().enumerate() {
            if *kind == 0x03 {
                return Ok(depth as u32);
            }
        }
        Err("wasm target has no loop to continue".to_string())
    }

    fn emit_compound(&mut self, name: &str, op: BinOp, value: &Expr) -> Result<(), String> {
        let target_ty = match self.local_of(name) {
            Some(slot) => slot.ty,
            None => self.checker.binding(name)?.ty,
        };
        if !is_number(target_ty) {
            return Err(format!(
                "wasm target supports compound assignment on numbers only, '{}' is {}",
                name,
                ty_name(target_ty)
            ));
        }
        self.emit_load(name)?;
        let value_ty = self.emit_expr(value)?;
        let value_ty = if target_ty == Ty::Float && value_ty == Ty::Int {
            self.builder.push(Instr::Op(0xb9));
            Ty::Float
        } else {
            value_ty
        };
        if target_ty != value_ty {
            return Err(format!(
                "wasm target requires stable types for compound assignment on '{}'",
                name
            ));
        }
        match (target_ty, op) {
            (Ty::Int, BinOp::Add) => self.builder.push(Instr::Op(0x7c)),
            (Ty::Int, BinOp::Sub) => self.builder.push(Instr::Op(0x7d)),
            (Ty::Int, BinOp::Mul) => self.builder.push(Instr::Op(0x7e)),
            (Ty::Int, BinOp::Mod) => self.builder.push(Instr::Op(0x81)),
            (Ty::Float, BinOp::Add) => self.builder.push(Instr::Op(0xa0)),
            (Ty::Float, BinOp::Sub) => self.builder.push(Instr::Op(0xa1)),
            (Ty::Float, BinOp::Mul) => self.builder.push(Instr::Op(0xa2)),
            (Ty::Float, BinOp::Div) => self.builder.push(Instr::Op(0xa3)),
            (Ty::Float, BinOp::Mod) => self.builder.push(Instr::Call(9)),
            _ => {
                return Err(format!(
                    "wasm target does not support compound {:?} here",
                    op
                ))
            }
        }
        self.emit_store(name)
    }

    fn type_of(&self, expr: &Expr) -> Result<Ty, String> {
        match expr {
            Expr::Ident(name) => match self.local_of(name) {
                Some(slot) => Ok(slot.ty),
                None => self.checker.infer(expr),
            },
            Expr::Int(_) => Ok(Ty::Int),
            Expr::Float(_) => Ok(Ty::Float),
            Expr::Neg(inner) => {
                self.type_of(inner)?;
                Ok(Ty::Int)
            }
            Expr::Bin(op, left, right) => {
                let lt = self.type_of(left)?;
                let rt = self.type_of(right)?;
                if op == &BinOp::Add && lt == Ty::Str && rt == Ty::Str {
                    return Ok(Ty::Str);
                }
                if !is_number(lt) || !is_number(rt) {
                    return Err(format!(
                        "wasm target does not support {:?} on {} yet",
                        op,
                        ty_name(if !is_number(lt) { lt } else { rt })
                    ));
                }
                match op {
                    BinOp::Div => Ok(Ty::Float),
                    _ => {
                        if lt == Ty::Int && rt == Ty::Int {
                            Ok(Ty::Int)
                        } else {
                            Ok(Ty::Float)
                        }
                    }
                }
            }
            Expr::Call(callee, _, _) => {
                if let Expr::Ident(name) = &**callee {
                    if self.checker.fn_sig(name).is_some() {
                        return Ok(Ty::Int);
                    }
                    match name.as_str() {
                        "argc" | "write_file" | "run" => return Ok(Ty::Int),
                        "arg" | "read_file" | "capture" => return Ok(Ty::Str),
                        _ => {}
                    }
                }
                self.checker.infer(expr)
            }
            _ => self.checker.infer(expr),
        }
    }
    fn emit_f64(&mut self, expr: &Expr) -> Result<(), String> {
        let ty = self.emit_expr(expr)?;
        match ty {
            Ty::Int => {
                self.builder.push(Instr::Op(0xb9));
                Ok(())
            }
            Ty::Float => Ok(()),
            other => Err(format!(
                "wasm target needs a number here, got {}",
                ty_name(other)
            )),
        }
    }

    fn emit_numeric(&mut self, op: BinOp, left: &Expr, right: &Expr) -> Result<Ty, String> {
        let lt = self.type_of(left)?;
        let rt = self.type_of(right)?;
        if !is_number(lt) || !is_number(rt) {
            return Err(format!(
                "wasm target does not support {:?} on {} yet",
                op,
                ty_name(if !is_number(lt) { lt } else { rt })
            ));
        }
        let both_int = lt == Ty::Int && rt == Ty::Int;
        match op {
            BinOp::Div => {
                self.emit_f64(left)?;
                self.emit_f64(right)?;
                self.builder.push(Instr::Op(0xa3));
                Ok(Ty::Float)
            }
            BinOp::Mod if !both_int => {
                self.emit_f64(left)?;
                self.emit_f64(right)?;
                self.builder.push(Instr::Call(9));
                Ok(Ty::Float)
            }
            BinOp::Mod => {
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                self.builder.push(Instr::Op(0x81));
                Ok(Ty::Int)
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                if both_int {
                    self.emit_expr(left)?;
                    self.emit_expr(right)?;
                    let opcode = match op {
                        BinOp::Add => 0x7c,
                        BinOp::Sub => 0x7d,
                        _ => 0x7e,
                    };
                    self.builder.push(Instr::Op(opcode));
                    Ok(Ty::Int)
                } else {
                    self.emit_f64(left)?;
                    self.emit_f64(right)?;
                    let opcode = match op {
                        BinOp::Add => 0xa0,
                        BinOp::Sub => 0xa1,
                        _ => 0xa2,
                    };
                    self.builder.push(Instr::Op(opcode));
                    Ok(Ty::Float)
                }
            }
            other => Err(format!("wasm target does not support {:?} here", other)),
        }
    }
    fn emit_cond(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Not(inner) => {
                self.emit_cond(inner)?;
                self.builder.push(Instr::Op(0x45));
                Ok(())
            }
            Expr::Bin(op, left, right) => {
                let cmp = matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                );
                if !cmp {
                    return Err("wasm target supports comparison conditions only".to_string());
                }
                let lt = self.type_of(left)?;
                let rt = self.type_of(right)?;
                if !is_number(lt) || !is_number(rt) {
                    return Err("wasm target compares numbers only".to_string());
                }
                if lt == Ty::Float || rt == Ty::Float {
                    self.emit_f64(left)?;
                    self.emit_f64(right)?;
                    let opcode = match op {
                        BinOp::Eq => 0x61,
                        BinOp::Ne => 0x62,
                        BinOp::Lt => 0x63,
                        BinOp::Gt => 0x64,
                        BinOp::Le => 0x65,
                        _ => 0x66,
                    };
                    self.builder.push(Instr::Op(opcode));
                    return Ok(());
                }
                let opcode = match op {
                    BinOp::Eq => 0x51,
                    BinOp::Ne => 0x52,
                    BinOp::Lt => 0x53,
                    BinOp::Le => 0x57,
                    BinOp::Gt => 0x55,
                    _ => 0x59,
                };
                self.emit_expr(left)?;
                self.emit_expr(right)?;
                self.builder.push(Instr::Op(opcode));
                Ok(())
            }
            other => Err(format!(
                "wasm target supports comparison conditions only, got {}",
                expr_kind(other)
            )),
        }
    }
    fn emit_while(
        &mut self,
        cond: Option<&Expr>,
        body: &[Stmt],
        step: Option<&Expr>,
    ) -> Result<(), String> {
        self.builder.push(Instr::Block {
            opcode: 0x02,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x02);
        self.builder.push(Instr::Block {
            opcode: 0x03,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x03);
        if let Some(cond) = cond {
            self.emit_cond(cond)?;
            self.builder.push(Instr::Op(0x45));
            self.builder.push(Instr::BrIf(1));
        }
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        if let Some(step) = step {
            match step {
                Expr::Assign(target, value, _) => {
                    let Expr::Ident(name) = &**target else {
                        return Err(
                            "wasm target supports assignment to a global name only".to_string()
                        );
                    };
                    self.emit_expr(value)?;
                    self.emit_store(name)?;
                }
                Expr::Compound(target, op, value, _) => {
                    let Expr::Ident(name) = &**target else {
                        return Err(
                            "wasm target supports compound assignment to a global only".to_string()
                        );
                    };
                    self.emit_compound(name, *op, value)?;
                }
                other => {
                    self.emit_expr(other)?;
                    self.builder.push(Instr::Op(0x1a));
                }
            }
        }
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        Ok(())
    }

    fn store_ident(&mut self, name: &str) -> Result<(), String> {
        let ty = self.checker.lookup(name)?;
        let base = self.checker.binding(name)?.base;
        if ty == Ty::Str {
            self.builder.push(Instr::Global(0x24, base + 1));
            self.builder.push(Instr::Global(0x24, base));
        } else {
            self.builder.push(Instr::Global(0x24, base));
        }
        Ok(())
    }

    /// Push an integer string address/length pair for a string expression.
    fn emit_str(&mut self, expr: &Expr) -> Result<(), String> {
        if let Some(text) = string_value(expr) {
            let offset = self.builder.add_data(text.as_bytes());
            self.builder
                .push(Instr::Const(0x41, WVal::I32(offset as i32)));
            self.builder
                .push(Instr::Const(0x41, WVal::I32(text.len() as i32)));
            return Ok(());
        }
        match expr {
            Expr::Ident(name) => {
                if let Some(slot) = self.local_of(name) {
                    if slot.ty != Ty::Str {
                        return Err(format!(
                            "wasm target cannot use {} as a string",
                            ty_name(slot.ty)
                        ));
                    }
                    self.push_local(slot);
                    return Ok(());
                }
                let base = self.checker.binding(name)?.base;
                self.builder.push(Instr::Global(0x23, base));
                self.builder.push(Instr::Global(0x23, base + 1));
                Ok(())
            }
            Expr::Bin(BinOp::Add, left, right) => {
                self.emit_str(left)?;
                self.builder.push(Instr::Local(0x21, self.temp(LEN_A)));
                self.builder.push(Instr::Local(0x21, self.temp(PTR_A)));
                self.emit_str(right)?;
                self.builder.push(Instr::Local(0x21, self.temp(LEN_B)));
                self.builder.push(Instr::Local(0x21, self.temp(PTR_B)));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_B)));
                self.builder.push(Instr::Op(0x6a));
                self.emit_alloc_dynamic();
                self.builder.push(Instr::Local(0x21, self.temp(DST)));
                self.builder.push(Instr::Local(0x20, self.temp(DST)));
                self.builder.push(Instr::Local(0x20, self.temp(PTR_A)));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                self.builder.push(Instr::MemoryCopy);
                self.builder.push(Instr::Local(0x20, self.temp(DST)));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x20, self.temp(PTR_B)));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_B)));
                self.builder.push(Instr::MemoryCopy);
                self.builder.push(Instr::Local(0x20, self.temp(DST)));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_B)));
                self.builder.push(Instr::Op(0x6a));
                Ok(())
            }
            other => Err(format!(
                "wasm target does not support this string expression yet: {}",
                expr_kind(other)
            )),
        }
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<Ty, String> {
        if string_value(expr).is_some() {
            self.emit_str(expr)?;
            return Ok(Ty::Str);
        }
        match expr {
            Expr::Int(value) => {
                self.builder.push(Instr::Const(0x42, WVal::I64(*value)));
                Ok(Ty::Int)
            }
            Expr::Float(value) => {
                self.builder.push(Instr::Const(0x44, WVal::F64(*value)));
                Ok(Ty::Float)
            }
            Expr::Ident(name) => {
                if let Some(slot) = self.local_of(name) {
                    self.push_local(slot);
                    return Ok(slot.ty);
                }
                let ty = self.checker.lookup(name)?;
                match ty {
                    Ty::Str => {
                        let base = self.checker.binding(name)?.base;
                        self.builder.push(Instr::Global(0x23, base));
                        self.builder.push(Instr::Global(0x23, base + 1));
                    }
                    Ty::Int | Ty::Float | Ty::Arr | Ty::Obj(_) => self.push_global(name, false)?,
                }
                Ok(ty)
            }
            Expr::Neg(inner) => {
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.emit_expr(inner)?;
                self.builder.push(Instr::Op(0x7d));
                Ok(Ty::Int)
            }
            Expr::Bin(BinOp::Add, left, right) => {
                if self.type_of(left)? == Ty::Str {
                    self.emit_str(expr)?;
                    return Ok(Ty::Str);
                }
                self.emit_numeric(BinOp::Add, left, right)
            }
            Expr::Bin(op, left, right) => self.emit_numeric(*op, left, right),
            Expr::Object(fields) => {
                let mut keys: Vec<String> = Vec::new();
                for (key, value) in fields {
                    let field_ty = self.type_of(value)?;
                    if field_ty != Ty::Int {
                        return Err(format!(
                            "wasm target supports int object fields only, got {}",
                            ty_name(field_ty)
                        ));
                    }
                    keys.push(key.clone());
                }
                let shape = self.checker.shape_id(&keys);
                let shape = Ty::Obj(shape);
                let size = 8 * fields.len() as u32;
                self.emit_alloc_const(size);
                self.builder.push(Instr::Local(0x21, self.temp(ARR_TMP)));
                for (index, (_, value)) in fields.iter().enumerate() {
                    self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                    self.emit_expr(value)?;
                    self.builder.push(Instr::Mem(0x37, 3, (8 * index) as u32));
                }
                self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                Ok(shape)
            }
            Expr::Member(object, key, _) => {
                let ty = self.type_of(object)?;
                let Ty::Obj(shape) = ty else {
                    return Err(format!(
                        "wasm target can read fields of objects only, got {}",
                        ty_name(ty)
                    ));
                };
                let index = self.checker.field_index(shape, key)?;
                self.emit_expr(object)?;
                self.builder.push(Instr::Mem(0x29, 3, (8 * index) as u32));
                Ok(Ty::Int)
            }
            Expr::Array(items) => {
                let count = items.len() as u32;
                let size = 4 + 8 * count;
                self.emit_alloc_const(size);
                self.builder.push(Instr::Local(0x21, self.temp(ARR_TMP)));
                self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                self.builder
                    .push(Instr::Const(0x41, WVal::I32(count as i32)));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                for (index, item) in items.iter().enumerate() {
                    self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32((4 + 8 * index as u32) as i32)));
                    self.builder.push(Instr::Op(0x6a));
                    self.emit_expr(item)?;
                    self.builder.push(Instr::Mem(0x37, 3, 0));
                }
                self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
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
                    if let Some(sig) = self.checker.fn_sig(name) {
                        if !sig.returns {
                            return Err(format!(
                                "wasm target function '{}' returns no value",
                                name
                            ));
                        }
                        if args.len() != sig.params {
                            return Err(format!(
                                "wasm target function '{}' takes {} arguments",
                                name, sig.params
                            ));
                        }
                        for arg in args {
                            self.emit_expr(arg)?;
                        }
                        self.builder.push(Instr::Call(sig.index));
                        return Ok(Ty::Int);
                    }

                    if name == "run" && args.len() == 1 {
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(6));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                    if name == "capture" && args.len() == 1 {
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(7));
                        self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Mem(0x28, 2, 4));
                        return Ok(Ty::Str);
                    }
                    if name == "read_file" && args.len() == 1 {
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(4));
                        self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        return Ok(Ty::Str);
                    }
                    if name == "write_file" && args.len() == 2 {
                        self.emit_str(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Call(5));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                    if name == "argc" && args.is_empty() {
                        self.builder.push(Instr::Call(2));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                    if name == "arg" && args.len() == 1 {
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Call(3));
                        self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        return Ok(Ty::Str);
                    }
                    if name == "len" && args.len() == 1 {
                        match self.type_of(&args[0])? {
                            Ty::Arr => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Mem(0x28, 2, 0));
                                self.builder.push(Instr::Op(0xad));
                                return Ok(Ty::Int);
                            }
                            Ty::Str => {
                                let Expr::Ident(var) = &args[0] else {
                                    return Err(
                                        "wasm target supports len() of string variables only"
                                            .to_string(),
                                    );
                                };
                                if let Some(slot) = self.local_of(var) {
                                    if slot.ty != Ty::Str {
                                        return Err("wasm target cannot take len here".to_string());
                                    }
                                    self.builder.push(Instr::Local(0x20, slot.slot + 1));
                                    self.builder.push(Instr::Op(0xad));
                                    return Ok(Ty::Int);
                                }
                                let base = self.checker.binding(var)?.base;
                                self.builder.push(Instr::Global(0x23, base + 1));
                                self.builder.push(Instr::Op(0xad));
                                return Ok(Ty::Int);
                            }
                            other => {
                                return Err(format!(
                                    "wasm target cannot take len of {}",
                                    ty_name(other)
                                ))
                            }
                        }
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
                let ty = match init {
                    Some(expr) => self.emit_expr(expr)?,
                    None => {
                        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                        Ty::Int
                    }
                };
                if self.fn_mode {
                    let slot = match ty {
                        Ty::Int => self.builder.add_local(ValType::I64),
                        Ty::Float => self.builder.add_local(ValType::F64),
                        Ty::Str => {
                            let slot = self.builder.add_local(ValType::I32);
                            self.builder.add_local(ValType::I32);
                            slot
                        }
                        Ty::Arr | Ty::Obj(_) => self.builder.add_local(ValType::I32),
                    };
                    self.locals.insert(name.clone(), LocalSlot { ty, slot });
                    self.emit_store(name)?;
                    return Ok(());
                }
                let base = self.checker.binding(name)?.base;
                if ty == Ty::Str {
                    self.builder.push(Instr::Global(0x24, base + 1));
                    self.builder.push(Instr::Global(0x24, base));
                } else {
                    self.builder.push(Instr::Global(0x24, base));
                }
                Ok(())
            }
            Stmt::Expr(Expr::Assign(target, value, _), _) => {
                let Expr::Ident(name) = &**target else {
                    return Err("wasm target supports assignment to a global name only".to_string());
                };
                let ty = self.emit_expr(value)?;
                if self.local_of(name).is_some() {
                    self.emit_store(name)?;
                    return Ok(());
                }
                let expected = self.checker.binding(name)?.ty;
                if expected == Ty::Int && ty == Ty::Float {
                    return Err(format!(
                        "wasm target requires stable types: '{}' is int, assigned float",
                        name
                    ));
                }
                let ty = if expected == Ty::Float && ty == Ty::Int {
                    self.builder.push(Instr::Op(0xb9));
                    Ty::Float
                } else {
                    ty
                };
                let base = self.checker.binding(name)?.base;
                if ty == Ty::Str {
                    self.builder.push(Instr::Global(0x24, base + 1));
                    self.builder.push(Instr::Global(0x24, base));
                } else {
                    self.builder.push(Instr::Global(0x24, base));
                }
                Ok(())
            }
            Stmt::Expr(Expr::Call(callee, args, _), _) => {
                let Expr::Ident(name) = &**callee else {
                    return Err("wasm target only supports calls to print()".to_string());
                };
                if let Some(sig) = self.checker.fn_sig(name) {
                    if args.len() != sig.params {
                        return Err(format!(
                            "wasm target function '{}' takes {} arguments",
                            name, sig.params
                        ));
                    }
                    for arg in args {
                        self.emit_expr(arg)?;
                    }
                    self.builder.push(Instr::Call(sig.index));
                    if sig.returns {
                        self.builder.push(Instr::Op(0x1a));
                    }
                    return Ok(());
                }
                if name != "print" || args.len() != 1 {
                    return Err("wasm target supports print(<value>) with one argument".to_string());
                }
                let ty = self.emit_expr(&args[0])?;
                match ty {
                    Ty::Str => {
                        self.builder.push(Instr::Call(1));
                    }
                    Ty::Float => {
                        self.builder.push(Instr::Call(8));
                    }
                    _ => {
                        self.builder.push(Instr::Call(0));
                        self.builder.push(Instr::Op(0x1a));
                    }
                }
                Ok(())
            }
            Stmt::Expr(Expr::Compound(target, op, value, _), _) => {
                let Expr::Ident(name) = &**target else {
                    return Err(
                        "wasm target supports compound assignment to a global only".to_string()
                    );
                };
                self.emit_compound(name, *op, value)
            }
            Stmt::Expr(expr, _) => {
                let ty = self.emit_expr(expr)?;
                match ty {
                    Ty::Str => {
                        // Drop both halves of the pointer/length pair.
                        self.builder.push(Instr::Op(0x1a));
                        self.builder.push(Instr::Op(0x1a));
                    }
                    Ty::Int | Ty::Float | Ty::Arr | Ty::Obj(_) => {
                        self.builder.push(Instr::Op(0x1a))
                    }
                }
                Ok(())
            }
            Stmt::Block(stmts) => {
                for stmt in stmts {
                    self.emit_stmt(stmt)?;
                }
                Ok(())
            }
            Stmt::If(cond, then_body, else_body, _) => {
                self.emit_cond(cond)?;
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                for stmt in then_body {
                    self.emit_stmt(stmt)?;
                }
                if let Some(else_body) = else_body {
                    self.builder.push(Instr::Else { end: 0 });
                    for stmt in else_body {
                        self.emit_stmt(stmt)?;
                    }
                }
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                Ok(())
            }
            Stmt::While(cond, body, _) => self.emit_while(Some(cond), body, None),
            Stmt::ForC(init, cond, step, body, _) => {
                if let Some(init) = init {
                    self.emit_stmt(init)?;
                }
                self.emit_while(cond.as_ref(), body, step.as_ref())
            }
            Stmt::Break(_) => {
                let depth = self.break_depth()?;
                self.builder.push(Instr::Br(depth));
                Ok(())
            }
            Stmt::Return(expr, _) => {
                if let Some(expr) = expr {
                    self.emit_expr(expr)?;
                }
                self.builder.push(Instr::Op(0x0f));
                Ok(())
            }
            Stmt::Fn(..) => Ok(()),
            Stmt::Continue(_) => {
                let depth = self.continue_depth()?;
                self.builder.push(Instr::Br(depth));
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
    struct FnDecl {
        params: Vec<String>,
        body: Vec<Stmt>,
        returns: bool,
    }

    let stmts = crate::parser::parse(source)?;
    let mut checker = Checker::new();
    let mut declarations: Vec<(String, FnDecl)> = Vec::new();
    for stmt in &stmts {
        if let Stmt::Fn(name, params, body, _) = stmt {
            let returns = body
                .iter()
                .any(|stmt| matches!(stmt, Stmt::Return(Some(_), _)));
            checker.declare_fn(name, params.len(), returns)?;
            declarations.push((
                name.clone(),
                FnDecl {
                    params: params.clone(),
                    body: body.clone(),
                    returns,
                },
            ));
        }
    }
    for stmt in &stmts {
        checker.check_stmt(stmt)?;
    }

    let mut builder = Builder::new();
    let print_i64_ty = builder.add_type(vec![ValType::I64], vec![ValType::I64]);
    let print_utf8_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![]);
    let argc_ty = builder.add_type(vec![], vec![ValType::I32]);
    let arg_ty = builder.add_type(vec![ValType::I32], vec![ValType::I32]);
    let read_file_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let write_file_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    let run_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let capture_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let print_f64_ty = builder.add_type(vec![ValType::F64], vec![]);
    let fmod_ty = builder.add_type(vec![ValType::F64, ValType::F64], vec![ValType::F64]);
    let start_ty = builder.add_type(vec![], vec![]);
    let mut fn_types: Vec<u32> = Vec::new();
    for (_, decl) in &declarations {
        let params = vec![ValType::I64; decl.params.len()];
        let results = if decl.returns {
            vec![ValType::I64]
        } else {
            Vec::new()
        };
        fn_types.push(builder.add_type(params, results));
    }
    builder.import_func("env", "print_i64", print_i64_ty);
    builder.import_func("env", "print_utf8", print_utf8_ty);
    builder.import_func("bx", "argc", argc_ty);
    builder.import_func("bx", "arg", arg_ty);
    builder.import_func("bx", "read_file", read_file_ty);
    builder.import_func("bx", "write_file", write_file_ty);
    builder.import_func("bx", "run", run_ty);
    builder.import_func("bx", "capture", capture_ty);
    builder.import_func("env", "print_f64", print_f64_ty);
    builder.import_func("env", "fmod", fmod_ty);
    builder.enable_memory(8);
    for name in &checker.order {
        let ty = checker.lookup(name)?;
        match ty {
            Ty::Int => builder.add_global(ValType::I64, true, WVal::I64(0)),
            Ty::Float => builder.add_global(ValType::F64, true, WVal::F64(0.0)),
            Ty::Arr => builder.add_global(ValType::I32, true, WVal::I32(0)),
            Ty::Str => {
                builder.add_global(ValType::I32, true, WVal::I32(0));
                builder.add_global(ValType::I32, true, WVal::I32(0));
            }
            Ty::Obj(_) => builder.add_global(ValType::I32, true, WVal::I32(0)),
        }
    }
    let heap = checker.globals_used;
    builder.add_global(ValType::I32, true, WVal::I32(8192));

    builder.begin_function(start_ty);
    for _ in 0..8 {
        builder.add_local(ValType::I32);
    }
    {
        let mut emitter = Emitter {
            checker: &checker,
            builder: &mut builder,
            heap,
            labels: Vec::new(),
            locals: HashMap::new(),
            fn_mode: false,
            temp_base: 0,
        };
        for stmt in &stmts {
            emitter.emit_stmt(stmt)?;
        }
    }

    for (index, (_, decl)) in declarations.iter().enumerate() {
        builder.begin_function(fn_types[index]);
        let temp_base = declarations[index].1.params.len() as u32;
        for _ in 0..8 {
            builder.add_local(ValType::I32);
        }
        {
            let mut emitter = Emitter {
                checker: &checker,
                builder: &mut builder,
                heap,
                labels: Vec::new(),
                locals: HashMap::new(),
                fn_mode: true,
                temp_base,
            };
            for (slot, param) in decl.params.iter().enumerate() {
                emitter.locals.insert(
                    param.clone(),
                    LocalSlot {
                        ty: Ty::Int,
                        slot: slot as u32,
                    },
                );
            }
            for stmt in &decl.body {
                emitter.emit_stmt(stmt)?;
            }
            if decl.returns {
                emitter.builder.push(Instr::Const(0x42, WVal::I64(0)));
            }
        }
    }
    builder.finish("_start")
}
/// Summary of a decoded wasm module, for `bx check --json`.
pub struct WasmInfo {
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub defined_functions: usize,
}

/// Decode a module and summarise its imports/exports for tooling.
pub fn inspect(bytes: &[u8]) -> Result<WasmInfo, String> {
    let module = crate::wasm::validate(bytes)?;
    Ok(WasmInfo {
        imports: module
            .imports
            .iter()
            .map(|import| format!("{}.{}", import.module, import.name))
            .collect(),
        exports: module
            .exports
            .iter()
            .map(|(name, _)| name.clone())
            .collect(),
        defined_functions: module.funcs.len(),
    })
}

/// Parse and validate a module without running it (used by the module tests).
#[cfg(test)]
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    crate::wasm::validate(bytes).map(|_| ())
}

/// Instantiate and run `_start`, forwarding `env.print_i64` / `env.print_utf8`
/// and exposing `argv` through `bx.argc` / `bx.arg`.
pub fn execute<F, G>(bytes: &[u8], argv: &[String], on_i64: F, on_str: G) -> Result<(), String>
where
    F: FnMut(i64),
    G: FnMut(&str),
{
    crate::wasm::run_start(bytes, argv, on_i64, on_str)
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
            &[],
            move |value| {
                sink.lock().expect("lock").push(value);
            },
            |_text| {},
        )
        .expect("execute");
        let out = seen.lock().expect("lock").clone();
        out
    }

    fn run_strings(source: &str) -> Vec<String> {
        let bytes = compile_to_wasm(source).expect("compile");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        execute(
            &bytes,
            &[],
            |_value| {},
            move |text| {
                sink.lock().expect("lock").push(text.to_string());
            },
        )
        .expect("execute");
        let out = seen.lock().expect("lock").clone();
        out
    }

    fn run_with_argv(source: &str, argv: &[String]) -> (Vec<i64>, Vec<String>) {
        let bytes = compile_to_wasm(source).expect("compile");
        let numbers = Arc::new(Mutex::new(Vec::new()));
        let strings = Arc::new(Mutex::new(Vec::new()));
        let number_sink = numbers.clone();
        let string_sink = strings.clone();
        execute(
            &bytes,
            argv,
            move |value| number_sink.lock().expect("lock").push(value),
            move |text| string_sink.lock().expect("lock").push(text.to_string()),
        )
        .expect("execute");
        let numbers = numbers.lock().expect("lock").clone();
        let strings = strings.lock().expect("lock").clone();
        (numbers, strings)
    }

    #[test]
    fn commands_run_through_the_shell() {
        let (numbers, strings) =
            run_with_argv("print(run(\"echo hi\"))\nprint(capture(\"echo hi\"))", &[]);
        assert_eq!(numbers, vec![0]);
        // Windows' cmd adds CRLF; the ABI passes bytes through untouched.
        assert_eq!(strings.len(), 1);
        assert_eq!(strings[0].trim(), "hi");
    }

    #[test]
    fn files_round_trip_through_the_host() {
        let path = std::env::temp_dir().join(format!("bx-wasm-fs-{}.txt", std::process::id()));
        let escaped = path.to_string_lossy().replace('\\', "\\\\");
        let source = format!(
            "let p = \"{escaped}\"\nprint(write_file(p, \"payload\"))\nprint(read_file(p))"
        );
        let (numbers, strings) = run_with_argv(&source, &[]);
        let _ = std::fs::remove_file(&path);
        assert_eq!(numbers, vec![0]);
        assert_eq!(strings, vec!["payload".to_string()]);
    }

    #[test]
    fn argv_reaches_the_module() {
        let argv = vec!["alpha".to_string(), "beta".to_string()];
        let (numbers, strings) =
            run_with_argv("print(argc())\nprint(arg(0))\nprint(arg(1))", &argv);
        assert_eq!(numbers, vec![2]);
        assert_eq!(strings, vec!["alpha".to_string(), "beta".to_string()]);
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
    fn runtime_strings_concatenate() {
        let prints = run_strings("let s = \"ab\"\nlet t = s + \"cd\"\nprint(t + s)\nprint(len(t))");
        assert_eq!(prints, vec!["abcdab".to_string()]);
    }

    #[test]
    fn user_functions_take_params_and_return() {
        let prints = run_source("fn add(a, b) { return a + b }\nprint(add(2, 3))");
        assert_eq!(prints, vec![5]);
    }

    #[test]
    fn recursion_works() {
        let prints = run_source(
            "fn fact(n) {\n  if (n < 2) { return 1 }\n  return n * fact(n - 1)\n}\nprint(fact(5))",
        );
        assert_eq!(prints, vec![120]);
    }

    #[test]
    fn functions_compose_with_loops() {
        let prints = run_source(
            "fn sum(n) {\n  let s = 0\n  for (let i = 0; i < n; i += 1) { s += i }\n  return s\n}\nprint(sum(5))",
        );
        assert_eq!(prints, vec![10]);
    }

    #[test]
    fn unit_functions_touch_globals() {
        let prints =
            run_source("let count = 0\nfn bump() { count += 1 }\nbump()\nbump()\nprint(count)");
        assert_eq!(prints, vec![2]);
    }

    #[test]
    #[test]
    fn functions_support_rich_locals() {
        let (numbers, strings) = run_with_argv(
            "fn build() {\n  let a = [1, 2, 3]\n  let s = \"hi\"\n  print(a[1])\n  print(s + \"!\")\n  let o = {k: 7}\n  print(o.k)\n  let f = 1.5\n  print(f * 2)\n}\nbuild()",
            &[],
        );
        assert_eq!(numbers, vec![2, 7]);
        assert_eq!(strings, vec!["hi!".to_string(), "3".to_string()]);
    }

    #[test]
    fn functions_read_rich_globals() {
        let (numbers, strings) = run_with_argv(
            "let s = \"abc\"\nfn show() { print(s) }\nshow()\nlet a = [5, 6]\nfn pick() { return a[1] }\nprint(pick())\nlet o = {a: 9}\nfn field() { return o.a }\nprint(field())",
            &[],
        );
        assert_eq!(numbers, vec![6, 9]);
        assert_eq!(strings, vec!["abc".to_string()]);
    }

    #[test]
    fn comparison_operators_match_wasm() {
        let prints = run_source(
            "let x = 5\nif (x <= 5) { print(1) }\nif (x >= 5) { print(2) }\nif (x < 5) { print(3) }\nif (x > 5) { print(4) }\nif (x != 5) { print(5) }",
        );
        assert_eq!(prints, vec![1, 2]);
    }

    #[test]
    fn floats_match_the_vm_formatting() {
        let (numbers, strings) = run_with_argv(
            "print(7 / 2)\nprint(6 / 2)\nprint(2.5 + 1)\nprint(7 % 2.5)\nlet f = 1.5\nf *= 2\nprint(f)\nf /= 4\nprint(f)\nif (2.5 < 3) { print(100) }",
            &[],
        );
        assert_eq!(numbers, vec![100]);
        assert_eq!(
            strings,
            vec![
                "3.5".to_string(),
                "3".to_string(),
                "3.5".to_string(),
                "2".to_string(),
                "3".to_string(),
                "0.75".to_string(),
            ]
        );
    }

    #[test]
    fn loops_and_conditionals() {
        let prints = run_source(
            "let i = 0\nwhile (i < 5) {\n  print(i)\n  i = i + 1\n}\nif (i == 5) { print(100) } else { print(200) }",
        );
        assert_eq!(prints, vec![0, 1, 2, 3, 4, 100]);
    }

    #[test]
    fn break_and_continue() {
        let prints = run_source(
            "let i = 0\nlet sum = 0\nwhile (i < 10) {\n  i = i + 1\n  if (i == 3) { continue }\n  if (i == 6) { break }\n  sum = sum + i\n}\nprint(sum)",
        );
        assert_eq!(prints, vec![1 + 2 + 4 + 5]);
    }

    #[test]
    fn for_loops_desugar() {
        let prints = run_source(
            "let sum = 0\nfor (let i = 0; i < 4; i = i + 1) {\n  sum = sum + i\n}\nprint(sum)",
        );
        assert_eq!(prints, vec![6]);
    }

    #[test]
    fn records_are_fixed_layout_objects() {
        let prints = run_source("let o = {a: 10, b: 20}\nprint(o.a + o.b)");
        assert_eq!(prints, vec![30]);
    }

    #[test]
    fn modules_are_deterministic_and_inspectable() {
        let source = "let x = 7\nfn double(n) { return n * 2 }\nprint(double(x))";
        let first = compile_to_wasm(source).expect("compile");
        let second = compile_to_wasm(source).expect("compile again");
        assert_eq!(first, second, "compilation must be deterministic");
        let info = inspect(&first).expect("inspect");
        assert!(info.imports.contains(&"env.print_i64".to_string()));
        assert!(info.imports.contains(&"bx.argc".to_string()));
        assert_eq!(info.exports, vec!["_start".to_string()]);
        assert_eq!(info.defined_functions, 2);
    }

    #[test]
    fn module_is_standard_wasm() {
        let bytes = compile_to_wasm("print(7)").expect("compile");
        assert_eq!(&bytes[0..4], b"\0asm");
        validate(&bytes).expect("validate");
    }

    #[test]
    fn strings_print_through_memory() {
        let prints = run_strings("print(\"ab\" + \"cd\")");
        assert_eq!(prints, vec!["abcd".to_string()]);
    }

    #[test]
    fn unsupported_constructs_are_rejected() {
        assert!(compile_to_wasm("let f = fn() { return 1 }").is_err());
        assert!(compile_to_wasm("print(unknown)").is_err());
        assert!(compile_to_wasm("let a = [1, \"x\"]").is_err());
        assert!(compile_to_wasm("let o = {}\nprint(o)").is_err());
    }
}
