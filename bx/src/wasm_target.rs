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
    /// Dynamic string-keyed map (values are ints; wasm-target extension).
    Map,
    /// Non-capturing function literal (payload = literal id). Values carry a
    /// dummy i32 and calls are resolved statically.
    Func(u32),
    /// Boolean: real i32 0/1 (wasm-target extension beyond the earlier subset).
    Bool,
}

fn ty_name(ty: Ty) -> &'static str {
    match ty {
        Ty::Int => "int",
        Ty::Float => "float",
        Ty::Str => "string",
        Ty::Arr => "array",
        Ty::Obj(_) => "object",
        Ty::Map => "map",
        Ty::Func(_) => "function",
        Ty::Bool => "bool",
    }
}

fn is_number(ty: Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float)
}

/// A call-site argument may promote an int to a float parameter.
fn arg_ok(expected: Ty, actual: Ty) -> bool {
    expected == actual || (expected == Ty::Float && actual == Ty::Int)
}

/// Wasm value types used to represent one bx value.
fn wasm_types(ty: Ty) -> Vec<ValType> {
    match ty {
        Ty::Int => vec![ValType::I64],
        Ty::Float => vec![ValType::F64],
        Ty::Str => vec![ValType::I32, ValType::I32],
        Ty::Arr | Ty::Obj(_) | Ty::Map | Ty::Func(_) | Ty::Bool => vec![ValType::I32],
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
    /// First global index; a string binding owns two slots (ptr, len).
    base: u32,
}

#[derive(Clone, Debug)]
struct Shape {
    keys: Vec<String>,
}

#[derive(Clone, Debug)]
struct FnSig {
    params: Vec<Ty>,
    result: Option<Ty>,
    /// Index in the wasm function index space (imports first).
    index: u32,
}

struct Checker {
    env: HashMap<String, Binding>,
    order: Vec<String>,
    globals_used: u32,
    shapes: RefCell<Vec<Shape>>,
    fns: HashMap<String, FnSig>,
    /// Signatures of non-capturing function literals (wasm indices set later).
    lit_sigs: Vec<FnSig>,
    /// AST address -> literal id, filled by `compile_to_wasm`.
    literal_ids: HashMap<usize, u32>,
    /// Overlay for the function body currently being checked.
    local_scope: HashMap<String, Ty>,
    /// Loop-scoped bindings (for-in variables) visible in the current block.
    scopes: Vec<HashMap<String, Ty>>,
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
            lit_sigs: Vec::new(),
            literal_ids: HashMap::new(),
            local_scope: HashMap::new(),
            scopes: Vec::new(),
            fn_depth: 0,
        }
    }

    fn declare_fn(&mut self, name: &str, index: u32) -> Result<(), String> {
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
        self.fns.insert(
            name.to_string(),
            FnSig {
                params: Vec::new(),
                result: None,
                index,
            },
        );
        Ok(())
    }

    fn fn_sig(&self, name: &str) -> Option<&FnSig> {
        self.fns.get(name)
    }

    fn set_fn_sig(&mut self, name: &str, params: Vec<Ty>, result: Option<Ty>) {
        if let Some(sig) = self.fns.get_mut(name) {
            sig.params = params;
            sig.result = result;
        }
    }

    fn literal_call(&self, id: u32, args: &[Expr]) -> Result<Ty, String> {
        let sig = self
            .lit_sigs
            .get(id as usize)
            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
        let result = sig
            .result
            .ok_or_else(|| "wasm target function literal returns no value".to_string())?;
        if args.len() != sig.params.len() {
            return Err(format!(
                "wasm target function literal takes {} arguments",
                sig.params.len()
            ));
        }
        for (arg, expected) in args.iter().zip(sig.params.iter()) {
            let actual = self.infer(arg)?;
            if !arg_ok(*expected, actual) {
                return Err(format!(
                    "wasm target function literal expects {}, got {}",
                    ty_name(*expected),
                    ty_name(actual)
                ));
            }
        }
        Ok(result)
    }

    fn global_types(&self) -> HashMap<String, Ty> {
        self.env
            .iter()
            .map(|(name, binding)| (name.clone(), binding.ty))
            .collect()
    }

    fn declare_let(&mut self, name: &str, ty: Ty) -> Result<(), String> {
        if self.fn_depth > 0 {
            self.local_scope.insert(name.to_string(), ty);
            return Ok(());
        }
        if let Some(existing) = self.env.get(name) {
            if existing.ty != ty {
                return Err(format!(
                    "wasm target requires stable types: '{}' is {}, assigned {}",
                    name,
                    ty_name(existing.ty),
                    ty_name(ty)
                ));
            }
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
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Ok(*ty);
            }
        }
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
            Expr::Func(..) => self
                .literal_ids
                .get(&(expr as *const Expr as usize))
                .map(|id| Ty::Func(*id))
                .ok_or_else(|| "wasm target lost a function literal".to_string()),
            Expr::Bool(_) => Ok(Ty::Bool),
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
            Expr::Not(inner) => {
                if self.infer(inner)? == Ty::Bool {
                    return Ok(Ty::Bool);
                }
                Err("wasm target negates bools only".to_string())
            }
            Expr::And(left, right) | Expr::Or(left, right) => {
                if self.infer(left)? == Ty::Bool && self.infer(right)? == Ty::Bool {
                    return Ok(Ty::Bool);
                }
                Err("wasm target combines bools with && and || only".to_string())
            }
            Expr::Ternary(cond, then, other, _) => {
                if self.infer(cond)? != Ty::Bool {
                    return Err("wasm target ternaries need a bool condition".to_string());
                }
                let lt = self.infer(then)?;
                let rt = self.infer(other)?;
                if lt == rt {
                    return Ok(lt);
                }
                if is_number(lt) && is_number(rt) {
                    return Ok(if lt == Ty::Float || rt == Ty::Float {
                        Ty::Float
                    } else {
                        Ty::Int
                    });
                }
                Err(format!(
                    "wasm target ternary branches differ: {} and {}",
                    ty_name(lt),
                    ty_name(rt)
                ))
            }
            Expr::Bin(
                op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge),
                left,
                right,
            ) => {
                let lt = self.infer(left)?;
                let rt = self.infer(right)?;
                let comparable = (is_number(lt) && is_number(rt))
                    || (lt == Ty::Str && rt == Ty::Str)
                    || (lt == Ty::Bool && rt == Ty::Bool);
                if !comparable {
                    return Err(format!(
                        "wasm target compares two numbers, strings or bools, got {} and {}",
                        ty_name(lt),
                        ty_name(rt)
                    ));
                }
                let _ = op;
                Ok(Ty::Bool)
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
            Expr::MethodCall(object, key, args, _) => {
                if key == "len" && args.is_empty() {
                    let ty = self.infer(object)?;
                    if matches!(ty, Ty::Str | Ty::Arr) {
                        return Ok(Ty::Int);
                    }
                    return Err(format!(
                        "wasm target .len() works on strings and arrays, got {}",
                        ty_name(ty)
                    ));
                }
                Err(format!(
                    "wasm target does not support the .{} method in expressions yet",
                    key
                ))
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
                        let result = sig.result;
                        let params = sig.params.clone();
                        if result.is_none() {
                            return Err(format!(
                                "wasm target function '{}' returns no value",
                                name
                            ));
                        }
                        if args.len() != params.len() {
                            return Err(format!(
                                "wasm target function '{}' takes {} arguments",
                                name,
                                params.len()
                            ));
                        }
                        for (arg, expected) in args.iter().zip(params.iter()) {
                            let actual = self.infer(arg)?;
                            if !arg_ok(*expected, actual) {
                                return Err(format!(
                                    "wasm target function '{}' expects {}, got {}",
                                    name,
                                    ty_name(*expected),
                                    ty_name(actual)
                                ));
                            }
                        }
                        return Ok(result.expect("checked above"));
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
                    if name == "performance_now" && args.is_empty() {
                        return Ok(Ty::Float);
                    }
                    if name == "gl_pixel" && args.len() == 2 {
                        for arg in args {
                            if self.infer(arg)? != Ty::Int {
                                return Err("wasm target gl_pixel takes ints".to_string());
                            }
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "gl_png" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target gl_png(path) needs a string".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if self.fn_sig(name).is_none() {
                        if let Ok(Ty::Func(id)) = self.infer(callee) {
                            return self.literal_call(id, args);
                        }
                    }
                    if name == "map_new" && args.is_empty() {
                        return Ok(Ty::Map);
                    }
                    if name == "map_get" && args.len() == 2 {
                        if self.infer(&args[0])? != Ty::Map || self.infer(&args[1])? != Ty::Str {
                            return Err("wasm target map_get(map, key) needs a map and a string"
                                .to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "map_has" && args.len() == 2 {
                        if self.infer(&args[0])? != Ty::Map || self.infer(&args[1])? != Ty::Str {
                            return Err("wasm target map_has(map, key) needs a map and a string"
                                .to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "map_len" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Map {
                            return Err("wasm target map_len(map) needs a map".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "now" && args.is_empty() {
                        return Ok(Ty::Int);
                    }
                    if name == "cwd" && args.is_empty() {
                        return Ok(Ty::Str);
                    }
                    if name == "env" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target env(name) needs a string".to_string());
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "file_exists" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target file_exists(path) needs a string".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "str" && args.len() == 1 {
                        let ty = self.infer(&args[0])?;
                        if !is_number(ty) && ty != Ty::Bool {
                            return Err("wasm target str() takes a number or bool".to_string());
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "int" && args.len() == 1 {
                        let ty = self.infer(&args[0])?;
                        if !is_number(ty) && ty != Ty::Str {
                            return Err("wasm target int() takes a number or string".to_string());
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

    fn check_fn_body(
        &mut self,
        params: &[String],
        types: &[Ty],
        body: &[Stmt],
    ) -> Result<(), String> {
        let saved_scope = std::mem::take(&mut self.local_scope);
        let saved_depth = self.fn_depth;
        self.fn_depth += 1;
        for (param, ty) in params.iter().zip(types.iter()) {
            self.local_scope.insert(param.clone(), *ty);
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

    fn check_literal_bodies(&mut self, decls: &[LitDecl]) -> Result<(), String> {
        for (id, decl) in decls.iter().enumerate() {
            let sig = self
                .lit_sigs
                .get(id)
                .cloned()
                .ok_or_else(|| "wasm target lost a function literal".to_string())?;
            let saved_scope = std::mem::take(&mut self.local_scope);
            let saved_depth = self.fn_depth;
            self.fn_depth = 1;
            for (param, ty) in decl.params.iter().zip(sig.params.iter()) {
                self.local_scope.insert(param.clone(), *ty);
            }
            let outcome = (|| -> Result<(), String> {
                for stmt in decl.body {
                    self.check_stmt(stmt)?;
                }
                Ok(())
            })();
            self.local_scope = saved_scope;
            self.fn_depth = saved_depth;
            if let Err(error) = outcome {
                if error.starts_with("undefined variable") {
                    return Err(format!(
                        "wasm target function literals cannot capture outer variables yet ({})",
                        error
                    ));
                }
                return Err(error);
            }
        }
        Ok(())
    }

    fn check_cond(&self, expr: &Expr) -> Result<(), String> {
        match self.infer(expr)? {
            Ty::Bool => Ok(()),
            other => Err(format!(
                "wasm target conditions must be bool, got {}",
                ty_name(other)
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
                match name.as_str() {
                    "map_set" => {
                        if args.len() != 3
                            || self.infer(&args[0])? != Ty::Map
                            || self.infer(&args[1])? != Ty::Str
                            || self.infer(&args[2])? != Ty::Int
                        {
                            return Err(
                                "wasm target map_set(map, key, int) takes a map, a string and an int"
                                    .to_string(),
                            );
                        }
                        return Ok(());
                    }
                    "sleep" => {
                        if args.len() != 1 || self.infer(&args[0])? != Ty::Int {
                            return Err("wasm target sleep(ms) takes an int".to_string());
                        }
                        return Ok(());
                    }
                    "mkdir" => {
                        if args.len() != 1 || self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target mkdir(path) takes a string".to_string());
                        }
                        return Ok(());
                    }
                    "gl_create" => {
                        if args.len() != 2 {
                            return Err("wasm target gl_create(width, height)".to_string());
                        }
                        for arg in args {
                            if self.infer(arg)? != Ty::Int {
                                return Err("wasm target gl_create takes ints".to_string());
                            }
                        }
                        return Ok(());
                    }
                    "gl_clear_color" => {
                        if args.len() != 4 {
                            return Err("wasm target gl_clear_color(r, g, b, a)".to_string());
                        }
                        for arg in args {
                            if !is_number(self.infer(arg)?) {
                                return Err("wasm target gl_clear_color takes numbers".to_string());
                            }
                        }
                        return Ok(());
                    }
                    "gl_clear" => {
                        if !args.is_empty() {
                            return Err("wasm target gl_clear() takes no arguments".to_string());
                        }
                        return Ok(());
                    }
                    "gl_png" => {
                        self.infer(&Expr::Call(
                            Box::new(Expr::Ident("gl_png".to_string())),
                            args.clone(),
                            0,
                        ))?;
                        return Ok(());
                    }
                    _ => {}
                }
                if let Some(sig) = self.fn_sig(name) {
                    let params = sig.params.clone();
                    if args.len() != params.len() {
                        return Err(format!(
                            "wasm target function '{}' takes {} arguments",
                            name,
                            params.len()
                        ));
                    }
                    for (arg, expected) in args.iter().zip(params.iter()) {
                        let actual = self.infer(arg)?;
                        if !arg_ok(*expected, actual) {
                            return Err(format!(
                                "wasm target function '{}' expects {}, got {}",
                                name,
                                ty_name(*expected),
                                ty_name(actual)
                            ));
                        }
                    }
                    return Ok(());
                } else if name != "print" && name != "console_log" {
                    return Err(format!(
                        "wasm target does not support calling '{}' yet",
                        name
                    ));
                }
                if args.len() != 1 {
                    return Err("wasm target supports print(<value>) with one argument".to_string());
                }
                let ty = self.infer(&args[0])?;
                if ty == Ty::Arr || matches!(ty, Ty::Obj(_) | Ty::Map | Ty::Func(_)) {
                    return Err("wasm target cannot log arrays or objects yet".to_string());
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
            Stmt::Expr(Expr::MethodCall(object, name, args, _), _) => {
                let base = self.infer(object)?;
                if base != Ty::Arr {
                    return Err("wasm target supports .push on arrays only".to_string());
                }
                if name == "pop" {
                    if !args.is_empty() {
                        return Err("wasm target .pop takes no arguments".to_string());
                    }
                    return Ok(());
                }
                if name != "push" {
                    return Err(format!(
                        "wasm target does not support the .{} method yet",
                        name
                    ));
                }
                if args.len() != 1 {
                    return Err("wasm target .push takes one value".to_string());
                }
                if self.infer(&args[0])? != Ty::Int {
                    return Err("wasm target arrays hold ints only".to_string());
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
            Stmt::ForIn(key, value, iter, body, _) => {
                let ty = self.infer(iter)?;
                let mut scope = HashMap::new();
                match ty {
                    Ty::Arr => {
                        let (key_name, value_name) = match value {
                            Some(value) => (Some(key.clone()), value.clone()),
                            None => (None, key.clone()),
                        };
                        if let Some(key_name) = key_name {
                            scope.insert(key_name, Ty::Int);
                        }
                        scope.insert(value_name, Ty::Int);
                    }
                    Ty::Map => {
                        scope.insert(key.clone(), Ty::Str);
                        if let Some(value) = value {
                            scope.insert(value.clone(), Ty::Int);
                        }
                    }
                    other => {
                        return Err(format!(
                            "wasm target iterates arrays and maps only, got {}",
                            ty_name(other)
                        ))
                    }
                }
                self.scopes.push(scope);
                let outcome = (|| -> Result<(), String> {
                    for stmt in body {
                        self.check_stmt(stmt)?;
                    }
                    Ok(())
                })();
                self.scopes.pop();
                outcome?;
            }
            Stmt::Return(expr, _) => {
                if self.fn_depth == 0 {
                    return Err("wasm target has no function to return from".to_string());
                }
                if let Some(value) = expr {
                    self.infer(value)?;
                }
            }
            Stmt::Fn(name, params, body, _) => {
                if self.fn_depth > 0 {
                    return Err("wasm target does not support nested functions".to_string());
                }
                let types = match self.fn_sig(name) {
                    Some(sig) => sig.params.clone(),
                    None => vec![Ty::Int; params.len()],
                };
                if types.len() != params.len() {
                    return Err(format!(
                        "wasm target could not infer the parameters of '{}'",
                        name
                    ));
                }
                self.check_fn_body(params, &types, body)?;
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

/// Imported host functions: env.print_i64, env.print_utf8, env.print_f64, env.fmod,
/// bx.argc, bx.arg, bx.read_file, bx.write_file, bx.run, bx.capture,
/// web.console_log_i64, web.console_log_f64, web.console_log_utf8, web.performance_now.
const IMPORT_COUNT: u32 = 37;
const WEB_CONSOLE_I64: u32 = 10;
const WEB_CONSOLE_F64: u32 = 11;
const WEB_CONSOLE_UTF8: u32 = 12;
const WEB_PERFORMANCE_NOW: u32 = 13;
const WEBGL_CREATE: u32 = 14;
const WEBGL_CLEAR_COLOR: u32 = 15;
const WEBGL_CLEAR: u32 = 16;
const WEBGL_READ_PIXEL: u32 = 17;
const WEBGL_TO_PNG: u32 = 18;
const STR_COMPARE: u32 = 19;
const INT_TO_STR: u32 = 20;
const F64_TO_STR: u32 = 21;
const STR_TO_INT: u32 = 22;
const NOW_MS: u32 = 23;
const SLEEP_MS: u32 = 24;
const CWD: u32 = 25;
const ENV_GET: u32 = 26;
const MKDIR: u32 = 27;
const FILE_EXISTS: u32 = 28;
const MAP_NEW: u32 = 29;
const MAP_SET: u32 = 30;
const MAP_GET: u32 = 31;
const MAP_HAS: u32 = 32;
const MAP_LEN: u32 = 33;
const MAP_KEY_AT: u32 = 34;
const MAP_VALUE_AT: u32 = 35;
const BOOL_TO_STR: u32 = 36;

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
    /// Result type of the function currently being emitted (`None` = unit).
    result: Option<Ty>,
    /// Loop-scoped locals (for-in variables), innermost last.
    scoped: Vec<HashMap<String, LocalSlot>>,
    /// AST address -> literal id, resolved statically.
    literal_ids: HashMap<usize, u32>,
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
        for scope in self.scoped.iter().rev() {
            if let Some(slot) = scope.get(name) {
                return Some(*slot);
            }
        }
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

    fn coerce_arg(&mut self, expected: Ty, actual: Ty) -> Result<(), String> {
        if expected == actual {
            return Ok(());
        }
        if expected == Ty::Float && actual == Ty::Int {
            self.builder.push(Instr::Op(0xb9));
            return Ok(());
        }
        Err(format!(
            "wasm target parameter expects {}, got {}",
            ty_name(expected),
            ty_name(actual)
        ))
    }

    /// Resolve a callee to (params, result, wasm index) for direct calls.
    fn callable(&self, callee: &Expr) -> Result<Option<CallableSig>, String> {
        if let Expr::Ident(name) = callee {
            if let Some(sig) = self.checker.fn_sig(name) {
                return Ok(Some((sig.params.clone(), sig.result, sig.index)));
            }
            let local_func = self
                .local_of(name)
                .map(|slot| slot.ty)
                .or_else(|| self.checker.lookup(name).ok());
            if let Some(Ty::Func(id)) = local_func {
                if let Some(sig) = self.checker.lit_sigs.get(id as usize) {
                    return Ok(Some((sig.params.clone(), sig.result, sig.index)));
                }
            }
            return Ok(None);
        }
        if let Ok(Ty::Func(id)) = self.type_of(callee) {
            if let Some(sig) = self.checker.lit_sigs.get(id as usize) {
                return Ok(Some((sig.params.clone(), sig.result, sig.index)));
            }
        }
        Ok(None)
    }

    /// `for k, v in map { .. }`: entry-order key (string) / value (int) locals.
    fn emit_for_in_map(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let map_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I32);
        let count_tmp = self.builder.add_local(ValType::I32);
        let key_ptr_tmp = self.builder.add_local(ValType::I32);
        let mut scope = HashMap::new();
        let key_slot = self.builder.add_local(ValType::I32);
        self.builder.add_local(ValType::I32);
        scope.insert(
            key.to_string(),
            LocalSlot {
                ty: Ty::Str,
                slot: key_slot,
            },
        );
        let value_slot = value.map(|value| {
            let slot = self.builder.add_local(ValType::I64);
            scope.insert(value.to_string(), LocalSlot { ty: Ty::Int, slot });
            slot
        });
        self.scoped.push(scope);
        self.emit_expr(iter)?;
        self.builder.push(Instr::Local(0x21, map_tmp));
        self.builder.push(Instr::Local(0x20, map_tmp));
        self.builder.push(Instr::Call(MAP_LEN));
        self.builder.push(Instr::Local(0x21, count_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(0)));
        self.builder.push(Instr::Local(0x21, index_tmp));
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
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Local(0x20, count_tmp));
        self.builder.push(Instr::Op(0x48));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        // key = map_key_at(map, index) -> [len][utf8] block; store ptr+4/len
        self.builder.push(Instr::Local(0x20, map_tmp));
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Call(MAP_KEY_AT));
        self.builder.push(Instr::Local(0x21, key_ptr_tmp));
        self.builder.push(Instr::Local(0x20, key_ptr_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, key_ptr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, key_slot + 1));
        self.builder.push(Instr::Local(0x21, key_slot));
        if let Some(value_slot) = value_slot {
            self.builder.push(Instr::Local(0x20, map_tmp));
            self.builder.push(Instr::Local(0x20, index_tmp));
            self.builder.push(Instr::Call(MAP_VALUE_AT));
            self.builder.push(Instr::Local(0x21, value_slot));
        }
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, index_tmp));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.scoped.pop();
        Ok(())
    }

    /// `for k, v in array { .. }`: index/value locals advanced over the array.
    fn emit_for_in(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let iter_ty = self.type_of(iter)?;
        if iter_ty == Ty::Map {
            return self.emit_for_in_map(key, value, iter, body);
        }
        if iter_ty != Ty::Arr {
            return Err(format!(
                "wasm target iterates arrays and maps only, got {}",
                ty_name(iter_ty)
            ));
        }
        let array_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I64);
        let (key_name, value_name) = match value {
            Some(value) => (Some(key), value),
            None => (None, key),
        };
        let mut scope = HashMap::new();
        let value_tmp = self.builder.add_local(ValType::I64);
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::Int,
                slot: value_tmp,
            },
        );
        let key_tmp = key_name.map(|name| {
            let tmp = self.builder.add_local(ValType::I64);
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: tmp,
                },
            );
            tmp
        });
        self.scoped.push(scope);
        self.emit_expr(iter)?;
        self.builder.push(Instr::Local(0x21, array_tmp));
        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
        self.builder.push(Instr::Local(0x21, index_tmp));
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
        // condition: index < len(array)
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Local(0x20, array_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Op(0xad));
        self.builder.push(Instr::Op(0x53));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        if let Some(key_tmp) = key_tmp {
            self.builder.push(Instr::Local(0x20, index_tmp));
            self.builder.push(Instr::Local(0x21, key_tmp));
        }
        self.builder.push(Instr::Local(0x20, array_tmp));
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Op(0xa7));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Mem(0x29, 3, 0));
        self.builder.push(Instr::Local(0x21, value_tmp));
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Const(0x42, WVal::I64(1)));
        self.builder.push(Instr::Op(0x7c));
        self.builder.push(Instr::Local(0x21, index_tmp));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.scoped.pop();
        Ok(())
    }

    /// `arr.pop()` statement: shrink the length when the array is not empty
    /// (bx returns the removed value in an expression; that is not supported yet).
    fn emit_pop(&mut self, name: &str) -> Result<(), String> {
        let array_tmp = self.builder.add_local(ValType::I32);
        self.emit_load(name)?;
        self.builder.push(Instr::Local(0x21, array_tmp));
        self.builder.push(Instr::Local(0x20, array_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(0)));
        self.builder.push(Instr::Op(0x4a));
        self.builder.push(Instr::Block {
            opcode: 0x04,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x04);
        self.builder.push(Instr::Local(0x20, array_tmp));
        self.builder.push(Instr::Local(0x20, array_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6b));
        self.builder.push(Instr::Mem(0x36, 2, 0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        Ok(())
    }

    /// `arr.push(value)`: allocate a larger block, copy the old one, append and
    /// write the new pointer back into the array variable.
    fn emit_push(&mut self, name: &str, value: &Expr) -> Result<(), String> {
        let arr_tmp = self.builder.add_local(ValType::I32);
        let new_tmp = self.builder.add_local(ValType::I32);
        let val_tmp = self.builder.add_local(ValType::I64);
        let value_ty = self.emit_expr(value)?;
        if value_ty != Ty::Int {
            return Err("wasm target arrays hold ints only".to_string());
        }
        self.builder.push(Instr::Local(0x21, val_tmp));
        self.emit_load(name)?;
        self.builder.push(Instr::Local(0x21, arr_tmp));
        // size = 4 + 8 * (len + 1)
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(12)));
        self.builder.push(Instr::Op(0x6a));
        self.emit_alloc_dynamic();
        self.builder.push(Instr::Local(0x21, new_tmp));
        // copy [ptr, ptr + 4 + 8*len)
        self.builder.push(Instr::Local(0x20, new_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::MemoryCopy);
        // len += 1
        self.builder.push(Instr::Local(0x20, new_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Mem(0x36, 2, 0));
        // new_ptr[4 + 8*len] = value
        self.builder.push(Instr::Local(0x20, new_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, val_tmp));
        self.builder.push(Instr::Mem(0x37, 3, 0));
        // store the new pointer back into the variable
        self.builder.push(Instr::Local(0x20, new_tmp));
        self.emit_store(name)?;
        Ok(())
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
                if matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                ) {
                    let comparable = (is_number(lt) && is_number(rt))
                        || (lt == Ty::Str && rt == Ty::Str)
                        || (lt == Ty::Bool && rt == Ty::Bool);
                    if !comparable {
                        return Err(format!(
                            "wasm target compares two numbers, strings or bools, got {} and {}",
                            ty_name(lt),
                            ty_name(rt)
                        ));
                    }
                    return Ok(Ty::Bool);
                }
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
            Expr::Not(inner) => {
                if self.type_of(inner)? == Ty::Bool {
                    return Ok(Ty::Bool);
                }
                Err("wasm target negates bools only".to_string())
            }
            Expr::And(left, right) | Expr::Or(left, right) => {
                if self.type_of(left)? == Ty::Bool && self.type_of(right)? == Ty::Bool {
                    return Ok(Ty::Bool);
                }
                Err("wasm target combines bools with && and || only".to_string())
            }
            Expr::Ternary(cond, then, other, _) => {
                if self.type_of(cond)? != Ty::Bool {
                    return Err("wasm target ternaries need a bool condition".to_string());
                }
                let lt = self.type_of(then)?;
                let rt = self.type_of(other)?;
                if lt == rt {
                    return Ok(lt);
                }
                if is_number(lt) && is_number(rt) {
                    return Ok(if lt == Ty::Float || rt == Ty::Float {
                        Ty::Float
                    } else {
                        Ty::Int
                    });
                }
                Err(format!(
                    "wasm target ternary branches differ: {} and {}",
                    ty_name(lt),
                    ty_name(rt)
                ))
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
                if lt == Ty::Str && rt == Ty::Str {
                    self.emit_str(left)?;
                    self.emit_str(right)?;
                    self.builder.push(Instr::Call(STR_COMPARE));
                    match op {
                        BinOp::Eq => self.builder.push(Instr::Op(0x45)),
                        BinOp::Ne => {
                            self.builder.push(Instr::Op(0x45));
                            self.builder.push(Instr::Op(0x45));
                        }
                        BinOp::Lt => {
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Op(0x48));
                        }
                        BinOp::Gt => {
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Op(0x4a));
                        }
                        BinOp::Le => {
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Op(0x4c));
                        }
                        _ => {
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Op(0x4e));
                        }
                    }
                    return Ok(());
                }
                if !is_number(lt) || !is_number(rt) {
                    return Err(format!(
                        "wasm target compares two numbers or two strings, got {} and {}",
                        ty_name(lt),
                        ty_name(rt)
                    ));
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
            other => {
                if self.type_of(other)? == Ty::Bool {
                    self.emit_expr(other)?;
                    return Ok(());
                }
                Err(format!(
                    "wasm target conditions must be bool, got {}",
                    expr_kind(other)
                ))
            }
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
            other => {
                if self.type_of(other)? == Ty::Str {
                    self.emit_expr(other)?;
                    return Ok(());
                }
                Err(format!(
                    "wasm target does not support this string expression yet: {}",
                    expr_kind(other)
                ))
            }
        }
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<Ty, String> {
        if string_value(expr).is_some() {
            self.emit_str(expr)?;
            return Ok(Ty::Str);
        }
        match expr {
            Expr::Func(..) => {
                let id = self
                    .literal_ids
                    .get(&(expr as *const Expr as usize))
                    .copied()
                    .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                Ok(Ty::Func(id))
            }
            Expr::Int(value) => {
                self.builder.push(Instr::Const(0x42, WVal::I64(*value)));
                Ok(Ty::Int)
            }
            Expr::Float(value) => {
                self.builder.push(Instr::Const(0x44, WVal::F64(*value)));
                Ok(Ty::Float)
            }
            Expr::Bool(value) => {
                self.builder
                    .push(Instr::Const(0x41, WVal::I32(*value as i32)));
                Ok(Ty::Bool)
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
                    Ty::Int
                    | Ty::Float
                    | Ty::Arr
                    | Ty::Obj(_)
                    | Ty::Map
                    | Ty::Func(_)
                    | Ty::Bool => self.push_global(name, false)?,
                }
                Ok(ty)
            }
            Expr::Not(inner) => {
                self.emit_expr(inner)?;
                self.builder.push(Instr::Op(0x45));
                Ok(Ty::Bool)
            }
            Expr::And(left, right) => {
                let slot = self.builder.add_local(ValType::I32);
                self.emit_cond(left)?;
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.emit_cond(right)?;
                self.builder.push(Instr::Local(0x21, slot));
                self.builder.push(Instr::Else { end: 0 });
                self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                self.builder.push(Instr::Local(0x21, slot));
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.builder.push(Instr::Local(0x20, slot));
                Ok(Ty::Bool)
            }
            Expr::Or(left, right) => {
                let slot = self.builder.add_local(ValType::I32);
                self.emit_cond(left)?;
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                self.builder.push(Instr::Local(0x21, slot));
                self.builder.push(Instr::Else { end: 0 });
                self.emit_cond(right)?;
                self.builder.push(Instr::Local(0x21, slot));
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.builder.push(Instr::Local(0x20, slot));
                Ok(Ty::Bool)
            }
            Expr::Ternary(cond, then, other, _) => {
                let ty = self.checker.infer(expr)?;
                if ty == Ty::Str {
                    return Err("wasm target string ternaries are not supported yet".to_string());
                }
                let slot_ty = match ty {
                    Ty::Float => ValType::F64,
                    Ty::Int => ValType::I64,
                    _ => ValType::I32,
                };
                let slot = self.builder.add_local(slot_ty);
                self.emit_cond(cond)?;
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                if ty == Ty::Float {
                    self.emit_f64(then)?;
                } else {
                    self.emit_expr(then)?;
                }
                self.builder.push(Instr::Local(0x21, slot));
                self.builder.push(Instr::Else { end: 0 });
                if ty == Ty::Float {
                    self.emit_f64(other)?;
                } else {
                    self.emit_expr(other)?;
                }
                self.builder.push(Instr::Local(0x21, slot));
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.builder.push(Instr::Local(0x20, slot));
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
            Expr::Bin(op, left, right) => {
                if matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                ) {
                    self.emit_cond(expr)?;
                    return Ok(Ty::Bool);
                }
                self.emit_numeric(*op, left, right)
            }
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
            Expr::MethodCall(object, key, args, _) => {
                if key == "len" && args.is_empty() {
                    return match self.type_of(object)? {
                        Ty::Arr => {
                            self.emit_expr(object)?;
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                            self.builder.push(Instr::Op(0xad));
                            Ok(Ty::Int)
                        }
                        Ty::Str => {
                            self.emit_str(object)?;
                            self.builder.push(Instr::Local(0x21, ARG_TMP));
                            self.builder.push(Instr::Op(0x1a));
                            self.builder.push(Instr::Local(0x20, ARG_TMP));
                            self.builder.push(Instr::Op(0xad));
                            Ok(Ty::Int)
                        }
                        other => Err(format!(
                            "wasm target .len() works on strings and arrays, got {}",
                            ty_name(other)
                        )),
                    };
                }
                Err(format!(
                    "wasm target does not support the .{} method in expressions yet",
                    key
                ))
            }
            Expr::Call(callee, args, _) => {
                if let Some((params, result, index)) = self.callable(callee)? {
                    let Some(result) = result else {
                        return Err("wasm target function literal returns no value".to_string());
                    };
                    if args.len() != params.len() {
                        return Err(
                            "wasm target function literal takes a different number of arguments"
                                .to_string(),
                        );
                    }
                    for (arg, expected) in args.iter().zip(params.iter()) {
                        let actual = self.emit_expr(arg)?;
                        self.coerce_arg(*expected, actual)?;
                    }
                    self.builder.push(Instr::Call(index));
                    return Ok(result);
                }
                if let Expr::Ident(name) = &**callee {
                    if let Some(sig) = self.checker.fn_sig(name).cloned() {
                        let Some(result) = sig.result else {
                            return Err(format!(
                                "wasm target function '{}' returns no value",
                                name
                            ));
                        };
                        if args.len() != sig.params.len() {
                            return Err(format!(
                                "wasm target function '{}' takes {} arguments",
                                name,
                                sig.params.len()
                            ));
                        }
                        for (arg, expected) in args.iter().zip(sig.params.iter()) {
                            let actual = self.emit_expr(arg)?;
                            self.coerce_arg(*expected, actual)?;
                        }
                        self.builder.push(Instr::Call(sig.index));
                        return Ok(result);
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
                    if name == "performance_now" && args.is_empty() {
                        self.builder.push(Instr::Call(WEB_PERFORMANCE_NOW));
                        return Ok(Ty::Float);
                    }
                    if name == "gl_pixel" && args.len() == 2 {
                        for arg in args {
                            self.emit_expr(arg)?;
                            self.builder.push(Instr::Op(0xa7));
                        }
                        self.builder.push(Instr::Call(WEBGL_READ_PIXEL));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                    if name == "gl_png" && args.len() == 1 {
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(WEBGL_TO_PNG));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }

                    if name == "map_new" && args.is_empty() {
                        self.builder.push(Instr::Call(MAP_NEW));
                        return Ok(Ty::Map);
                    }
                    if name == "map_get" && args.len() == 2 {
                        self.emit_expr(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Call(MAP_GET));
                        return Ok(Ty::Int);
                    }
                    if name == "map_has" && args.len() == 2 {
                        self.emit_expr(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Call(MAP_HAS));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                    if name == "map_len" && args.len() == 1 {
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Call(MAP_LEN));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                    if name == "now" && args.is_empty() {
                        self.builder.push(Instr::Call(NOW_MS));
                        return Ok(Ty::Int);
                    }
                    if name == "cwd" && args.is_empty() {
                        self.builder.push(Instr::Call(CWD));
                        self.builder.push(Instr::Local(0x21, ARG_TMP));
                        self.builder.push(Instr::Local(0x20, ARG_TMP));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, ARG_TMP));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        return Ok(Ty::Str);
                    }
                    if name == "env" && args.len() == 1 {
                        if self.type_of(&args[0])? != Ty::Str {
                            return Err("wasm target env(name) needs a string".to_string());
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(ENV_GET));
                        self.builder.push(Instr::Local(0x21, ARG_TMP));
                        self.builder.push(Instr::Local(0x20, ARG_TMP));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, ARG_TMP));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        return Ok(Ty::Str);
                    }
                    if name == "file_exists" && args.len() == 1 {
                        if self.type_of(&args[0])? != Ty::Str {
                            return Err("wasm target file_exists(path) needs a string".to_string());
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(FILE_EXISTS));
                        self.builder.push(Instr::Op(0xad));
                        return Ok(Ty::Int);
                    }
                    if name == "str" && args.len() == 1 {
                        let ty = self.type_of(&args[0])?;
                        if ty == Ty::Bool {
                            self.emit_expr(&args[0])?;
                            self.builder.push(Instr::Call(BOOL_TO_STR));
                            self.builder.push(Instr::Local(0x21, ARG_TMP));
                            self.builder.push(Instr::Local(0x20, ARG_TMP));
                            self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x20, ARG_TMP));
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                            return Ok(Ty::Str);
                        }
                        if ty == Ty::Float {
                            self.emit_f64(&args[0])?;
                            self.builder.push(Instr::Call(F64_TO_STR));
                        } else {
                            self.emit_expr(&args[0])?;
                            self.builder.push(Instr::Call(INT_TO_STR));
                        }
                        self.builder.push(Instr::Local(0x21, ARG_TMP));
                        self.builder.push(Instr::Local(0x20, ARG_TMP));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, ARG_TMP));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        return Ok(Ty::Str);
                    }
                    if name == "int" && args.len() == 1 {
                        match self.type_of(&args[0])? {
                            Ty::Str => {
                                self.emit_str(&args[0])?;
                                self.builder.push(Instr::Call(STR_TO_INT));
                            }
                            Ty::Float => {
                                self.emit_f64(&args[0])?;
                                self.builder.push(Instr::Op(0xb0));
                            }
                            _ => {
                                self.emit_expr(&args[0])?;
                            }
                        }
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
                        Ty::Arr | Ty::Obj(_) | Ty::Map | Ty::Func(_) | Ty::Bool => {
                            self.builder.add_local(ValType::I32)
                        }
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
                if let Some((params, result, index)) = self.callable(callee)? {
                    if args.len() != params.len() {
                        return Err(
                            "wasm target function literal takes a different number of arguments"
                                .to_string(),
                        );
                    }
                    for (arg, expected) in args.iter().zip(params.iter()) {
                        let actual = self.emit_expr(arg)?;
                        self.coerce_arg(*expected, actual)?;
                    }
                    self.builder.push(Instr::Call(index));
                    if let Some(result) = result {
                        self.builder.push(Instr::Op(0x1a));
                        if result == Ty::Str {
                            self.builder.push(Instr::Op(0x1a));
                        }
                    }
                    return Ok(());
                }
                let Expr::Ident(name) = &**callee else {
                    return Err("wasm target only supports calls to print()".to_string());
                };
                if let Some(sig) = self.checker.fn_sig(name).cloned() {
                    if args.len() != sig.params.len() {
                        return Err(format!(
                            "wasm target function '{}' takes {} arguments",
                            name,
                            sig.params.len()
                        ));
                    }
                    for (arg, expected) in args.iter().zip(sig.params.iter()) {
                        let actual = self.emit_expr(arg)?;
                        self.coerce_arg(*expected, actual)?;
                    }
                    self.builder.push(Instr::Call(sig.index));
                    if let Some(result) = sig.result {
                        self.builder.push(Instr::Op(0x1a));
                        if result == Ty::Str {
                            self.builder.push(Instr::Op(0x1a));
                        }
                    }
                    return Ok(());
                }
                match name.as_str() {
                    "map_set" => {
                        if args.len() != 3
                            || self.type_of(&args[0])? != Ty::Map
                            || self.type_of(&args[1])? != Ty::Str
                            || self.type_of(&args[2])? != Ty::Int
                        {
                            return Err(
                                "wasm target map_set(map, key, int) takes a map, a string and an int"
                                    .to_string(),
                            );
                        }
                        self.emit_expr(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.emit_expr(&args[2])?;
                        self.builder.push(Instr::Call(MAP_SET));
                        self.builder.push(Instr::Op(0x1a));
                        return Ok(());
                    }
                    "sleep" => {
                        if args.len() != 1 || self.type_of(&args[0])? != Ty::Int {
                            return Err("wasm target sleep(ms) takes an int".to_string());
                        }
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Call(SLEEP_MS));
                        return Ok(());
                    }
                    "mkdir" => {
                        if args.len() != 1 || self.type_of(&args[0])? != Ty::Str {
                            return Err("wasm target mkdir(path) takes a string".to_string());
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(MKDIR));
                        self.builder.push(Instr::Op(0x1a));
                        return Ok(());
                    }
                    "gl_create" => {
                        for arg in args {
                            self.emit_expr(arg)?;
                            self.builder.push(Instr::Op(0xa7));
                        }
                        self.builder.push(Instr::Call(WEBGL_CREATE));
                        self.builder.push(Instr::Op(0x1a));
                        return Ok(());
                    }
                    "gl_clear_color" => {
                        for arg in args {
                            self.emit_f64(arg)?;
                        }
                        self.builder.push(Instr::Call(WEBGL_CLEAR_COLOR));
                        return Ok(());
                    }
                    "gl_clear" => {
                        self.builder.push(Instr::Call(WEBGL_CLEAR));
                        return Ok(());
                    }
                    "gl_png" => {
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(WEBGL_TO_PNG));
                        self.builder.push(Instr::Op(0x1a));
                        return Ok(());
                    }
                    _ => {}
                }
                if (name != "print" && name != "console_log") || args.len() != 1 {
                    return Err(
                        "wasm target supports print(<value>) / console_log(<value>) with one argument"
                            .to_string(),
                    );
                }
                let web = name == "console_log";
                let ty = self.emit_expr(&args[0])?;
                match ty {
                    Ty::Bool => {
                        self.builder.push(Instr::Call(BOOL_TO_STR));
                        self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder
                            .push(Instr::Call(if web { WEB_CONSOLE_UTF8 } else { 1 }));
                    }
                    Ty::Str => {
                        self.builder
                            .push(Instr::Call(if web { WEB_CONSOLE_UTF8 } else { 1 }));
                    }
                    Ty::Float => {
                        self.builder
                            .push(Instr::Call(if web { WEB_CONSOLE_F64 } else { 8 }));
                    }
                    _ => {
                        if web {
                            self.builder.push(Instr::Call(if ty == Ty::Float {
                                WEB_CONSOLE_F64
                            } else {
                                WEB_CONSOLE_I64
                            }));
                        } else {
                            self.builder.push(Instr::Call(0));
                            self.builder.push(Instr::Op(0x1a));
                        }
                    }
                }
                Ok(())
            }
            Stmt::ForIn(key, value, iter, body, _) => {
                self.emit_for_in(key, value.as_deref(), iter, body)
            }
            Stmt::Expr(Expr::MethodCall(object, name, args, _), _) => {
                let Expr::Ident(array) = &**object else {
                    return Err("wasm target supports .push on array variables only".to_string());
                };
                if name == "pop" {
                    if !args.is_empty() {
                        return Err("wasm target .pop takes no arguments".to_string());
                    }
                    return self.emit_pop(array);
                }
                if name != "push" || args.len() != 1 {
                    return Err(format!(
                        "wasm target does not support the .{} method yet",
                        name
                    ));
                }
                self.emit_push(array, &args[0])?;
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
                    Ty::Int
                    | Ty::Float
                    | Ty::Arr
                    | Ty::Obj(_)
                    | Ty::Map
                    | Ty::Func(_)
                    | Ty::Bool => self.builder.push(Instr::Op(0x1a)),
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
                match (expr, self.result) {
                    (Some(expr), Some(expected)) => {
                        let actual = self.emit_expr(expr)?;
                        if expected == Ty::Float && actual == Ty::Int {
                            self.builder.push(Instr::Op(0xb9));
                        } else if expected != actual {
                            return Err(format!(
                                "wasm target function returns {}, got {}",
                                ty_name(expected),
                                ty_name(actual)
                            ));
                        }
                    }
                    (Some(_), None) => {
                        return Err(
                            "wasm target function has no result type; return without a value"
                                .to_string(),
                        )
                    }
                    (None, Some(expected)) => {
                        return Err(format!(
                            "wasm target function returns {}; return needs a value",
                            ty_name(expected)
                        ))
                    }
                    (None, None) => {}
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

struct FnDecl {
    name: String,
    params: Vec<String>,
    body: Vec<Stmt>,
}

struct LitDecl<'a> {
    params: Vec<String>,
    body: &'a [Stmt],
}

fn collect_literal_nodes<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Expr>) {
    for stmt in stmts {
        collect_stmt_literals(stmt, out);
    }
}

fn collect_stmt_literals<'a>(stmt: &'a Stmt, out: &mut Vec<&'a Expr>) {
    match stmt {
        Stmt::Let(_, Some(expr), _) => collect_expr_literals(expr, out),
        Stmt::Expr(expr, _) => collect_expr_literals(expr, out),
        Stmt::Fn(_, _, body, _) => collect_literal_nodes(body, out),
        Stmt::Block(body) => collect_literal_nodes(body, out),
        Stmt::If(cond, then_body, else_body, _) => {
            collect_expr_literals(cond, out);
            collect_literal_nodes(then_body, out);
            if let Some(else_body) = else_body {
                collect_literal_nodes(else_body, out);
            }
        }
        Stmt::While(cond, body, _) => {
            collect_expr_literals(cond, out);
            collect_literal_nodes(body, out);
        }
        Stmt::ForC(init, cond, step, body, _) => {
            if let Some(init) = init {
                collect_stmt_literals(init, out);
            }
            if let Some(cond) = cond {
                collect_expr_literals(cond, out);
            }
            if let Some(step) = step {
                collect_expr_literals(step, out);
            }
            collect_literal_nodes(body, out);
        }
        Stmt::ForIn(_, _, iter, body, _) => {
            collect_expr_literals(iter, out);
            collect_literal_nodes(body, out);
        }
        Stmt::Return(Some(expr), _) => collect_expr_literals(expr, out),
        _ => {}
    }
}

fn collect_expr_literals<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Func(_, body, _) => {
            out.push(expr);
            collect_literal_nodes(body, out);
        }
        Expr::Neg(inner) | Expr::Not(inner) => collect_expr_literals(inner, out),
        Expr::Bin(_, left, right) | Expr::And(left, right) | Expr::Or(left, right) => {
            collect_expr_literals(left, out);
            collect_expr_literals(right, out);
        }
        Expr::Ternary(cond, then, other, _) => {
            collect_expr_literals(cond, out);
            collect_expr_literals(then, out);
            collect_expr_literals(other, out);
        }
        Expr::Call(callee, args, _) => {
            collect_expr_literals(callee, out);
            for arg in args {
                collect_expr_literals(arg, out);
            }
        }
        Expr::MethodCall(object, _, args, _) => {
            collect_expr_literals(object, out);
            for arg in args {
                collect_expr_literals(arg, out);
            }
        }
        Expr::Index(array, index, _) => {
            collect_expr_literals(array, out);
            collect_expr_literals(index, out);
        }
        Expr::Member(object, _, _) => collect_expr_literals(object, out),
        Expr::Assign(target, value, _) => {
            collect_expr_literals(target, out);
            collect_expr_literals(value, out);
        }
        Expr::Compound(target, _, value, _) => {
            collect_expr_literals(target, out);
            collect_expr_literals(value, out);
        }
        Expr::Array(items) => {
            for item in items {
                collect_expr_literals(item, out);
            }
        }
        Expr::Object(fields) => {
            for (_, value) in fields {
                collect_expr_literals(value, out);
            }
        }
        _ => {}
    }
}

/// `(parameter types, result type, wasm function index)` resolved statically.
type CallableSig = (Vec<Ty>, Option<Ty>, u32);

/// Soft (partial) type inference for call sites and returns: `None` means the
/// type is not known yet, so a later fixpoint iteration may resolve it.
struct InferState<'a> {
    index: &'a HashMap<String, usize>,
    globals: &'a HashMap<String, Ty>,
    shapes: &'a RefCell<Vec<Shape>>,
    literal_ids: &'a HashMap<usize, u32>,
    decl_count: usize,
    params: Vec<Vec<Option<Ty>>>,
    results: Vec<Option<Ty>>,
}

impl InferState<'_> {
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
}

impl<'a> InferState<'a> {
    fn soft(&self, expr: &Expr, locals: &HashMap<String, Ty>) -> Option<Ty> {
        match expr {
            Expr::Func(..) => self
                .literal_ids
                .get(&(expr as *const Expr as usize))
                .map(|id| Ty::Func(*id)),
            Expr::Bool(_) => Some(Ty::Bool),
            Expr::Not(inner) => {
                self.soft(inner, locals)?;
                Some(Ty::Bool)
            }
            Expr::And(left, right) | Expr::Or(left, right) => {
                self.soft(left, locals)?;
                self.soft(right, locals)?;
                Some(Ty::Bool)
            }

            Expr::Int(_) => Some(Ty::Int),
            Expr::Float(_) => Some(Ty::Float),
            Expr::Str(_) => Some(Ty::Str),
            Expr::Ident(name) => locals
                .get(name)
                .copied()
                .or_else(|| self.globals.get(name).copied()),
            Expr::Neg(inner) => match self.soft(inner, locals)? {
                Ty::Float => Some(Ty::Float),
                _ => Some(Ty::Int),
            },
            Expr::Bin(
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
                left,
                right,
            ) => {
                self.soft(left, locals)?;
                self.soft(right, locals)?;
                Some(Ty::Bool)
            }
            Expr::Bin(op, left, right) => {
                let lt = self.soft(left, locals)?;
                let rt = self.soft(right, locals)?;
                if *op == BinOp::Add && lt == Ty::Str && rt == Ty::Str {
                    return Some(Ty::Str);
                }
                if !is_number(lt) || !is_number(rt) {
                    return Some(Ty::Int);
                }
                if *op == BinOp::Div || lt == Ty::Float || rt == Ty::Float {
                    Some(Ty::Float)
                } else {
                    Some(Ty::Int)
                }
            }
            Expr::Ternary(cond, then, other, _) => {
                self.soft(cond, locals)?;
                self.soft(then, locals).or_else(|| self.soft(other, locals))
            }
            Expr::Array(items) => {
                for item in items {
                    self.soft(item, locals)?;
                }
                Some(Ty::Arr)
            }
            Expr::Object(fields) => {
                let mut keys = Vec::new();
                for (key, value) in fields {
                    self.soft(value, locals)?;
                    keys.push(key.clone());
                }
                Some(Ty::Obj(self.shape_id(&keys)))
            }
            Expr::Member(object, _, _) => {
                self.soft(object, locals)?;
                Some(Ty::Int)
            }
            Expr::Index(array, index, _) => {
                self.soft(array, locals)?;
                self.soft(index, locals)?;
                Some(Ty::Int)
            }
            Expr::Call(callee, args, _) => {
                if let Some(Ty::Func(id)) = self.soft(callee, locals) {
                    for arg in args {
                        self.soft(arg, locals)?;
                    }
                    return self
                        .results
                        .get(self.decl_count + id as usize)
                        .copied()
                        .flatten();
                }
                let Expr::Ident(name) = &**callee else {
                    return None;
                };
                if let Some(index) = self.index.get(name) {
                    return self.results.get(*index).copied().flatten();
                }
                match name.as_str() {
                    "len" | "argc" | "write_file" | "run" | "gl_pixel" | "gl_png" => Some(Ty::Int),
                    "arg" | "read_file" | "capture" => Some(Ty::Str),
                    "performance_now" => Some(Ty::Float),
                    _ => {
                        for arg in args {
                            self.soft(arg, locals)?;
                        }
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn unify_param(&mut self, index: usize, slot: usize, ty: Ty) -> Result<bool, String> {
        let Some(value) = self.params[index].get_mut(slot) else {
            return Ok(false);
        };
        match value {
            None => {
                *value = Some(ty);
                Ok(true)
            }
            Some(current) => {
                if *current == ty {
                    return Ok(false);
                }
                if is_number(*current) && is_number(ty) {
                    if ty == Ty::Float && *current == Ty::Int {
                        *current = Ty::Float;
                        return Ok(true);
                    }
                    return Ok(false);
                }
                Err(format!(
                    "wasm target function parameter is called with both {} and {} values",
                    ty_name(*current),
                    ty_name(ty)
                ))
            }
        }
    }

    fn unify_result(&mut self, index: usize, ty: Ty) -> Result<bool, String> {
        match self.results[index] {
            None => {
                self.results[index] = Some(ty);
                Ok(true)
            }
            Some(current) if current == ty => Ok(false),
            Some(current) if is_number(current) && is_number(ty) => {
                if ty == Ty::Float && current == Ty::Int {
                    self.results[index] = Some(Ty::Float);
                    return Ok(true);
                }
                Ok(false)
            }
            Some(current) => Err(format!(
                "wasm target function returns both {} and {} values",
                ty_name(current),
                ty_name(ty)
            )),
        }
    }
    fn walk(
        &mut self,
        stmts: &[Stmt],
        locals: &mut HashMap<String, Ty>,
        fn_index: Option<usize>,
    ) -> Result<bool, String> {
        let mut changed = false;
        for stmt in stmts {
            match stmt {
                Stmt::Let(name, init, _) => {
                    let ty = init
                        .as_ref()
                        .and_then(|expr| self.soft(expr, locals))
                        .unwrap_or(Ty::Int);
                    locals.insert(name.clone(), ty);
                }
                Stmt::Expr(expr, _) => {
                    changed |= self.walk_expr(expr, locals)?;
                }
                Stmt::Fn(..) => {}
                Stmt::Block(body) => changed |= self.walk(body, &mut locals.clone(), fn_index)?,
                Stmt::If(cond, then, other, _) => {
                    changed |= self.walk_expr(cond, locals)?;
                    changed |= self.walk(then, &mut locals.clone(), fn_index)?;
                    if let Some(other) = other {
                        changed |= self.walk(other, &mut locals.clone(), fn_index)?;
                    }
                }
                Stmt::While(cond, body, _) => {
                    changed |= self.walk_expr(cond, locals)?;
                    changed |= self.walk(body, &mut locals.clone(), fn_index)?;
                }
                Stmt::ForC(init, cond, step, body, _) => {
                    if let Some(init) = init {
                        changed |= self.walk(std::slice::from_ref(init), locals, fn_index)?;
                    }
                    if let Some(cond) = cond {
                        changed |= self.walk_expr(cond, locals)?;
                    }
                    if let Some(step) = step {
                        changed |= self.walk_expr(step, locals)?;
                    }
                    changed |= self.walk(body, &mut locals.clone(), fn_index)?;
                }
                Stmt::Return(expr, _) => {
                    if let (Some(expr), Some(index)) = (expr, fn_index) {
                        if let Some(ty) = self.soft(expr, locals) {
                            changed |= self.unify_result(index, ty)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(changed)
    }

    fn walk_expr(&mut self, expr: &Expr, locals: &HashMap<String, Ty>) -> Result<bool, String> {
        match expr {
            Expr::Call(callee, args, _) => {
                let mut changed = false;
                if let Expr::Ident(name) = &**callee {
                    if let Some(index) = self.index.get(name).copied() {
                        for (slot, arg) in args.iter().enumerate() {
                            if let Some(ty) = self.soft(arg, locals) {
                                changed |= self.unify_param(index, slot, ty)?;
                            }
                        }
                        return Ok(changed);
                    }
                }
                if let Some(Ty::Func(id)) = self.soft(callee, locals) {
                    let index = self.decl_count + id as usize;
                    for (slot, arg) in args.iter().enumerate() {
                        if let Some(ty) = self.soft(arg, locals) {
                            changed |= self.unify_param(index, slot, ty)?;
                        }
                    }
                    return Ok(changed);
                }
                for arg in args {
                    changed |= self.walk_expr(arg, locals)?;
                }
                Ok(changed)
            }
            Expr::Bin(_, left, right) => {
                Ok(self.walk_expr(left, locals)? | self.walk_expr(right, locals)?)
            }
            Expr::Neg(inner) | Expr::Not(inner) => self.walk_expr(inner, locals),
            Expr::Assign(target, value, _) => {
                let mut changed = self.walk_expr(value, locals)?;
                changed |= self.walk_expr(target, locals)?;
                Ok(changed)
            }
            Expr::Compound(target, _, value, _) => {
                self.walk_expr(target, locals)?;
                self.walk_expr(value, locals)
            }
            Expr::Ternary(cond, then, other, _) => {
                self.walk_expr(cond, locals)?;
                self.walk_expr(then, locals)?;
                self.walk_expr(other, locals)
            }
            Expr::Array(items) => {
                let mut changed = false;
                for item in items {
                    changed |= self.walk_expr(item, locals)?;
                }
                Ok(changed)
            }
            Expr::Object(fields) => {
                let mut changed = false;
                for (_, value) in fields {
                    changed |= self.walk_expr(value, locals)?;
                }
                Ok(changed)
            }
            Expr::Index(array, index, _) => {
                Ok(self.walk_expr(array, locals)? | self.walk_expr(index, locals)?)
            }
            Expr::Member(object, _, _) => self.walk_expr(object, locals),
            Expr::And(left, right) | Expr::Or(left, right) => {
                Ok(self.walk_expr(left, locals)? | self.walk_expr(right, locals)?)
            }
            _ => Ok(false),
        }
    }
}

/// Compile bx source to a standard WebAssembly module (current subset only).
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, String> {
    let stmts = crate::parser::parse(source)?;
    let mut checker = Checker::new();
    let mut declarations: Vec<FnDecl> = Vec::new();
    for stmt in &stmts {
        if let Stmt::Fn(name, params, body, _) = stmt {
            declarations.push(FnDecl {
                name: name.clone(),
                params: params.clone(),
                body: body.clone(),
            });
        }
    }
    let mut index: HashMap<String, usize> = HashMap::new();
    for (slot, decl) in declarations.iter().enumerate() {
        let fn_index = IMPORT_COUNT + 1 + slot as u32;
        checker.declare_fn(&decl.name, fn_index)?;
        index.insert(decl.name.clone(), slot);
    }
    let mut literal_nodes: Vec<&Expr> = Vec::new();
    collect_literal_nodes(&stmts, &mut literal_nodes);
    let mut literal_ids: HashMap<usize, u32> = HashMap::new();
    let mut lit_decls: Vec<LitDecl> = Vec::new();
    for (id, node) in literal_nodes.iter().enumerate() {
        literal_ids.insert(*node as *const Expr as usize, id as u32);
        let Expr::Func(params, body, _) = node else {
            continue;
        };
        lit_decls.push(LitDecl {
            params: params.clone(),
            body: body.as_slice(),
        });
    }
    checker.literal_ids = literal_ids.clone();
    let decl_count = declarations.len();
    let lit_count = lit_decls.len();
    // Seed top-level globals so argument inference can see their types. The
    // checker itself types literals/arrays/records correctly; statements whose
    // initialiser cannot be typed yet (calls to not-yet-inferred functions) are
    // skipped here and declared during the full check after inference.
    let mut seeded: HashMap<String, Ty> = HashMap::new();
    for stmt in &stmts {
        if let Stmt::Let(name, _, _) = stmt {
            if checker.check_stmt(stmt).is_ok() {
                if let Ok(ty) = checker.lookup(name) {
                    seeded.insert(name.clone(), ty);
                }
            }
        }
    }
    let (inferred_params, inferred_results) = {
        let globals = checker.global_types();
        let mut infer = InferState {
            index: &index,
            globals: &globals,
            shapes: &checker.shapes,
            literal_ids: &literal_ids,
            decl_count,
            params: declarations
                .iter()
                .map(|decl| vec![None; decl.params.len()])
                .chain(lit_decls.iter().map(|decl| vec![None; decl.params.len()]))
                .collect(),
            results: vec![None; decl_count + lit_count],
        };
        for _ in 0..8 {
            let mut changed = false;
            for stmt in &stmts {
                if let Stmt::Fn(..) = stmt {
                    continue;
                }
                let mut locals = globals.clone();
                changed |= infer.walk(std::slice::from_ref(stmt), &mut locals, None)?;
            }
            for (slot, decl) in declarations.iter().enumerate() {
                let mut locals = globals.clone();
                for (param, ty) in decl.params.iter().zip(infer.params[slot].iter()) {
                    locals.insert(param.clone(), ty.unwrap_or(Ty::Int));
                }
                changed |= infer.walk(&decl.body, &mut locals, Some(slot))?;
            }
            for (id, decl) in lit_decls.iter().enumerate() {
                let slot = decl_count + id;
                let mut locals = globals.clone();
                for (param, ty) in decl.params.iter().zip(infer.params[slot].iter()) {
                    locals.insert(param.clone(), ty.unwrap_or(Ty::Int));
                }
                changed |= infer.walk(decl.body, &mut locals, Some(slot))?;
            }
            if !changed {
                break;
            }
        }
        (infer.params, infer.results)
    };
    for (slot, decl) in declarations.iter().enumerate() {
        let params: Vec<Ty> = inferred_params[slot]
            .iter()
            .map(|ty| ty.unwrap_or(Ty::Int))
            .collect();
        checker.set_fn_sig(&decl.name, params, inferred_results[slot]);
    }
    for id in 0..lit_count {
        let slot = decl_count + id;
        let params: Vec<Ty> = inferred_params[slot]
            .iter()
            .map(|ty| ty.unwrap_or(Ty::Int))
            .collect();
        checker.lit_sigs.push(FnSig {
            params,
            result: inferred_results[slot],
            index: IMPORT_COUNT + 1 + decl_count as u32 + id as u32,
        });
    }
    checker.check_literal_bodies(&lit_decls)?;
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
    let console_i64_ty = builder.add_type(vec![ValType::I64], vec![]);
    let console_f64_ty = builder.add_type(vec![ValType::F64], vec![]);
    let console_utf8_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![]);
    let performance_ty = builder.add_type(vec![], vec![ValType::F64]);
    let gl_create_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let gl_clear_color_ty = builder.add_type(
        vec![ValType::F64, ValType::F64, ValType::F64, ValType::F64],
        vec![],
    );
    let gl_clear_ty = builder.add_type(vec![], vec![]);
    let gl_read_pixel_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let gl_to_png_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let str_compare_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    let int_to_str_ty = builder.add_type(vec![ValType::I64], vec![ValType::I32]);
    let f64_to_str_ty = builder.add_type(vec![ValType::F64], vec![ValType::I32]);
    let str_to_int_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I64]);
    let now_ms_ty = builder.add_type(vec![], vec![ValType::I64]);
    let map_new_ty = builder.add_type(vec![], vec![ValType::I32]);
    let map_set_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32, ValType::I64],
        vec![ValType::I32],
    );
    let map_key_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I64],
    );
    let map_has_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    let map_len_ty = builder.add_type(vec![ValType::I32], vec![ValType::I32]);
    let map_key_at_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let map_value_at_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I64]);
    let bool_to_str_ty = builder.add_type(vec![ValType::I32], vec![ValType::I32]);
    let sleep_ms_ty = builder.add_type(vec![ValType::I64], vec![]);
    let cwd_ty = builder.add_type(vec![], vec![ValType::I32]);
    let env_get_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let mkdir_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let start_ty = builder.add_type(vec![], vec![]);
    let mut fn_types: Vec<u32> = Vec::new();
    for decl in &declarations {
        let sig = checker
            .fn_sig(&decl.name)
            .cloned()
            .ok_or_else(|| format!("missing signature for '{}'", decl.name))?;
        let mut params: Vec<ValType> = Vec::new();
        for ty in &sig.params {
            params.extend(wasm_types(*ty));
        }
        let results = match sig.result {
            Some(ty) => wasm_types(ty),
            None => Vec::new(),
        };
        fn_types.push(builder.add_type(params, results));
    }
    for sig in &checker.lit_sigs {
        let mut params: Vec<ValType> = Vec::new();
        for ty in &sig.params {
            params.extend(wasm_types(*ty));
        }
        let results = match sig.result {
            Some(ty) => wasm_types(ty),
            None => Vec::new(),
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
    builder.import_func("web", "console_log_i64", console_i64_ty);
    builder.import_func("web", "console_log_f64", console_f64_ty);
    builder.import_func("web", "console_log_utf8", console_utf8_ty);
    builder.import_func("web", "performance_now", performance_ty);
    builder.import_func("webgl", "create", gl_create_ty);
    builder.import_func("webgl", "clear_color", gl_clear_color_ty);
    builder.import_func("webgl", "clear", gl_clear_ty);
    builder.import_func("webgl", "read_pixel", gl_read_pixel_ty);
    builder.import_func("webgl", "to_png", gl_to_png_ty);
    builder.import_func("env", "str_compare", str_compare_ty);
    builder.import_func("env", "int_to_str", int_to_str_ty);
    builder.import_func("env", "f64_to_str", f64_to_str_ty);
    builder.import_func("env", "str_to_int", str_to_int_ty);
    builder.import_func("env", "now_ms", now_ms_ty);
    builder.import_func("env", "sleep_ms", sleep_ms_ty);
    builder.import_func("env", "cwd", cwd_ty);
    builder.import_func("env", "env_get", env_get_ty);
    builder.import_func("env", "mkdir", mkdir_ty);
    builder.import_func("env", "file_exists", mkdir_ty);
    builder.import_func("env", "map_new", map_new_ty);
    builder.import_func("env", "map_set", map_set_ty);
    builder.import_func("env", "map_get", map_key_ty);
    builder.import_func("env", "map_has", map_has_ty);
    builder.import_func("env", "map_len", map_len_ty);
    builder.import_func("env", "map_key_at", map_key_at_ty);
    builder.import_func("env", "map_value_at", map_value_at_ty);
    builder.import_func("env", "bool_to_str", bool_to_str_ty);
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
            Ty::Obj(_) | Ty::Map | Ty::Func(_) | Ty::Bool => {
                builder.add_global(ValType::I32, true, WVal::I32(0))
            }
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
            result: None,
            scoped: Vec::new(),
            literal_ids: literal_ids.clone(),
        };
        for stmt in &stmts {
            emitter.emit_stmt(stmt)?;
        }
    }

    for (slot, decl) in declarations.iter().enumerate() {
        let sig = checker
            .fn_sig(&decl.name)
            .cloned()
            .ok_or_else(|| format!("missing signature for '{}'", decl.name))?;
        builder.begin_function(fn_types[slot]);
        let mut temp_base = 0u32;
        for ty in &sig.params {
            temp_base += wasm_types(*ty).len() as u32;
        }
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
                result: sig.result,
                scoped: Vec::new(),
                literal_ids: literal_ids.clone(),
            };
            let mut used = 0u32;
            for (param, ty) in decl.params.iter().zip(sig.params.iter()) {
                emitter.locals.insert(
                    param.clone(),
                    LocalSlot {
                        ty: *ty,
                        slot: used,
                    },
                );
                used += wasm_types(*ty).len() as u32;
            }
            for stmt in &decl.body {
                emitter.emit_stmt(stmt)?;
            }
            match sig.result {
                Some(Ty::Int) => emitter.builder.push(Instr::Const(0x42, WVal::I64(0))),
                Some(Ty::Float) => emitter.builder.push(Instr::Const(0x44, WVal::F64(0.0))),
                Some(Ty::Str) => {
                    emitter.builder.push(Instr::Const(0x41, WVal::I32(0)));
                    emitter.builder.push(Instr::Const(0x41, WVal::I32(0)));
                }
                Some(Ty::Arr | Ty::Obj(_) | Ty::Map | Ty::Func(_) | Ty::Bool) => {
                    emitter.builder.push(Instr::Const(0x41, WVal::I32(0)))
                }
                None => {}
            }
        }
    }
    for (id, decl) in lit_decls.iter().enumerate() {
        let sig = checker
            .lit_sigs
            .get(id)
            .cloned()
            .ok_or_else(|| "missing signature for a function literal".to_string())?;
        builder.begin_function(fn_types[decl_count + id]);
        let mut temp_base = 0u32;
        for ty in &sig.params {
            temp_base += wasm_types(*ty).len() as u32;
        }
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
                result: sig.result,
                scoped: Vec::new(),
                literal_ids: literal_ids.clone(),
            };
            let mut used = 0u32;
            for (param, ty) in decl.params.iter().zip(sig.params.iter()) {
                emitter.locals.insert(
                    param.clone(),
                    LocalSlot {
                        ty: *ty,
                        slot: used,
                    },
                );
                used += wasm_types(*ty).len() as u32;
            }
            for stmt in decl.body {
                emitter.emit_stmt(stmt)?;
            }
            match sig.result {
                Some(Ty::Int) => emitter.builder.push(Instr::Const(0x42, WVal::I64(0))),
                Some(Ty::Float) => emitter.builder.push(Instr::Const(0x44, WVal::F64(0.0))),
                Some(Ty::Str) => {
                    emitter.builder.push(Instr::Const(0x41, WVal::I32(0)));
                    emitter.builder.push(Instr::Const(0x41, WVal::I32(0)));
                }
                Some(Ty::Arr | Ty::Obj(_) | Ty::Map | Ty::Func(_) | Ty::Bool) => {
                    emitter.builder.push(Instr::Const(0x41, WVal::I32(0)))
                }
                None => {}
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
pub fn execute<F, G>(
    bytes: &[u8],
    argv: &[String],
    max_steps: u64,
    timeout_ms: u64,
    on_i64: F,
    on_str: G,
) -> Result<(), String>
where
    F: FnMut(i64),
    G: FnMut(&str),
{
    crate::wasm::run_start(bytes, argv, max_steps, timeout_ms, on_i64, on_str)
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
            0,
            0,
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
            0,
            0,
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
            0,
            0,
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
    fn float_parameters_and_results() {
        let (numbers, strings) = run_with_argv(
            "fn half(x) { return x / 2 }\nfn scale(x, factor) { return x * factor }\nfn show(x) { print(x) }\nprint(half(7))\nprint(scale(3, 1.5))\nshow(2.5)",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec!["3.5".to_string(), "4.5".to_string(), "2.5".to_string()]
        );
    }

    #[test]
    fn bool_values_and_expressions_work() {
        let (numbers, strings) = run_with_argv(
            "let flag = 3 > 2\nif (flag) { print(1) }\nprint(str(flag))\nlet a = true\nlet b = false\nprint(a)\nprint(b)\nprint(a && b)\nprint(a || b)\nprint(!b)\nlet n = flag ? 10 : 20\nprint(n)\nfn even(x) { return x % 2 == 0 }\nprint(even(4))\nprint(even(5))",
            &[],
        );
        assert_eq!(numbers, vec![1, 10]);
        assert_eq!(
            strings,
            vec![
                "true".to_string(),
                "true".to_string(),
                "false".to_string(),
                "false".to_string(),
                "true".to_string(),
                "true".to_string(),
                "true".to_string(),
                "false".to_string(),
            ]
        );
    }

    #[test]
    fn non_capturing_function_literals_work() {
        let (numbers, strings) = run_with_argv(
            "let double = fn(x) { return x * 2 }\nprint(double(21))\nfn apply(f, x) { return f(x) }\nlet inc = fn(x) { return x + 1 }\nprint(apply(inc, 41))\nlet hi = fn(name) { return \"hi \" + name }\nprint(hi(\"there\"))",
            &[],
        );
        assert_eq!(numbers, vec![42, 42]);
        assert_eq!(strings, vec!["hi there".to_string()]);
    }

    #[test]
    fn capturing_function_literals_are_rejected() {
        let error = compile_to_wasm(
            "fn outer() {\n  let n = 5\n  let f = fn(x) { return x + n }\n  return f(1)\n}\nprint(outer())",
        )
        .unwrap_err();
        assert!(
            error.contains("cannot capture outer variables"),
            "unexpected error: {}",
            error
        );
    }

    #[test]
    fn maps_iterate_in_entry_order() {
        let (numbers, strings) = run_with_argv(
            "let m = map_new()\nmap_set(m, \"a\", 1)\nmap_set(m, \"b\", 2)\nfor k, v in m {\n  print(k)\n  print(v)\n}\nfor k in m {\n  print(k)\n}",
            &[],
        );
        assert_eq!(numbers, vec![1, 2]);
        assert_eq!(strings, vec!["a", "b", "a", "b"]);
    }

    #[test]
    fn dynamic_maps_work() {
        let prints = run_source(
            "let m = map_new()\nmap_set(m, \"a\", 1)\nmap_set(m, \"b\", 2)\nmap_set(m, \"a\", 5)\nprint(map_len(m))\nprint(map_get(m, \"a\") + map_get(m, \"b\"))\nprint(map_has(m, \"a\"))\nprint(map_has(m, \"zz\"))\nprint(map_get(m, \"missing\"))",
        );
        assert_eq!(prints, vec![2, 7, 1, 0, 0]);
        assert!(compile_to_wasm("let m = map_new()\nprint(m)").is_err());
    }

    #[test]
    fn process_utilities_work() {
        let dir = std::env::temp_dir().join(format!("bx-wasm-util-{}", std::process::id()));
        let made = dir.join("made");
        let path = made.to_string_lossy().replace('\\', "/");
        let source = format!(
            "let t = now()\nif (t > 0) {{ print(1) }}\nlet c = cwd()\nif (c.len() > 0) {{ print(2) }}\nlet p = env(\"PATH\")\nif (p.len() >= 0) {{ print(3) }}\nsleep(1)\nmkdir(\"{path}\")\nprint(file_exists(\"{path}\"))\nprint(file_exists(\"{path}/missing\"))"
        );
        let (numbers, _strings) = run_with_argv(&source, &[]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(numbers, vec![1, 2, 3, 1, 0]);
    }

    #[test]
    fn number_string_conversions_and_methods() {
        let (numbers, strings) = run_with_argv(
            "print(str(42))\nprint(str(3.5))\nprint(int(\"45\"))\nprint(int(3.9))\nlet a = [1, 2, 3]\na.pop()\nprint(len(a))\nlet s = \"hello\"\nprint(s.len())\nprint(a.len())\nprint(\"n=\" + str(7))",
            &[],
        );
        assert_eq!(numbers, vec![45, 3, 2, 5, 2]);
        assert_eq!(
            strings,
            vec!["42".to_string(), "3.5".to_string(), "n=7".to_string()]
        );
    }

    #[test]
    fn for_in_iterates_arrays() {
        let prints = run_source(
            "let a = [5, 6, 7]\nlet s = 0\nfor v in a { s = s + v }\nprint(s)\nlet b = []\nfor k, v in a { b.push(k * 10 + v) }\nprint(len(b))\nprint(b[2])\nfn total(xs) {\n  let t = 0\n  for v in xs { t = t + v }\n  return t\n}\nprint(total([1, 2, 3]))",
        );
        assert_eq!(prints, vec![18, 3, 27, 6]);
    }

    #[test]
    fn arrays_grow_with_push() {
        let prints = run_source(
            "let a = [1, 2]\na.push(3)\na.push(4)\nprint(len(a))\nprint(a[3])\nfn build() {\n  let xs = []\n  xs.push(7)\n  return xs\n}\nlet b = build()\nprint(b[0])",
        );
        assert_eq!(prints, vec![4, 4, 7]);
    }

    #[test]
    fn string_comparisons_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let a = \"apple\"\nlet b = a + \"s\"\nif (a < b) { print(1) }\nif (a == \"apple\") { print(2) }\nif (b != a) { print(3) }\nif (\"b\" >= \"b\") { print(4) }\nif (b <= a) { print(5) }\nif (\"z\" > \"a\") { print(6) }",
            &[],
        );
        assert!(strings.is_empty());
        assert_eq!(numbers, vec![1, 2, 3, 4, 6]);
    }

    #[test]
    fn mixed_returns_promote_to_float() {
        let (numbers, strings) = run_with_argv(
            "fn f(x) { if (x < 0) { return 0 }\n  return x / 2 }\nprint(f(-1))\nprint(f(7))",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(strings, vec!["0".to_string(), "3.5".to_string()]);
    }

    #[test]
    fn rich_parameters_and_results() {
        let (numbers, strings) = run_with_argv(
            "fn shout(s) { print(s + \"!\") }\nfn tag(s) { return s + \"?\" }\nfn first(xs) { return xs[0] }\nfn make() { return [7, 8] }\nfn kv(o) { return o.k }\nshout(\"hey\")\nprint(tag(\"ok\"))\nlet a = [5, 6]\nprint(first(a))\nlet b = make()\nprint(b[1])\nlet r = {k: 3}\nprint(kv(r))",
            &[],
        );
        assert_eq!(numbers, vec![5, 8, 3]);
        assert_eq!(strings, vec!["hey!".to_string(), "ok?".to_string()]);
    }

    #[test]
    fn calls_inside_blocks_keep_the_caller_frame() {
        let prints = run_source(
            "fn add(a, b) { return a + b }\nlet i = 0\nwhile (i < 3) { i = add(i, 1) }\nprint(i)\nif (i == 3) { print(add(i, 2)) }\nprint(99)",
        );
        assert_eq!(prints, vec![3, 5, 99]);
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
    fn web_console_and_performance_work() {
        let (numbers, strings) = run_with_argv(
            "console_log(42)\nconsole_log(2.5)\nconsole_log(\"hi\")\nlet t = performance_now()\nif (t >= 0.0) { print(1) }",
            &[],
        );
        assert_eq!(numbers, vec![42, 1]);
        assert_eq!(strings, vec!["2.5".to_string(), "hi".to_string()]);
    }

    #[test]
    fn webgl_clear_and_png_export() {
        let path = std::env::temp_dir().join(format!("bx-wasm-gl-{}.png", std::process::id()));
        let escaped = path.to_string_lossy().replace('\\', "\\\\");
        let source = format!(
            "gl_create(2, 2)\ngl_clear_color(1.0, 0.0, 0.0, 1.0)\ngl_clear()\nprint(gl_pixel(0, 0))\nprint(gl_png(\"{escaped}\"))"
        );
        let (numbers, _strings) = run_with_argv(&source, &[]);
        let bytes = std::fs::read(&path).expect("png file");
        let _ = std::fs::remove_file(&path);
        assert_eq!(numbers, vec![0xFF0000FFu32 as i64, 0]);
        assert_eq!(
            &bytes[0..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );
    }

    #[test]
    fn step_budget_stops_infinite_loops() {
        let bytes = compile_to_wasm("let i = 0\nwhile (i < 1) { i = i }").expect("compile");
        let result = execute(&bytes, &[], 5_000, 0, |_value| {}, |_text| {});
        let error = result.expect_err("infinite loop must hit the budget");
        assert!(error.contains("step budget"), "got {error}");
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
        assert!(compile_to_wasm("let f = fn() { return 1 }\nprint(f)").is_err());
        assert!(compile_to_wasm("print(unknown)").is_err());
        assert!(compile_to_wasm("let a = [1, \"x\"]").is_err());
        assert!(compile_to_wasm("let o = {}\nprint(o)").is_err());
    }
}
