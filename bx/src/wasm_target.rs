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
    /// Array of strings: `[len][ptr,len]*N`.
    ArrStr,
    /// Array of records of one fixed shape; entries hold record pointers.
    ArrObj(u32),
    /// Array of int arrays; entries hold inner-array pointers.
    ArrArr,
    /// Array of floats (mixed numeric literals promote to this).
    ArrF,
    /// Array of string arrays (text rows); entries hold inner-array pointers.
    ArrStrArr,
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
        Ty::ArrStr => "array",
        Ty::ArrObj(_) => "array",
        Ty::ArrArr => "array",
        Ty::ArrF => "array",
        Ty::ArrStrArr => "array",
        Ty::Obj(_) => "object",
        Ty::Map => "map",
        Ty::Func(_) => "function",
        Ty::Bool => "bool",
    }
}

/// Element type of a supported array, or `None` for non-arrays.
fn element_ty(ty: Ty) -> Option<Ty> {
    match ty {
        Ty::Arr => Some(Ty::Int),
        Ty::ArrStr => Some(Ty::Str),
        Ty::ArrObj(shape) => Some(Ty::Obj(shape)),
        Ty::ArrArr => Some(Ty::Arr),
        Ty::ArrF => Some(Ty::Float),
        Ty::ArrStrArr => Some(Ty::ArrStr),
        _ => None,
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
        Ty::Arr
        | Ty::ArrStr
        | Ty::ArrObj(_)
        | Ty::ArrArr
        | Ty::ArrF
        | Ty::ArrStrArr
        | Ty::Obj(_)
        | Ty::Map
        | Ty::Func(_)
        | Ty::Bool => {
            vec![ValType::I32]
        }
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
    /// Per-field storage kind: 0 = int, 1 = string (`ptr, len` in one slot),
    /// 2 = float, 3 = bool. Every field still occupies one 8-byte slot.
    kinds: Vec<u8>,
}

/// Storage kind of a record field type.
fn field_kind(ty: Ty) -> Option<u8> {
    match ty {
        Ty::Int => Some(0),
        Ty::Str => Some(1),
        Ty::Float => Some(2),
        Ty::Bool => Some(3),
        _ => None,
    }
}

/// Keys and field kinds of the record `exec()`/`sh()` return.
fn exec_result_keys() -> (Vec<String>, Vec<u8>) {
    (
        vec![
            "code".to_string(),
            "err".to_string(),
            "ms".to_string(),
            "ok".to_string(),
            "out".to_string(),
            "timeout".to_string(),
        ],
        vec![0, 1, 0, 3, 1, 3],
    )
}

fn kind_ty(kind: u8) -> Ty {
    match kind {
        1 => Ty::Str,
        2 => Ty::Float,
        3 => Ty::Bool,
        _ => Ty::Int,
    }
}

#[derive(Clone, Copy)]
struct AttemptCtx {
    /// Record being filled: error (str at 0/4), ok (bool at 8), value at 16.
    record: u32,
    result_ty: Ty,
}

#[derive(Clone, Copy)]
enum SortMode {
    Int,
    Str,
    Float,
    Ptr,
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

    fn shape_id(&self, keys: &[String], kinds: &[u8]) -> u32 {
        let mut shapes = self.shapes.borrow_mut();
        if let Some(index) = shapes
            .iter()
            .position(|shape| shape.keys == keys && shape.kinds == kinds)
        {
            return index as u32;
        }
        shapes.push(Shape {
            keys: keys.to_vec(),
            kinds: kinds.to_vec(),
        });
        (shapes.len() - 1) as u32
    }

    fn shape_kinds(&self, shape: u32) -> Result<Vec<u8>, String> {
        let shapes = self.shapes.borrow();
        shapes
            .get(shape as usize)
            .map(|shape| shape.kinds.clone())
            .ok_or_else(|| "unknown object shape".to_string())
    }

    fn field_ty(&self, shape: u32, key: &str) -> Result<Ty, String> {
        let shapes = self.shapes.borrow();
        let shape = shapes
            .get(shape as usize)
            .ok_or_else(|| "unknown object shape".to_string())?;
        let index = shape
            .keys
            .iter()
            .position(|known| known == key)
            .ok_or_else(|| format!("object has no field '{}'", key))?;
        Ok(kind_ty(shape.kinds[index]))
    }

    fn shape_keys(&self, shape: u32) -> Result<Vec<String>, String> {
        let shapes = self.shapes.borrow();
        shapes
            .get(shape as usize)
            .map(|shape| shape.keys.clone())
            .ok_or_else(|| "unknown object shape".to_string())
    }

    fn exec_result_shape(&self) -> u32 {
        let (keys, kinds) = exec_result_keys();
        self.shape_id(&keys, &kinds)
    }

    /// Shape of the `attempt()` record for a callback returning `result_ty`.
    fn attempt_shape(&self, result_ty: Ty) -> Result<u32, String> {
        let kind = field_kind(result_ty).ok_or_else(|| {
            "wasm target attempt() callbacks return a number, string or bool".to_string()
        })?;
        Ok(self.shape_id(
            &["error".to_string(), "ok".to_string(), "value".to_string()],
            &[1, 3, kind],
        ))
    }

    fn shape_union(&self, left: u32, right: u32) -> Result<u32, String> {
        let left_keys = self.shape_keys(left)?;
        let right_keys = self.shape_keys(right)?;
        let mut entries: Vec<(String, u8)> =
            left_keys.into_iter().zip(self.shape_kinds(left)?).collect();
        for (key, kind) in right_keys.into_iter().zip(self.shape_kinds(right)?) {
            if !entries.iter().any(|(known, _)| *known == key) {
                entries.push((key, kind));
            }
        }
        let keys: Vec<String> = entries.iter().map(|(key, _)| key.clone()).collect();
        let kinds: Vec<u8> = entries.iter().map(|(_, kind)| *kind).collect();
        let mut shapes = self.shapes.borrow_mut();
        if let Some(index) = shapes
            .iter()
            .position(|shape| shape.keys == keys && shape.kinds == kinds)
        {
            return Ok(index as u32);
        }
        shapes.push(Shape { keys, kinds });
        Ok((shapes.len() - 1) as u32)
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
                match ty {
                    Ty::Int => Ok(Ty::Int),
                    Ty::Float => Ok(Ty::Float),
                    other => Err(format!(
                        "wasm target negates numbers only, got {}",
                        ty_name(other)
                    )),
                }
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
                if op == &BinOp::Add && (lt == Ty::Str || rt == Ty::Str) {
                    let other = if lt == Ty::Str { rt } else { lt };
                    if matches!(other, Ty::Str | Ty::Int | Ty::Float | Ty::Bool) {
                        return Ok(Ty::Str);
                    }
                }
                if op == &BinOp::Add && lt == Ty::Str && rt == Ty::Str {
                    return Ok(Ty::Str);
                }
                let structural = lt == rt
                    && matches!(
                        lt,
                        Ty::Obj(_)
                            | Ty::Arr
                            | Ty::ArrF
                            | Ty::ArrStr
                            | Ty::ArrObj(_)
                            | Ty::ArrArr
                            | Ty::ArrStrArr
                    )
                    && matches!(op, BinOp::Eq | BinOp::Ne);
                let comparable = structural
                    || (is_number(lt) && is_number(rt))
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
                if op == &BinOp::Add && (lt == Ty::Str || rt == Ty::Str) {
                    let other = if lt == Ty::Str { rt } else { lt };
                    if matches!(other, Ty::Str | Ty::Int | Ty::Float | Ty::Bool) {
                        return Ok(Ty::Str);
                    }
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
                let mut keys: Vec<String> = Vec::new();
                let mut kinds: Vec<u8> = Vec::new();
                for (key, value) in fields {
                    if keys.iter().any(|seen| seen == key) {
                        return Err(format!("duplicate object key '{}'", key));
                    }
                    let ty = self.infer(value)?;
                    let kind = field_kind(ty).ok_or_else(|| {
                        format!(
                            "wasm target record fields hold int, string, float or bool values, got {}",
                            ty_name(ty)
                        )
                    })?;
                    keys.push(key.clone());
                    kinds.push(kind);
                }
                Ok(Ty::Obj(self.shape_id(&keys, &kinds)))
            }
            Expr::MethodCall(object, key, args, _) => {
                let str_method = match key.as_str() {
                    "trim" | "upper" | "lower" => Some((0usize, Ty::Str)),
                    "repeat" => Some((1usize, Ty::Str)),
                    "index_of" => Some((1usize, Ty::Int)),
                    "split" => Some((1usize, Ty::ArrStr)),
                    "slice" => Some((2usize, Ty::Str)),
                    "replace" => Some((2usize, Ty::Str)),
                    _ => None,
                };
                if let Some((argc, result)) = str_method {
                    let base = self.infer(object)?;
                    if base != Ty::Str && !(key == "index_of" && base == Ty::Arr) {
                        return Err(format!("wasm target .{} works on strings only", key));
                    }
                    if base == Ty::Str && args.len() != argc {
                        return Err(format!("wasm target .{} takes {} value(s)", key, argc));
                    }
                    if base == Ty::Str {
                        for arg in args {
                            let expected = match key.as_str() {
                                "repeat" | "slice" => Ty::Int,
                                _ => Ty::Str,
                            };
                            if self.infer(arg)? != expected {
                                return Err(format!(
                                    "wasm target .{} got the wrong argument type",
                                    key
                                ));
                            }
                        }
                        return Ok(result);
                    }
                }
                if matches!(key.as_str(), "contains" | "starts_with" | "ends_with") {
                    if args.len() != 1 {
                        return Err(format!("wasm target .{} takes one value", key));
                    }
                    let base = self.infer(object)?;
                    match base {
                        Ty::Str => {
                            if self.infer(&args[0])? != Ty::Str {
                                return Err(format!("wasm target .{} works on strings only", key));
                            }
                        }
                        Ty::Arr => {
                            if key != "contains" || self.infer(&args[0])? != Ty::Int {
                                return Err("wasm target int arrays support .contains(int) only"
                                    .to_string());
                            }
                        }
                        Ty::ArrF => {
                            if key != "contains" || !is_number(self.infer(&args[0])?) {
                                return Err(
                                    "wasm target float arrays support .contains(number) only"
                                        .to_string(),
                                );
                            }
                        }
                        Ty::ArrStr => {
                            if key != "contains" || self.infer(&args[0])? != Ty::Str {
                                return Err(
                                    "wasm target string arrays support .contains(str) only"
                                        .to_string(),
                                );
                            }
                        }
                        _ => {
                            return Err(format!(
                                "wasm target .{} works on strings and arrays only",
                                key
                            ))
                        }
                    }
                    return Ok(Ty::Bool);
                }
                if key == "index_of" && args.len() == 1 {
                    let base = self.infer(object)?;
                    let element = match base {
                        Ty::Str => Ty::Str,
                        Ty::Arr => Ty::Int,
                        Ty::ArrF => Ty::Float,
                        Ty::ArrStr => Ty::Str,
                        _ => {
                            return Err("wasm target .index_of() works on strings and arrays only"
                                .to_string())
                        }
                    };
                    if self.infer(&args[0])? != element {
                        return Err("wasm target .index_of() value has the wrong type".to_string());
                    }
                    return Ok(Ty::Int);
                }
                if key == "len" && args.is_empty() {
                    let ty = self.infer(object)?;
                    if matches!(
                        ty,
                        Ty::Str | Ty::Arr | Ty::ArrStr | Ty::ArrObj(_) | Ty::ArrArr | Ty::Map
                    ) {
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
                        let index = self.field_index(shape, key)?;
                        let kinds = self.shape_kinds(shape)?;
                        Ok(kind_ty(kinds[index]))
                    }
                    other => Err(format!(
                        "wasm target can read fields of objects only, got {}",
                        ty_name(other)
                    )),
                }
            }
            Expr::Array(items) => {
                let mut saw_int = false;
                let mut saw_float = false;
                let mut saw_str = false;
                let mut saw_arr = false;
                let mut saw_strarr = false;
                let mut saw_obj: Option<u32> = None;
                for item in items {
                    match self.infer(item)? {
                        Ty::Int => saw_int = true,
                        Ty::Float => saw_float = true,
                        Ty::Str => saw_str = true,
                        Ty::Arr => saw_arr = true,
                        Ty::ArrStr => saw_strarr = true,
                        Ty::Obj(shape) => match saw_obj {
                            Some(known) if known != shape => {
                                return Err("wasm target records in one array need the same shape"
                                    .to_string())
                            }
                            _ => saw_obj = Some(shape),
                        },
                        other => {
                            return Err(format!(
                                "wasm target supports arrays of ints, strings or records, got {}",
                                ty_name(other)
                            ))
                        }
                    }
                }
                // `[["a"], []]`: an empty array literal is layout-compatible with
                // any row kind, so it may join a string-row array.
                let empty_rows_only = items.iter().all(|item| match item {
                    Expr::Array(inner) => {
                        inner.is_empty() || matches!(self.infer(item), Ok(Ty::ArrStr))
                    }
                    _ => matches!(self.infer(item), Ok(Ty::ArrStr)),
                });
                let kinds = if saw_strarr && empty_rows_only {
                    1
                } else {
                    [saw_str, saw_arr, saw_strarr, saw_obj.is_some()]
                        .iter()
                        .filter(|seen| **seen)
                        .count()
                };
                if kinds > 1 || (kinds > 0 && (saw_int || saw_float)) {
                    return Err(
                        "wasm target arrays need one element type (number, string or record)"
                            .to_string(),
                    );
                }
                Ok(if saw_strarr {
                    Ty::ArrStrArr
                } else {
                    match saw_obj {
                        Some(shape) => Ty::ArrObj(shape),
                        None if saw_str => Ty::ArrStr,
                        None if saw_arr => Ty::ArrArr,
                        None if saw_float => Ty::ArrF,
                        None => Ty::Arr,
                    }
                })
            }
            Expr::Index(array, index, _) => {
                let arr_ty = self.infer(array)?;
                if !matches!(
                    arr_ty,
                    Ty::Arr
                        | Ty::ArrStr
                        | Ty::ArrObj(_)
                        | Ty::ArrArr
                        | Ty::ArrF
                        | Ty::ArrStrArr
                        | Ty::Str
                ) {
                    return Err(format!(
                        "wasm target indexes arrays and strings only, got {}",
                        ty_name(arr_ty)
                    ));
                }
                let idx_ty = self.infer(index)?;
                if !is_number(idx_ty) {
                    return Err("wasm target needs a numeric index".to_string());
                }
                Ok(match arr_ty {
                    Ty::Str | Ty::ArrStr => Ty::Str,
                    Ty::ArrObj(shape) => Ty::Obj(shape),
                    Ty::ArrArr => Ty::Arr,
                    Ty::ArrStrArr => Ty::ArrStr,
                    Ty::ArrF => Ty::Float,
                    _ => Ty::Int,
                })
            }

            Expr::Call(callee, args, _) => {
                if let Expr::Ident(name) = &**callee {
                    if name == "exit" && args.len() == 1 {
                        if !is_number(self.infer(&args[0])?) {
                            return Err("wasm target exit() needs a numeric code".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "error" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target error() needs a string".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "assert" && (args.len() == 1 || args.len() == 2) {
                        if self.infer(&args[0])? != Ty::Bool {
                            return Err("wasm target assert() needs a bool condition".to_string());
                        }
                        if let Some(message) = args.get(1) {
                            if self.infer(message)? != Ty::Str {
                                return Err(
                                    "wasm target assert() needs a string message".to_string()
                                );
                            }
                        }
                        return Ok(Ty::Int);
                    }
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
                    if name == "read" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target read(path) needs a string".to_string());
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "write" && args.len() == 2 {
                        if self.infer(&args[0])? != Ty::Str || self.infer(&args[1])? != Ty::Str {
                            return Err("wasm target write(path, data) needs strings".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "which" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target which(name) needs a string".to_string());
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
                    if name == "exists" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target exists(path) needs a string".to_string());
                        }
                        return Ok(Ty::Bool);
                    }
                    if name == "file_exists" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target file_exists(path) needs a string".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "float" && args.len() == 1 {
                        if !matches!(self.infer(&args[0])?, Ty::Int | Ty::Float | Ty::Str) {
                            return Err(
                                "wasm target float() takes a number or a string".to_string()
                            );
                        }
                        return Ok(Ty::Float);
                    }
                    if name == "abs" && args.len() == 1 {
                        return match self.infer(&args[0])? {
                            Ty::Int => Ok(Ty::Int),
                            Ty::Float => Ok(Ty::Float),
                            other => Err(format!(
                                "wasm target abs() takes a number, got {}",
                                ty_name(other)
                            )),
                        };
                    }
                    if matches!(name.as_str(), "floor" | "ceil" | "round") && args.len() == 1 {
                        if !is_number(self.infer(&args[0])?) {
                            return Err(format!("wasm target {}() takes a number", name));
                        }
                        return Ok(Ty::Float);
                    }
                    if name == "sqrt" && args.len() == 1 {
                        if !is_number(self.infer(&args[0])?) {
                            return Err("wasm target sqrt() takes a number".to_string());
                        }
                        return Ok(Ty::Float);
                    }
                    if matches!(name.as_str(), "trim" | "upper" | "lower") && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err(format!("wasm target {}() takes a string", name));
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "replace" && args.len() == 3 {
                        for arg in args {
                            if self.infer(arg)? != Ty::Str {
                                return Err("wasm target replace() takes three strings".to_string());
                            }
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "index_of" && args.len() == 2 {
                        let base = self.infer(&args[0])?;
                        let element = match base {
                            Ty::Str => Ty::Str,
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            _ => {
                                return Err(
                                    "wasm target index_of() takes a string or array".to_string()
                                )
                            }
                        };
                        if element == Ty::Float && self.infer(&args[1])? == Ty::Int {
                            return Ok(Ty::Int);
                        }
                        let pushed = self.infer(&args[1])?;
                        if pushed != element && !(element == Ty::Float && pushed == Ty::Int) {
                            return Err(format!(
                                "wasm target index_of() takes a {} value",
                                ty_name(element)
                            ));
                        }
                        return Ok(Ty::Int);
                    }
                    if matches!(name.as_str(), "min" | "max") && args.len() == 1 {
                        return match self.infer(&args[0])? {
                            Ty::Arr => Ok(Ty::Int),
                            Ty::ArrF => Ok(Ty::Float),
                            Ty::ArrStr => Ok(Ty::Str),
                            _ => Err("wasm target min()/max() takes a numeric or string array"
                                .to_string()),
                        };
                    }
                    if name == "repeat" && args.len() == 2 {
                        if self.infer(&args[0])? != Ty::Str || self.infer(&args[1])? != Ty::Int {
                            return Err(
                                "wasm target repeat() takes a string and an int".to_string()
                            );
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "type" && args.len() == 1 {
                        self.infer(&args[0])?;
                        return Ok(Ty::Str);
                    }
                    if name == "split" && args.len() == 2 {
                        if self.infer(&args[0])? != Ty::Str || self.infer(&args[1])? != Ty::Str {
                            return Err("wasm target split() takes two strings".to_string());
                        }
                        return Ok(Ty::ArrStr);
                    }
                    if name == "join" && (args.len() == 1 || args.len() == 2) {
                        let base = self.infer(&args[0])?;
                        if !matches!(base, Ty::Arr | Ty::ArrF | Ty::ArrStr) {
                            return Err(
                                "wasm target join() takes a numeric or string array".to_string()
                            );
                        }
                        if args.len() == 2 && self.infer(&args[1])? != Ty::Str {
                            return Err("wasm target join() separator is a string".to_string());
                        }
                        return Ok(Ty::Str);
                    }
                    if name == "slice" && (args.len() == 2 || args.len() == 3) {
                        let base = self.infer(&args[0])?;
                        for arg in &args[1..] {
                            if !is_number(self.infer(arg)?) {
                                return Err("wasm target slice() needs numeric bounds".to_string());
                            }
                        }
                        return match base {
                            Ty::Str => Ok(Ty::Str),
                            Ty::Arr
                            | Ty::ArrF
                            | Ty::ArrStr
                            | Ty::ArrObj(_)
                            | Ty::ArrArr
                            | Ty::ArrStrArr => Ok(base),
                            other => Err(format!("wasm target cannot slice {}", ty_name(other))),
                        };
                    }
                    if name == "range" && (args.len() == 1 || args.len() == 2 || args.len() == 3) {
                        for arg in args {
                            if self.infer(arg)? != Ty::Int {
                                return Err("wasm target range() takes ints".to_string());
                            }
                        }
                        return Ok(Ty::Arr);
                    }
                    if name == "sum" && args.len() == 1 {
                        return match self.infer(&args[0])? {
                            Ty::Arr => Ok(Ty::Int),
                            Ty::ArrF => Ok(Ty::Float),
                            _ => Err("wasm target sum() takes a numeric array".to_string()),
                        };
                    }
                    if matches!(name.as_str(), "reverse" | "sort") && args.len() == 1 {
                        let input = self.infer(&args[0])?;
                        let supported = matches!(input, Ty::Arr | Ty::ArrF | Ty::ArrStr)
                            || (name == "reverse"
                                && matches!(input, Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrStrArr));
                        if !supported {
                            return Err(format!(
                                "wasm target {}() takes a numeric or string array, got {}",
                                name,
                                ty_name(input)
                            ));
                        }
                        return Ok(input);
                    }
                    if matches!(name.as_str(), "map" | "filter" | "each") && args.len() == 2 {
                        let input = self.infer(&args[0])?;
                        let elem = match input {
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            other => {
                                return Err(format!(
                                    "wasm target {}() takes an array, got {}",
                                    name,
                                    ty_name(other)
                                ))
                            }
                        };
                        let Ty::Func(id) = self.infer(&args[1])? else {
                            return Err(format!(
                                "wasm target {}() needs a function literal callback",
                                name
                            ));
                        };
                        let sig = self
                            .lit_sigs
                            .get(id as usize)
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?
                            .clone();
                        if sig.params != vec![elem] {
                            return Err(format!(
                                "wasm target {}() callbacks take one {}",
                                name,
                                ty_name(elem)
                            ));
                        }
                        return match name.as_str() {
                            "map" => match sig.result {
                                Some(Ty::Int) => Ok(Ty::Arr),
                                Some(Ty::Float) => Ok(Ty::ArrF),
                                Some(Ty::Str) => Ok(Ty::ArrStr),
                                Some(Ty::Obj(shape)) => Ok(Ty::ArrObj(shape)),
                                Some(Ty::Arr) => Ok(Ty::ArrArr),
                                Some(Ty::ArrStr) => Ok(Ty::ArrStrArr),
                                _ => Err(
                                    "wasm target map() callbacks return numbers, strings, records or arrays"
                                        .to_string(),
                                ),
                            },
                            "filter" => {
                                if sig.result != Some(Ty::Bool) {
                                    return Err(
                                        "wasm target filter() callbacks return bools".to_string()
                                    );
                                }
                                Ok(input)
                            }
                            _ => Err("wasm target each() is statement-only".to_string()),
                        };
                    }
                    if name == "reduce" && (args.len() == 2 || args.len() == 3) {
                        let elem = match self.infer(&args[0])? {
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            other => {
                                return Err(format!(
                                    "wasm target reduce() takes an array, got {}",
                                    ty_name(other)
                                ))
                            }
                        };
                        let Ty::Func(id) = self.infer(&args[1])? else {
                            return Err("wasm target reduce() needs a function literal callback"
                                .to_string());
                        };
                        let sig = self
                            .lit_sigs
                            .get(id as usize)
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?
                            .clone();
                        let acc = if args.len() == 3 {
                            self.infer(&args[2])?
                        } else {
                            elem
                        };
                        if !matches!(acc, Ty::Int | Ty::Float | Ty::Str) {
                            return Err(
                                "wasm target reduce() accumulator must be a number or string"
                                    .to_string(),
                            );
                        }
                        if sig.params != vec![acc, elem] || sig.result != Some(acc) {
                            return Err(format!(
                                "wasm target reduce() callbacks take ({}, {}) and return {}",
                                ty_name(acc),
                                ty_name(elem),
                                ty_name(acc)
                            ));
                        }
                        return Ok(acc);
                    }
                    if name == "keys" && args.len() == 1 {
                        if matches!(self.infer(&args[0])?, Ty::Obj(_) | Ty::Map) {
                            return Ok(Ty::ArrStr);
                        }
                        return Err("wasm target keys() takes a record or map".to_string());
                    }
                    if name == "values" && args.len() == 1 {
                        if matches!(self.infer(&args[0])?, Ty::Obj(_) | Ty::Map) {
                            return Ok(Ty::Arr);
                        }
                        return Err("wasm target values() takes a record or map".to_string());
                    }
                    if name == "has" && args.len() == 2 {
                        if !matches!(self.infer(&args[0])?, Ty::Obj(_) | Ty::Map)
                            || self.infer(&args[1])? != Ty::Str
                        {
                            return Err(
                                "wasm target has() takes a record/map and a string".to_string()
                            );
                        }
                        return Ok(Ty::Bool);
                    }
                    if name == "get" && (args.len() == 2 || args.len() == 3) {
                        let base = self.infer(&args[0])?;
                        if self.infer(&args[1])? != Ty::Str {
                            return Err("wasm target get() takes a string key".to_string());
                        }
                        if let Ty::Obj(shape) = base {
                            let Expr::Str(key) = &args[1] else {
                                let kinds = self.shape_kinds(shape)?;
                                if kinds.iter().any(|kind| *kind != kinds[0]) {
                                    return Err(
                                        "wasm target dynamic get() needs a record whose fields share one type"
                                            .to_string(),
                                    );
                                }
                                let field_ty = kind_ty(kinds[0]);
                                if let Some(default) = args.get(2) {
                                    let default_ty = self.infer(default)?;
                                    if default_ty != field_ty
                                        && !(field_ty == Ty::Float && default_ty == Ty::Int)
                                    {
                                        return Err("wasm target get() default has the wrong type"
                                            .to_string());
                                    }
                                }
                                return Ok(field_ty);
                            };
                            let keys = self.shape_keys(shape)?;
                            let Some(position) = keys.iter().position(|known| known == key) else {
                                match args.get(2) {
                                    Some(default) => return self.infer(default),
                                    None => return Ok(Ty::Int),
                                }
                            };
                            let kinds = self.shape_kinds(shape)?;
                            let field_ty = kind_ty(kinds[position]);
                            if let Some(default) = args.get(2) {
                                let default_ty = self.infer(default)?;
                                if default_ty != field_ty
                                    && !(field_ty == Ty::Float && default_ty == Ty::Int)
                                {
                                    return Err(
                                        "wasm target get() default has the wrong type".to_string()
                                    );
                                }
                            }
                            return Ok(field_ty);
                        }
                        if base != Ty::Map {
                            return Err("wasm target get() takes a record/map".to_string());
                        }
                        if args.len() == 3 && self.infer(&args[2])? != Ty::Int {
                            return Err("wasm target get() default must be an int".to_string());
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "merge" && args.len() == 2 {
                        let left = self.infer(&args[0])?;
                        let right = self.infer(&args[1])?;
                        let (Ty::Obj(left_shape), Ty::Obj(right_shape)) = (left, right) else {
                            return Err("wasm target merge() takes two records".to_string());
                        };
                        return Ok(Ty::Obj(self.shape_union(left_shape, right_shape)?));
                    }
                    if name == "sort" && args.len() == 2 {
                        let input = self.infer(&args[0])?;
                        let element = match input {
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            _ => {
                                return Err(
                                    "wasm target comparator sort() takes a numeric, string or record array"
                                        .to_string(),
                                )
                            }
                        };
                        let Ty::Func(id) = self.infer(&args[1])? else {
                            return Err("wasm target sort() needs a function literal comparator"
                                .to_string());
                        };
                        let sig = self
                            .lit_sigs
                            .get(id as usize)
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        if sig.params != vec![element, element] || sig.result != Some(Ty::Int) {
                            return Err(format!(
                                "wasm target sort() comparators take two {} values and return an int",
                                ty_name(element)
                            ));
                        }
                        return Ok(input);
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
                        if !is_number(ty) && ty != Ty::Str && ty != Ty::Bool {
                            return Err(
                                "wasm target int() takes a number, string or bool".to_string()
                            );
                        }
                        return Ok(Ty::Int);
                    }
                    if name == "attempt" && args.len() == 1 {
                        let Ty::Func(id) = self.infer(&args[0])? else {
                            return Err(
                                "wasm target attempt() needs a function literal".to_string()
                            );
                        };
                        let sig = self
                            .lit_sigs
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        if !sig.params.is_empty() {
                            return Err(
                                "wasm target attempt() callbacks take no parameters".to_string()
                            );
                        }
                        let result_ty = sig.result.unwrap_or(Ty::Int);
                        return Ok(Ty::Obj(self.attempt_shape(result_ty)?));
                    }
                    if name == "exec" && (args.len() == 1 || args.len() == 2) {
                        match self.infer(&args[0])? {
                            Ty::Str | Ty::ArrStr => {}
                            other => {
                                return Err(format!(
                                    "wasm target exec() takes a string or a string array, got {}",
                                    ty_name(other)
                                ))
                            }
                        }
                        if let Some(opts) = args.get(1) {
                            let Ty::Obj(shape) = self.infer(opts)? else {
                                return Err(
                                    "wasm target exec() options need an object literal".to_string()
                                );
                            };
                            let keys = self.shape_keys(shape)?;
                            let kinds = self.shape_kinds(shape)?;
                            for (key, kind) in keys.iter().zip(kinds.iter()) {
                                let ok = match key.as_str() {
                                    "cwd" | "stdin" => *kind == 1,
                                    "timeout" => *kind == 0,
                                    "shell" => *kind == 3,
                                    _ => {
                                        return Err(format!(
                                            "wasm target exec() does not support the '{}' option",
                                            key
                                        ))
                                    }
                                };
                                if !ok {
                                    return Err(format!(
                                        "wasm target exec() option '{}' has the wrong type",
                                        key
                                    ));
                                }
                            }
                        }
                        return Ok(Ty::Obj(self.exec_result_shape()));
                    }
                    if name == "sh" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target sh(command) needs a string".to_string());
                        }
                        return Ok(Ty::Obj(self.exec_result_shape()));
                    }
                    if name == "push" && args.len() == 2 {
                        if !matches!(args[0], Expr::Ident(_)) {
                            return Err(
                                "wasm target pushes into a named array variable only".to_string()
                            );
                        }
                        let arr_ty = self.infer(&args[0])?;
                        let element = element_ty(arr_ty).ok_or_else(|| {
                            format!(
                                "wasm target pushes into arrays only, got {}",
                                ty_name(arr_ty)
                            )
                        })?;
                        let pushed = self.infer(&args[1])?;
                        if pushed != element && !(element == Ty::Float && pushed == Ty::Int) {
                            return Err(format!(
                                "wasm target array elements hold {}",
                                ty_name(element)
                            ));
                        }
                        return Ok(arr_ty);
                    }
                    if name == "pop" && args.len() == 1 {
                        let arr_ty = self.infer(&args[0])?;
                        return element_ty(arr_ty).ok_or_else(|| {
                            format!("wasm target pops arrays only, got {}", ty_name(arr_ty))
                        });
                    }
                    if name == "contains" && args.len() == 2 {
                        let arr_ty = self.infer(&args[0])?;
                        match arr_ty {
                            Ty::Arr => {
                                self.infer(&args[1])?;
                            }
                            Ty::ArrStr => {
                                if self.infer(&args[1])? != Ty::Str {
                                    return Err(
                                        "wasm target contains() on string arrays takes a string"
                                            .to_string(),
                                    );
                                }
                            }
                            Ty::ArrF => {
                                if !is_number(self.infer(&args[1])?) {
                                    return Err(
                                        "wasm target contains() on float arrays takes a number"
                                            .to_string(),
                                    );
                                }
                            }
                            Ty::ArrObj(shape) => {
                                if self.infer(&args[1])? != Ty::Obj(shape) {
                                    return Err(
                                        "wasm target contains() on record arrays takes a record"
                                            .to_string(),
                                    );
                                }
                            }
                            Ty::ArrArr => {
                                if self.infer(&args[1])? != Ty::Arr {
                                    return Err(
                                        "wasm target contains() on nested arrays takes an array"
                                            .to_string(),
                                    );
                                }
                            }
                            Ty::ArrStrArr => {
                                if self.infer(&args[1])? != Ty::ArrStr {
                                    return Err(
                                        "wasm target contains() on text rows takes a string array"
                                            .to_string(),
                                    );
                                }
                            }

                            other => {
                                return Err(format!(
                                    "wasm target contains() supports int, string and float arrays, got {}",
                                    ty_name(other)
                                ))
                            }
                        }
                        return Ok(Ty::Bool);
                    }
                    if name == "mkdir" && args.len() == 1 {
                        if self.infer(&args[0])? != Ty::Str {
                            return Err("wasm target mkdir(path) takes a string".to_string());
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
                            Ty::Arr
                            | Ty::ArrStr
                            | Ty::ArrObj(_)
                            | Ty::ArrArr
                            | Ty::ArrF
                            | Ty::ArrStrArr
                            | Ty::Str
                            | Ty::Map => Ok(Ty::Int),
                            other => {
                                Err(format!("wasm target cannot take len of {}", ty_name(other)))
                            }
                        };
                    }
                }
                if let Ok(Ty::Func(id)) = self.infer(callee) {
                    let sig = self
                        .lit_sigs
                        .get(id as usize)
                        .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                    return sig.result.ok_or_else(|| {
                        "wasm target function literal returns no value".to_string()
                    });
                }
                match &**callee {
                    Expr::Ident(name) => Err(format!(
                        "wasm target does not support {}() with {} argument(s)",
                        name,
                        args.len()
                    )),
                    _ => Err(format!(
                        "wasm target does not support this call yet: {}",
                        expr_kind(expr)
                    )),
                }
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

    fn check_literal_bodies(
        &mut self,
        decls: &[LitDecl],
        captures: &[Vec<(String, Ty)>],
    ) -> Result<(), String> {
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
            // Captured outer bindings become ordinary locals in the literal body.
            if let Some(list) = captures.get(id) {
                for (name, ty) in list {
                    self.local_scope.insert(name.clone(), *ty);
                }
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
                match &**target {
                    Expr::Ident(name) => {
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
                    Expr::Member(object, key, _) => {
                        let Ty::Obj(shape) = self.infer(object)? else {
                            return Err(
                                "wasm target writes record fields only".to_string()
                            );
                        };
                        let expected = self.field_ty(shape, key)?;
                        let actual = self.infer(value)?;
                        let ok = expected == actual
                            || (expected == Ty::Float && actual == Ty::Int);
                        if !ok {
                            return Err(format!(
                                "wasm target record field '{}' holds {}, assigned {}",
                                key,
                                ty_name(expected),
                                ty_name(actual)
                            ));
                        }
                    }
                    Expr::Index(array, index, _) => {
                        let arr_ty = self.infer(array)?;
                        if !matches!(
                            arr_ty,
                            Ty::Arr
                                | Ty::ArrStr
                                | Ty::ArrObj(_)
                                | Ty::ArrArr
                                | Ty::ArrF
                                | Ty::ArrStrArr
                        ) {
                            return Err(
                                "wasm target writes array elements only".to_string()
                            );
                        }
                        if !is_number(self.infer(index)?) {
                            return Err("wasm target needs a numeric index".to_string());
                        }
                        let expected = match arr_ty {
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            Ty::ArrF => Ty::Float,
                            _ => Ty::Int,
                        };
                        let actual = self.infer(value)?;
                        let fits = actual == expected
                            || (expected == Ty::Float && actual == Ty::Int);
                        if !fits {
                            return Err(format!(
                                "wasm target array elements hold {}",
                                ty_name(expected)
                            ));
                        }
                    }
                    _ => {
                        return Err(
                            "wasm target supports assignment to names, record fields and array elements only"
                                .to_string(),
                        )
                    }
                }
            }
            Stmt::Expr(expr @ Expr::Call(callee, args, _), _) => {
                let Expr::Ident(name) = &**callee else {
                    return Err("wasm target only supports calls to print()".to_string());
                };
                match name.as_str() {
                    "push" => {
                        if args.len() != 2 {
                            return Err(
                                "wasm target push(array, value) takes two arguments".to_string()
                            );
                        }
                        if !matches!(args[0], Expr::Ident(_)) {
                            return Err(
                                "wasm target pushes into a named array variable only".to_string()
                            );
                        }
                        let arr_ty = self.infer(&args[0])?;
                        let element = match arr_ty {
                            Ty::Arr => Ty::Int,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStrArr => Ty::ArrStr,
                            other => {
                                return Err(format!(
                                    "wasm target pushes into arrays only, got {}",
                                    ty_name(other)
                                ))
                            }
                        };
                        let pushed = self.infer(&args[1])?;
                        if pushed != element && !(element == Ty::Float && pushed == Ty::Int) {
                            return Err(format!(
                                "wasm target array elements hold {}",
                                ty_name(element)
                            ));
                        }
                        return Ok(());
                    }
                    "pop" => {
                        if args.len() != 1 || !matches!(args[0], Expr::Ident(_)) {
                            return Err(
                                "wasm target pop(array) takes a named array".to_string()
                            );
                        }
                        match self.infer(&args[0])? {
                            Ty::Arr | Ty::ArrStr | Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrF | Ty::ArrStrArr => {}
                            other => {
                                return Err(format!(
                                    "wasm target pops arrays only, got {}",
                                    ty_name(other)
                                ))
                            }
                        }
                        return Ok(());
                    }
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

                    "each" => {
                        if args.len() != 2 {
                            return Err(
                                "wasm target each() takes an array and a callback".to_string()
                            );
                        }
                        let elem = match self.infer(&args[0])? {
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            other => {
                                return Err(format!(
                                    "wasm target each() takes an array, got {}",
                                    ty_name(other)
                                ))
                            }
                        };
                        self.infer(&args[0])?;
                        let Ty::Func(id) = self.infer(&args[1])? else {
                            return Err(
                                "wasm target each() needs a function literal callback".to_string()
                            );
                        };
                        let sig = self
                            .lit_sigs
                            .get(id as usize)
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        if sig.params != vec![elem] {
                            return Err(format!(
                                "wasm target each() callbacks take one {}",
                                ty_name(elem)
                            ));
                        }
                        return Ok(());
                    }                    "sleep" => {
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
                    // Any other call whose result can be discarded is allowed in
                    // statement position (e.g. `sort(a)` or `reverse(a)`).
                    return self.infer(expr).map(|_| ());
                }
                if args.len() != 1 {
                    return Err("wasm target supports print(<value>) with one argument".to_string());
                }
                let ty = self.infer(&args[0])?;
                if matches!(ty, Ty::Func(_)) {
                    return Err("wasm target cannot log functions yet".to_string());
                }
            }

            Stmt::Expr(Expr::Compound(target, _op, value, _), _) => {
                if !matches!(**target, Expr::Ident(_)) {
                    match &**target {
                        Expr::Member(object, key, _) => {
                            let Ty::Obj(shape) = self.infer(object)? else {
                                return Err(
                                    "wasm target writes record fields only".to_string()
                                );
                            };
                            self.field_index(shape, key)?;
                        }
                        Expr::Index(array, index, _) => {
                            if self.infer(array)? != Ty::Arr {
                                return Err(
                                    "wasm target writes int array elements only".to_string()
                                );
                            }
                            if !is_number(self.infer(index)?) {
                                return Err("wasm target needs a numeric index".to_string());
                            }
                        }
                        _ => {
                            return Err(
                                "wasm target supports compound assignment to names, record fields and array elements only"
                                    .to_string(),
                            )
                        }
                    }
                    if !matches!(
                        _op,
                        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod
                    ) {
                        return Err(
                            "wasm target compound-assigns with + - * % only".to_string()
                        );
                    }
                    if self.infer(value)? != Ty::Int {
                        return Err("wasm target records and arrays hold ints".to_string());
                    }
                    return Ok(());
                }
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
                let element = match base {
                    Ty::Arr => Ty::Int,
                    Ty::ArrStr => Ty::Str,
                    Ty::ArrObj(shape) => Ty::Obj(shape),
                    Ty::ArrArr => Ty::Arr,
                    Ty::ArrF => Ty::Float,
                    Ty::ArrStrArr => Ty::ArrStr,
                    _ => {
                        return Err("wasm target supports .push on arrays only".to_string())
                    }
                };
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
                if self.infer(&args[0])? != element {
                    return Err(format!(
                        "wasm target array elements hold {}",
                        ty_name(element)
                    ));
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
                    Ty::Map | Ty::Obj(_) => {
                        scope.insert(key.clone(), Ty::Str);
                        if let Some(value) = value {
                            scope.insert(value.clone(), Ty::Int);
                        }
                    }
                    Ty::ArrStr => {
                        let (key_name, value_name) = match value {
                            Some(value) => (Some(key.clone()), value.clone()),
                            None => (None, key.clone()),
                        };
                        if let Some(key_name) = key_name {
                            scope.insert(key_name, Ty::Int);
                        }
                        scope.insert(value_name, Ty::Str);
                    }
                    Ty::ArrObj(shape) => {
                        let (key_name, value_name) = match value {
                            Some(value) => (Some(key.clone()), value.clone()),
                            None => (None, key.clone()),
                        };
                        if let Some(key_name) = key_name {
                            scope.insert(key_name, Ty::Int);
                        }
                        scope.insert(value_name, Ty::Obj(shape));
                    }
                    Ty::ArrArr => {
                        let (key_name, value_name) = match value {
                            Some(value) => (Some(key.clone()), value.clone()),
                            None => (None, key.clone()),
                        };
                        if let Some(key_name) = key_name {
                            scope.insert(key_name, Ty::Int);
                        }
                        scope.insert(value_name, Ty::Arr);
                    }
                    Ty::ArrF => {
                        let (key_name, value_name) = match value {
                            Some(value) => (Some(key.clone()), value.clone()),
                            None => (None, key.clone()),
                        };
                        if let Some(key_name) = key_name {
                            scope.insert(key_name, Ty::Int);
                        }
                        scope.insert(value_name, Ty::Float);
                    }
                    Ty::ArrStrArr => {
                        let (key_name, value_name) = match value {
                            Some(value) => (Some(key.clone()), value.clone()),
                            None => (None, key.clone()),
                        };
                        if let Some(key_name) = key_name {
                            scope.insert(key_name, Ty::Int);
                        }
                        scope.insert(value_name, Ty::ArrStr);
                    }
                    Ty::Str => {
                        let (key_name, value_name) = match value {
                            Some(value) => (Some(key.clone()), value.clone()),
                            None => (None, key.clone()),
                        };
                        if let Some(key_name) = key_name {
                            scope.insert(key_name, Ty::Int);
                        }
                        scope.insert(value_name, Ty::Str);
                    }                    other => {
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
const DST: u32 = 5;
const ALLOC_TMP: u32 = 6;
const ARG_TMP: u32 = 7;

/// Imported host functions: env.print_i64, env.print_utf8, env.print_f64, env.fmod,
/// bx.argc, bx.arg, bx.read_file, bx.write_file, bx.run, bx.capture,
/// web.console_log_i64, web.console_log_f64, web.console_log_utf8, web.performance_now.
const IMPORT_COUNT: u32 = 85;
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
const FILE_EXISTS: u32 = 28;
const MAP_NEW: u32 = 29;
const MAP_SET: u32 = 30;
const MAP_GET: u32 = 31;
const MAP_HAS: u32 = 32;
const MAP_LEN: u32 = 33;
const MAP_KEY_AT: u32 = 34;
const MAP_VALUE_AT: u32 = 35;
const BOOL_TO_STR: u32 = 36;
const STR_TEST: u32 = 37;
const PANIC: u32 = 38;
const STR_CHAR_AT: u32 = 39;
const STR_TRIM: u32 = 40;
const STR_UPPER: u32 = 41;
const STR_LOWER: u32 = 42;
const STR_REPLACE: u32 = 43;
const STR_INDEX_OF: u32 = 44;
const STR_REPEAT: u32 = 45;
const STR_SPLIT: u32 = 46;
const STR_JOIN: u32 = 47;
const STR_SLICE: u32 = 48;
const ARR_SLICE: u32 = 49;
const ARR_RANGE: u32 = 50;
const ARR_SUM: u32 = 51;
const ARR_REVERSE: u32 = 52;
const ARR_SORT: u32 = 53;
const PRINT_ARR_I64: u32 = 54;
const PRINT_ARR_STR: u32 = 55;
const PRINT_ARR_ARR: u32 = 58;
const PRINT_MAP: u32 = 59;
const ARR_INDEX_OF: u32 = 60;
const ARR_INDEX_OF_STR: u32 = 61;
const ARR_MINMAX: u32 = 62;
const ARR_MINMAX_STR: u32 = 63;
const PRINT_ARR_F64: u32 = 64;
const ARR_SUM_F: u32 = 65;
const ARR_MINMAX_F: u32 = 66;
const ARR_INDEX_OF_F: u32 = 67;
const ARR_SORT_F: u32 = 68;
const ARR_SORT_STR: u32 = 69;
const ARR_JOIN_INT: u32 = 70;
const ARR_JOIN_F: u32 = 71;
const PRINT_ARR_STRARR: u32 = 72;
const STR_LEN: u32 = 73;
const EXIT: u32 = 74;
const ERROR: u32 = 75;
const STR_TO_FLOAT: u32 = 76;
const WHICH: u32 = 77;
const PRINT_RECORD_TYPED: u32 = 78;
const PRINT_ARR_OBJ_TYPED: u32 = 79;
const EXEC_RECORD: u32 = 80;
const EXEC_RECORD_SOFT: u32 = 81;
const READ_FILE_SOFT: u32 = 82;
const WRITE_FILE_SOFT: u32 = 83;
const MKDIR_SOFT: u32 = 84;

#[derive(Clone, Copy)]
struct LocalSlot {
    ty: Ty,
    /// First wasm local index; strings own two slots (ptr, len). For captured
    /// names this is the environment offset instead.
    slot: u32,
    /// Value lives in a heap cell pointed to by `slot`.
    boxed: bool,
    /// Value lives in the enclosing literal's environment at this offset.
    capture: Option<u32>,
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
    /// shape id -> (data-segment base of its key table, key count).
    record_tables: HashMap<u32, (u32, u32)>,
    /// shape id -> data-segment base of its field-kind bytes.
    record_kind_tables: HashMap<u32, u32>,
    /// Active `attempt()` regions (innermost last); errors branch to the nearest.
    attempts: Vec<AttemptCtx>,
    /// Labels index that `continue` must branch to (innermost last).
    continue_targets: Vec<usize>,
    /// Captures of the literal currently being emitted (empty for functions).
    captures: Vec<Capture>,
    /// Wasm local holding this literal's environment pointer (first parameter).
    env_slot: Option<u32>,
    /// Names of the current frame that must live in heap cells.
    boxed: std::collections::HashSet<String>,
    /// Capture analysis shared by all emission (literal creation needs it).
    analysis: &'a CaptureAnalysis,
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
            if let Some(slot) = self.locals.get(name) {
                return Some(*slot);
            }
        }
        if let Some((offset, capture)) = self
            .captures
            .iter()
            .enumerate()
            .find(|(_, capture)| capture.name == name)
        {
            return Some(LocalSlot {
                ty: capture.ty,
                slot: offset as u32,
                boxed: false,
                capture: Some(offset as u32),
            });
        }
        None
    }

    /// Push the heap-cell pointer of a boxed local or a captured binding.
    fn push_cell_ptr(&mut self, slot: LocalSlot) -> Result<(), String> {
        if let Some(offset) = slot.capture {
            let env = self
                .env_slot
                .ok_or_else(|| "wasm target captured a name outside a literal".to_string())?;
            self.builder.push(Instr::Local(0x20, env));
            self.builder
                .push(Instr::Const(0x41, WVal::I32((4 * offset) as i32)));
            self.builder.push(Instr::Op(0x6a));
            self.builder.push(Instr::Mem(0x28, 2, 0));
        } else {
            self.builder.push(Instr::Local(0x20, slot.slot));
        }
        Ok(())
    }

    /// Push a local (or a string's ptr/len pair) onto the operand stack.
    fn push_local(&mut self, slot: LocalSlot) {
        if slot.capture.is_some() || slot.boxed {
            let _ = self.push_cell_ptr(slot);
            match slot.ty {
                Ty::Str => {
                    self.builder.push(Instr::Mem(0x28, 2, 0));
                    let _ = self.push_cell_ptr(slot);
                    self.builder.push(Instr::Mem(0x28, 2, 8));
                }
                Ty::Int => self.builder.push(Instr::Mem(0x29, 3, 0)),
                Ty::Float => self.builder.push(Instr::Mem(0x2b, 3, 0)),
                _ => self.builder.push(Instr::Mem(0x28, 2, 0)),
            }
            return;
        }
        self.builder.push(Instr::Local(0x20, slot.slot));
        if slot.ty == Ty::Str {
            self.builder.push(Instr::Local(0x20, slot.slot + 1));
        }
    }

    /// Pop the value (or ptr/len pair) into a local, or into its heap cell.
    fn pop_local(&mut self, slot: LocalSlot) {
        if slot.capture.is_none() && !slot.boxed {
            if slot.ty == Ty::Str {
                self.builder.push(Instr::Local(0x21, slot.slot + 1));
            }
            self.builder.push(Instr::Local(0x21, slot.slot));
            return;
        }
        match slot.ty {
            Ty::Str => {
                let len_tmp = self.builder.add_local(ValType::I32);
                let ptr_tmp = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x21, len_tmp));
                self.builder.push(Instr::Local(0x21, ptr_tmp));
                let _ = self.push_cell_ptr(slot);
                self.builder.push(Instr::Local(0x20, ptr_tmp));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                let _ = self.push_cell_ptr(slot);
                self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x20, len_tmp));
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
            Ty::Int => {
                let tmp = self.builder.add_local(ValType::I64);
                self.builder.push(Instr::Local(0x21, tmp));
                let _ = self.push_cell_ptr(slot);
                self.builder.push(Instr::Local(0x20, tmp));
                self.builder.push(Instr::Mem(0x37, 3, 0));
            }
            Ty::Float => {
                let tmp = self.builder.add_local(ValType::F64);
                self.builder.push(Instr::Local(0x21, tmp));
                let _ = self.push_cell_ptr(slot);
                self.builder.push(Instr::Local(0x20, tmp));
                self.builder.push(Instr::Mem(0x39, 3, 0));
            }
            _ => {
                let tmp = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x21, tmp));
                let _ = self.push_cell_ptr(slot);
                self.builder.push(Instr::Local(0x20, tmp));
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
        }
    }

    /// Move the value on top of the stack into a fresh heap cell and point the
    /// named local at that cell (used for captured locals/parameters).
    fn box_local(&mut self, name: &str, ty: Ty) -> Result<(), String> {
        let size = if ty == Ty::Str { 16 } else { 8 };
        match ty {
            Ty::Str => {
                let len_tmp = self.builder.add_local(ValType::I32);
                let ptr_tmp = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x21, len_tmp));
                self.builder.push(Instr::Local(0x21, ptr_tmp));
                self.emit_alloc_const(size);
                let cell = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x21, cell));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Local(0x20, ptr_tmp));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x20, len_tmp));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                self.locals.insert(
                    name.to_string(),
                    LocalSlot {
                        ty,
                        slot: cell,
                        boxed: true,
                        capture: None,
                    },
                );
            }
            _ => {
                let (load_op, store_op, tmp_ty) = match ty {
                    Ty::Int => (0x29u8, 0x37u8, ValType::I64),
                    Ty::Float => (0x2b, 0x39, ValType::F64),
                    _ => (0x28, 0x36, ValType::I32),
                };
                let _ = load_op;
                let tmp = self.builder.add_local(tmp_ty);
                self.builder.push(Instr::Local(0x21, tmp));
                self.emit_alloc_const(size);
                let cell = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x21, cell));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Local(0x20, tmp));
                self.builder.push(Instr::Mem(store_op, 3, 0));
                self.locals.insert(
                    name.to_string(),
                    LocalSlot {
                        ty,
                        slot: cell,
                        boxed: true,
                        capture: None,
                    },
                );
            }
        }
        Ok(())
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
            if *kind != 0x02 {
                continue;
            }
            // A continue block is the innermost `block` of a loop; `break`
            // must leave the loop's own exit block instead.
            let index = self.labels.len() - 1 - depth;
            if !self.continue_targets.contains(&index) {
                return Ok(depth as u32);
            }
        }
        Err("wasm target has no loop to break out of".to_string())
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

    /// Abort the module with `message` (host `env.panic`), mirroring VM aborts.
    fn emit_panic(&mut self, message: &str) {
        if let Some(ctx) = self.attempts.last().copied() {
            let base = self.builder.add_data(message.as_bytes());
            self.emit_attempt_error_constant(ctx.record, base, message.len() as u32);
            self.emit_attempt_jump();
            return;
        }
        let base = self.builder.add_data(message.as_bytes());
        self.builder
            .push(Instr::Const(0x41, WVal::I32(base as i32)));
        self.builder
            .push(Instr::Const(0x41, WVal::I32(message.len() as i32)));
        self.builder.push(Instr::Call(PANIC));
    }

    /// Store a constant error message into the attempt record and clear `ok`.
    fn emit_attempt_error_constant(&mut self, record: u32, base: u32, len: u32) {
        for (offset, value) in [(0u32, base), (4u32, len)] {
            self.builder.push(Instr::Local(0x20, record));
            self.builder
                .push(Instr::Const(0x41, WVal::I32(value as i32)));
            self.builder.push(Instr::Mem(0x36, 2, offset));
        }
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
        self.builder.push(Instr::Mem(0x37, 3, 8));
    }

    /// Store a runtime `(ptr, len)` error message into the attempt record.
    fn emit_attempt_error_pair(&mut self, record: u32) {
        let len = self.builder.add_local(ValType::I32);
        let ptr = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x21, len));
        self.builder.push(Instr::Local(0x21, ptr));
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, ptr));
        self.builder.push(Instr::Mem(0x36, 2, 0));
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, len));
        self.builder.push(Instr::Mem(0x36, 2, 4));
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
        self.builder.push(Instr::Mem(0x37, 3, 8));
    }

    /// Branch to the innermost enclosing `attempt` block.
    fn emit_attempt_jump(&mut self) {
        if let Some(depth) = self.labels.iter().rev().position(|kind| *kind == 0x05) {
            self.builder.push(Instr::Br(depth as u32));
        }
    }

    /// Store the attempt's success value into the record.
    fn emit_attempt_value(&mut self, record: u32, ty: Ty, value: &Expr) -> Result<(), String> {
        match ty {
            Ty::Str => {
                self.emit_str(value)?;
                self.builder.push(Instr::Local(0x21, self.temp(LEN_A)));
                self.builder.push(Instr::Local(0x21, self.temp(PTR_A)));
                self.builder.push(Instr::Local(0x20, record));
                self.builder.push(Instr::Local(0x20, self.temp(PTR_A)));
                self.builder.push(Instr::Mem(0x36, 2, 16));
                self.builder.push(Instr::Local(0x20, record));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                self.builder.push(Instr::Mem(0x36, 2, 20));
            }
            Ty::Float => {
                self.builder.push(Instr::Local(0x20, record));
                self.emit_f64(value)?;
                self.builder.push(Instr::Mem(0x39, 3, 16));
            }
            Ty::Bool => {
                self.builder.push(Instr::Local(0x20, record));
                self.emit_expr(value)?;
                self.builder.push(Instr::Op(0xad));
                self.builder.push(Instr::Mem(0x37, 3, 16));
            }
            Ty::Obj(_)
            | Ty::Arr
            | Ty::ArrStr
            | Ty::ArrF
            | Ty::ArrObj(_)
            | Ty::ArrArr
            | Ty::ArrStrArr => {
                self.builder.push(Instr::Local(0x20, record));
                self.emit_expr(value)?;
                self.builder.push(Instr::Mem(0x36, 2, 16));
            }
            _ => {
                self.builder.push(Instr::Local(0x20, record));
                self.emit_expr(value)?;
                self.builder.push(Instr::Mem(0x37, 3, 16));
            }
        }
        Ok(())
    }

    /// Push a continue-target block around a loop body so `continue` can reach
    /// the post-body step (index increment / for-step) instead of skipping it.
    fn push_continue_block(&mut self) {
        self.builder.push(Instr::Block {
            opcode: 0x02,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x02);
        self.continue_targets.push(self.labels.len() - 1);
    }

    fn pop_continue_block(&mut self) {
        self.continue_targets.pop();
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
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

    /// Lazily build a data-segment table: `count` u32 offsets followed by
    /// `[len][utf8]` key blocks for a record shape.
    fn record_table(&mut self, shape: u32) -> Result<(u32, u32), String> {
        if let Some(entry) = self.record_tables.get(&shape) {
            return Ok(*entry);
        }
        let keys = self.checker.shape_keys(shape)?;
        let count = keys.len() as u32;
        let mut data: Vec<u8> = Vec::new();
        let mut cursor = count * 4;
        let mut blocks: Vec<u8> = Vec::new();
        for key in &keys {
            data.extend_from_slice(&cursor.to_le_bytes());
            blocks.extend_from_slice(&(key.len() as u32).to_le_bytes());
            blocks.extend_from_slice(key.as_bytes());
            cursor += 4 + key.len() as u32;
        }
        data.extend_from_slice(&blocks);
        let base = self.builder.add_data(&data);
        self.record_tables.insert(shape, (base, count));
        Ok((base, count))
    }

    /// Lazily store one kind byte per record field for the typed print hosts.
    fn record_kinds_table(&mut self, shape: u32) -> Result<u32, String> {
        if let Some(base) = self.record_kind_tables.get(&shape) {
            return Ok(*base);
        }
        let kinds = self.checker.shape_kinds(shape)?;
        let base = self.builder.add_data(&kinds);
        self.record_kind_tables.insert(shape, base);
        Ok(base)
    }

    /// `for k, v in record { .. }`: declaration-order key (string) / value (int).
    fn emit_for_in_record(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        shape: u32,
        body: &[Stmt],
    ) -> Result<(), String> {
        let (table, count) = self.record_table(shape)?;
        let record_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I32);
        let block_tmp = self.builder.add_local(ValType::I32);
        let key_slot = self.builder.add_local(ValType::I32);
        let key_len_slot = self.builder.add_local(ValType::I32);
        let mut scope = HashMap::new();
        let key_boxed = self.boxed.contains(key);
        let key_cell = if key_boxed {
            Some(self.loop_cell(Ty::Str))
        } else {
            None
        };
        scope.insert(
            key.to_string(),
            LocalSlot {
                ty: Ty::Str,
                slot: key_cell.unwrap_or(key_slot),
                boxed: key_boxed,
                capture: None,
            },
        );
        let value_boxed = value.map(|name| self.boxed.contains(name)).unwrap_or(false);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Int))
        } else {
            None
        };
        let value_slot = value.map(|value| {
            let slot = self.builder.add_local(ValType::I64);
            scope.insert(
                value.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: value_cell.unwrap_or(slot),
                    boxed: value_boxed,
                    capture: None,
                },
            );
            slot
        });
        self.scoped.push(scope);
        self.emit_expr(iter)?;
        self.builder.push(Instr::Local(0x21, record_tmp));
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
        self.builder
            .push(Instr::Const(0x41, WVal::I32(count as i32)));
        self.builder.push(Instr::Op(0x48));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        // key block address = table + load(table + 4*index)
        self.builder
            .push(Instr::Const(0x41, WVal::I32(table as i32)));
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder
            .push(Instr::Const(0x41, WVal::I32(table as i32)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, block_tmp));
        self.builder.push(Instr::Local(0x20, block_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, key_len_slot));
        self.builder.push(Instr::Local(0x20, block_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, key_slot));
        if let Some(value_slot) = value_slot {
            self.builder.push(Instr::Local(0x20, record_tmp));
            self.builder.push(Instr::Local(0x20, index_tmp));
            self.builder.push(Instr::Const(0x41, WVal::I32(8)));
            self.builder.push(Instr::Op(0x6c));
            self.builder.push(Instr::Op(0x6a));
            self.builder.push(Instr::Mem(0x29, 3, 0));
            self.builder.push(Instr::Local(0x21, value_slot));
        }
        if let Some(cell) = key_cell {
            self.builder.push(Instr::Local(0x20, cell));
            self.builder.push(Instr::Local(0x20, key_slot));
            self.builder.push(Instr::Mem(0x36, 2, 0));
            self.builder.push(Instr::Local(0x20, cell));
            self.builder.push(Instr::Const(0x41, WVal::I32(8)));
            self.builder.push(Instr::Op(0x6a));
            self.builder.push(Instr::Local(0x20, key_len_slot));
            self.builder.push(Instr::Mem(0x36, 2, 0));
        }
        if let (Some(cell), Some(slot)) = (value_cell, value_slot) {
            self.refresh_loop_cell(cell, slot, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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

    /// `for i, r in record_array { .. }`: index (int) / element (record) locals.
    fn emit_for_in_arrobj(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        shape: u32,
        body: &[Stmt],
    ) -> Result<(), String> {
        let _ = shape;
        let array_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I64);
        let (key_name, value_name) = match value {
            Some(value) => (Some(key), value),
            None => (None, key),
        };
        let mut scope = HashMap::new();
        let value_tmp = self.builder.add_local(ValType::I32);
        let value_boxed = self.boxed.contains(value_name);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Obj(shape)))
        } else {
            None
        };
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::Obj(shape),
                slot: value_cell.unwrap_or(value_tmp),
                boxed: value_boxed,
                capture: None,
            },
        );
        // The element type carries no shape payload here; callers only need an
        // i32 record pointer, so Ty::Obj(shape) acts as the opaque record marker.
        let key_tmp = key_name.map(|name| {
            let tmp = self.builder.add_local(ValType::I64);
            let boxed = self.boxed.contains(name);
            let cell = if boxed {
                Some(self.loop_cell(Ty::Int))
            } else {
                None
            };
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: cell.unwrap_or(tmp),
                    boxed,
                    capture: None,
                },
            );
            tmp
        });
        let key_cell = key_name.and_then(|name| {
            if self.boxed.contains(name) {
                self.scoped
                    .last()
                    .and_then(|scope| scope.get(name))
                    .map(|slot| slot.slot)
            } else {
                None
            }
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
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, value_tmp));
        if let Some(cell) = value_cell {
            self.builder.push(Instr::Local(0x20, cell));
            self.builder.push(Instr::Local(0x20, value_tmp));
            self.builder.push(Instr::Mem(0x36, 2, 0));
        }
        if let (Some(cell), Some(tmp)) = (key_cell, key_tmp) {
            self.refresh_loop_cell(cell, tmp, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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
    /// `for i, x in floats { .. }`: index (int) / element (float) locals.
    fn emit_for_in_arrf(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let array_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I64);
        let (key_name, value_name) = match value {
            Some(value) => (Some(key), value),
            None => (None, key),
        };
        let mut scope = HashMap::new();
        let value_tmp = self.builder.add_local(ValType::F64);
        let value_boxed = self.boxed.contains(value_name);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Float))
        } else {
            None
        };
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::Float,
                slot: value_cell.unwrap_or(value_tmp),
                boxed: value_boxed,
                capture: None,
            },
        );
        let mut key_tmp = None;
        if let Some(name) = key_name {
            let tmp = self.builder.add_local(ValType::I64);
            let boxed = self.boxed.contains(name);
            let cell = if boxed {
                Some(self.loop_cell(Ty::Int))
            } else {
                None
            };
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: cell.unwrap_or(tmp),
                    boxed,
                    capture: None,
                },
            );
            key_tmp = Some(tmp);
        }
        let key_cell = key_name.and_then(|name| {
            if self.boxed.contains(name) {
                self.scoped
                    .last()
                    .and_then(|scope| scope.get(name))
                    .map(|slot| slot.slot)
            } else {
                None
            }
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
        self.builder.push(Instr::Mem(0x2b, 3, 0));
        self.builder.push(Instr::Local(0x21, value_tmp));
        if let Some(cell) = value_cell {
            self.builder.push(Instr::Local(0x20, cell));
            self.builder.push(Instr::Local(0x20, value_tmp));
            self.builder.push(Instr::Mem(0x39, 3, 0));
        }
        if let (Some(cell), Some(tmp)) = (key_cell, key_tmp) {
            self.refresh_loop_cell(cell, tmp, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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
    /// `for i, row in textRows { .. }`: index (int) / element (string array) locals.
    fn emit_for_in_arrstrarr(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let array_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I64);
        let (key_name, value_name) = match value {
            Some(value) => (Some(key), value),
            None => (None, key),
        };
        let mut scope = HashMap::new();
        let value_tmp = self.builder.add_local(ValType::I32);
        let value_boxed = self.boxed.contains(value_name);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::ArrStr))
        } else {
            None
        };
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::ArrStr,
                slot: value_cell.unwrap_or(value_tmp),
                boxed: value_boxed,
                capture: None,
            },
        );
        let mut key_tmp = None;
        if let Some(name) = key_name {
            let tmp = self.builder.add_local(ValType::I64);
            let boxed = self.boxed.contains(name);
            let cell = if boxed {
                Some(self.loop_cell(Ty::Int))
            } else {
                None
            };
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: cell.unwrap_or(tmp),
                    boxed,
                    capture: None,
                },
            );
            key_tmp = Some(tmp);
        }
        let key_cell = key_name.and_then(|name| {
            if self.boxed.contains(name) {
                self.scoped
                    .last()
                    .and_then(|scope| scope.get(name))
                    .map(|slot| slot.slot)
            } else {
                None
            }
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
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, value_tmp));
        if let Some(cell) = value_cell {
            self.builder.push(Instr::Local(0x20, cell));
            self.builder.push(Instr::Local(0x20, value_tmp));
            self.builder.push(Instr::Mem(0x36, 2, 0));
        }
        if let (Some(cell), Some(tmp)) = (key_cell, key_tmp) {
            self.refresh_loop_cell(cell, tmp, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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
    /// `for i, row in matrix { .. }`: index (int) / element (int array) locals.
    fn emit_for_in_arrarr(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let array_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I64);
        let (key_name, value_name) = match value {
            Some(value) => (Some(key), value),
            None => (None, key),
        };
        let mut scope = HashMap::new();
        let value_tmp = self.builder.add_local(ValType::I32);
        let value_boxed = self.boxed.contains(value_name);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Arr))
        } else {
            None
        };
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::Arr,
                slot: value_cell.unwrap_or(value_tmp),
                boxed: value_boxed,
                capture: None,
            },
        );
        let key_tmp = key_name.map(|name| {
            let tmp = self.builder.add_local(ValType::I64);
            let boxed = self.boxed.contains(name);
            let cell = if boxed {
                Some(self.loop_cell(Ty::Int))
            } else {
                None
            };
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: cell.unwrap_or(tmp),
                    boxed,
                    capture: None,
                },
            );
            tmp
        });
        let key_cell = key_name.and_then(|name| {
            if self.boxed.contains(name) {
                self.scoped
                    .last()
                    .and_then(|scope| scope.get(name))
                    .map(|slot| slot.slot)
            } else {
                None
            }
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
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, value_tmp));
        if let Some(cell) = value_cell {
            self.builder.push(Instr::Local(0x20, cell));
            self.builder.push(Instr::Local(0x20, value_tmp));
            self.builder.push(Instr::Mem(0x36, 2, 0));
        }
        if let (Some(cell), Some(tmp)) = (key_cell, key_tmp) {
            self.refresh_loop_cell(cell, tmp, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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
    /// `for i, s in strarr { .. }`: index (int) / element (string) locals.
    fn emit_for_in_strarr(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let array_tmp = self.builder.add_local(ValType::I32);
        let index_tmp = self.builder.add_local(ValType::I64);
        let (key_name, value_name) = match value {
            Some(value) => (Some(key), value),
            None => (None, key),
        };
        let mut scope = HashMap::new();
        let value_ptr = self.builder.add_local(ValType::I32);
        let value_len = self.builder.add_local(ValType::I32);
        let value_boxed = self.boxed.contains(value_name);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Str))
        } else {
            None
        };
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::Str,
                slot: value_cell.unwrap_or(value_ptr),
                boxed: value_boxed,
                capture: None,
            },
        );
        let key_tmp = key_name.map(|name| {
            let tmp = self.builder.add_local(ValType::I64);
            let boxed = self.boxed.contains(name);
            let cell = if boxed {
                Some(self.loop_cell(Ty::Int))
            } else {
                None
            };
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: cell.unwrap_or(tmp),
                    boxed,
                    capture: None,
                },
            );
            tmp
        });
        let key_cell = key_name.and_then(|name| {
            if self.boxed.contains(name) {
                self.scoped
                    .last()
                    .and_then(|scope| scope.get(name))
                    .map(|slot| slot.slot)
            } else {
                None
            }
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
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, value_ptr));
        self.builder.push(Instr::Local(0x20, array_tmp));
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Op(0xa7));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, value_len));
        self.builder.push(Instr::Local(0x20, value_ptr));
        self.builder.push(Instr::Local(0x20, value_len));
        if let Some(cell) = value_cell {
            self.refresh_loop_cell(cell, value_ptr, Ty::Str);
        }
        if let (Some(cell), Some(tmp)) = (key_cell, key_tmp) {
            self.refresh_loop_cell(cell, tmp, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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
        let key_boxed = self.boxed.contains(key);
        let key_cell = if key_boxed {
            Some(self.loop_cell(Ty::Str))
        } else {
            None
        };
        scope.insert(
            key.to_string(),
            LocalSlot {
                ty: Ty::Str,
                slot: key_cell.unwrap_or(key_slot),
                boxed: key_boxed,
                capture: None,
            },
        );
        let value_boxed = value.map(|name| self.boxed.contains(name)).unwrap_or(false);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Int))
        } else {
            None
        };
        let value_slot = value.map(|value| {
            let slot = self.builder.add_local(ValType::I64);
            scope.insert(
                value.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: value_cell.unwrap_or(slot),
                    boxed: value_boxed,
                    capture: None,
                },
            );
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
        if let Some(cell) = key_cell {
            self.refresh_loop_cell(cell, key_slot, Ty::Str);
        }
        if let (Some(cell), Some(slot)) = (value_cell, value_slot) {
            self.refresh_loop_cell(cell, slot, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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
    /// Allocate a heap cell (once, before a loop) and return its wasm local.
    fn loop_cell(&mut self, ty: Ty) -> u32 {
        let size = if ty == Ty::Str { 16 } else { 8 };
        self.emit_alloc_const(size);
        let cell = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x21, cell));
        cell
    }

    /// Copy a loop temporary into its cell after the per-iteration refresh.
    fn refresh_loop_cell(&mut self, cell: u32, temp: u32, ty: Ty) {
        self.builder.push(Instr::Local(0x20, cell));
        match ty {
            Ty::Str => {
                self.builder.push(Instr::Local(0x20, temp));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x20, temp + 1));
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
            Ty::Int => {
                self.builder.push(Instr::Local(0x20, temp));
                self.builder.push(Instr::Mem(0x37, 3, 0));
            }
            _ => {
                self.builder.push(Instr::Local(0x20, temp));
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
        }
    }
    /// `for i, ch in "text" { .. }`: iterate Unicode scalar values.
    fn emit_for_in_strchars(
        &mut self,
        key: &str,
        value: Option<&str>,
        iter: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        let ptr_tmp = self.builder.add_local(ValType::I32);
        let len_tmp = self.builder.add_local(ValType::I32);
        let count_tmp = self.builder.add_local(ValType::I64);
        let index_tmp = self.builder.add_local(ValType::I64);
        let (key_name, value_name) = match value {
            Some(value) => (Some(key), value),
            None => (None, key),
        };
        let mut scope = HashMap::new();
        let value_ptr = self.builder.add_local(ValType::I32);
        let _value_len_slot = self.builder.add_local(ValType::I32);
        let value_len = value_ptr + 1;
        let value_boxed = self.boxed.contains(value_name);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Str))
        } else {
            None
        };
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::Str,
                slot: value_cell.unwrap_or(value_ptr),
                boxed: value_boxed,
                capture: None,
            },
        );
        let mut key_tmp = None;
        if let Some(name) = key_name {
            let tmp = self.builder.add_local(ValType::I64);
            let boxed = self.boxed.contains(name);
            let cell = if boxed {
                Some(self.loop_cell(Ty::Int))
            } else {
                None
            };
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: cell.unwrap_or(tmp),
                    boxed,
                    capture: None,
                },
            );
            key_tmp = Some(tmp);
        }
        let key_cell = key_name.and_then(|name| {
            if self.boxed.contains(name) {
                self.scoped
                    .last()
                    .and_then(|scope| scope.get(name))
                    .map(|slot| slot.slot)
            } else {
                None
            }
        });
        self.scoped.push(scope);
        self.emit_str(iter)?;
        self.builder.push(Instr::Local(0x21, len_tmp));
        self.builder.push(Instr::Local(0x21, ptr_tmp));
        self.builder.push(Instr::Local(0x20, ptr_tmp));
        self.builder.push(Instr::Local(0x20, len_tmp));
        self.builder.push(Instr::Call(STR_LEN));
        self.builder.push(Instr::Local(0x21, count_tmp));
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
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Local(0x20, count_tmp));
        self.builder.push(Instr::Op(0x53));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        if let Some(key_tmp) = key_tmp {
            self.builder.push(Instr::Local(0x20, index_tmp));
            self.builder.push(Instr::Local(0x21, key_tmp));
        }
        self.builder.push(Instr::Local(0x20, ptr_tmp));
        self.builder.push(Instr::Local(0x20, len_tmp));
        self.builder.push(Instr::Local(0x20, index_tmp));
        self.builder.push(Instr::Op(0xa7));
        self.builder.push(Instr::Call(STR_CHAR_AT));
        self.push_block_as_pair();
        match value_cell {
            Some(cell) => {
                self.builder.push(Instr::Local(0x21, value_len));
                self.builder.push(Instr::Local(0x21, value_ptr));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Local(0x20, value_ptr));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x20, value_len));
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
            None => {
                self.builder.push(Instr::Local(0x21, value_len));
                self.builder.push(Instr::Local(0x21, value_ptr));
            }
        }
        if let (Some(cell), Some(tmp)) = (key_cell, key_tmp) {
            self.refresh_loop_cell(cell, tmp, Ty::Int);
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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
        if iter_ty == Ty::Str {
            return self.emit_for_in_strchars(key, value, iter, body);
        }
        if iter_ty == Ty::ArrStr {
            return self.emit_for_in_strarr(key, value, iter, body);
        }
        if let Ty::ArrObj(shape) = iter_ty {
            return self.emit_for_in_arrobj(key, value, iter, shape, body);
        }
        if iter_ty == Ty::ArrArr {
            return self.emit_for_in_arrarr(key, value, iter, body);
        }
        if iter_ty == Ty::ArrF {
            return self.emit_for_in_arrf(key, value, iter, body);
        }
        if iter_ty == Ty::ArrStrArr {
            return self.emit_for_in_arrstrarr(key, value, iter, body);
        }
        if let Ty::Obj(shape) = iter_ty {
            return self.emit_for_in_record(key, value, iter, shape, body);
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
        let value_boxed = self.boxed.contains(value_name);
        let value_cell = if value_boxed {
            Some(self.loop_cell(Ty::Int))
        } else {
            None
        };
        scope.insert(
            value_name.to_string(),
            LocalSlot {
                ty: Ty::Int,
                slot: value_cell.unwrap_or(value_tmp),
                boxed: value_boxed,
                capture: None,
            },
        );
        let key_tmp = key_name.map(|name| {
            let tmp = self.builder.add_local(ValType::I64);
            let boxed = self.boxed.contains(name);
            let cell = if boxed {
                Some(self.loop_cell(Ty::Int))
            } else {
                None
            };
            scope.insert(
                name.to_string(),
                LocalSlot {
                    ty: Ty::Int,
                    slot: cell.unwrap_or(tmp),
                    boxed,
                    capture: None,
                },
            );
            tmp
        });
        let key_boxed = key_name
            .map(|name| self.boxed.contains(name))
            .unwrap_or(false);
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
        if let (true, Some(cell)) = (value_boxed, value_cell) {
            self.refresh_loop_cell(cell, value_tmp, Ty::Int);
        }
        if key_boxed {
            if let Some(key_tmp) = key_tmp {
                if let Some(slot) = self
                    .scoped
                    .last()
                    .and_then(|scope| scope.get(key_name.unwrap()))
                {
                    self.refresh_loop_cell(slot.slot, key_tmp, Ty::Int);
                }
            }
        }
        self.push_continue_block();
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        self.pop_continue_block();
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

    /// `exec(cmd[, opts])` / `sh(cmd)`: run through the host and build the
    /// `{code, err, ms, ok, out, timeout}` record the VM returns.
    fn emit_exec_record(&mut self, args: &[Expr], shell: bool) -> Result<Ty, String> {
        let mode = match self.type_of(&args[0])? {
            Ty::ArrStr => 1,
            _ => 0,
        };
        let cmd_ptr = self.builder.add_local(ValType::I32);
        let cmd_len = self.builder.add_local(ValType::I32);
        match mode {
            1 => {
                self.emit_expr(&args[0])?;
                self.builder.push(Instr::Local(0x21, cmd_ptr));
                self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                self.builder.push(Instr::Local(0x21, cmd_len));
            }
            _ => {
                self.emit_str(&args[0])?;
                self.builder.push(Instr::Local(0x21, cmd_len));
                self.builder.push(Instr::Local(0x21, cmd_ptr));
            }
        }
        let cwd_ptr = self.builder.add_local(ValType::I32);
        let cwd_len = self.builder.add_local(ValType::I32);
        let stdin_ptr = self.builder.add_local(ValType::I32);
        let stdin_len = self.builder.add_local(ValType::I32);
        let timeout = self.builder.add_local(ValType::I64);
        let shell_flag = self.builder.add_local(ValType::I32);
        for slot in [cwd_ptr, cwd_len, stdin_ptr, stdin_len] {
            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
            self.builder.push(Instr::Local(0x21, slot));
        }
        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
        self.builder.push(Instr::Local(0x21, timeout));
        self.builder
            .push(Instr::Const(0x41, WVal::I32(if shell { 1 } else { 0 })));
        self.builder.push(Instr::Local(0x21, shell_flag));
        if let Some(Expr::Object(fields)) = args.get(1) {
            for (key, value) in fields {
                match key.as_str() {
                    "cwd" => {
                        self.emit_str(value)?;
                        self.builder.push(Instr::Local(0x21, cwd_len));
                        self.builder.push(Instr::Local(0x21, cwd_ptr));
                    }
                    "stdin" => {
                        self.emit_str(value)?;
                        self.builder.push(Instr::Local(0x21, stdin_len));
                        self.builder.push(Instr::Local(0x21, stdin_ptr));
                    }
                    "timeout" => {
                        self.emit_expr(value)?;
                        self.builder.push(Instr::Local(0x21, timeout));
                    }
                    "shell" => {
                        self.emit_expr(value)?;
                        self.builder.push(Instr::Local(0x21, shell_flag));
                    }
                    _ => {}
                }
            }
        }
        self.builder.push(Instr::Const(0x41, WVal::I32(mode)));
        self.builder.push(Instr::Local(0x20, cmd_ptr));
        self.builder.push(Instr::Local(0x20, cmd_len));
        self.builder.push(Instr::Local(0x20, cwd_ptr));
        self.builder.push(Instr::Local(0x20, cwd_len));
        self.builder.push(Instr::Local(0x20, stdin_ptr));
        self.builder.push(Instr::Local(0x20, stdin_len));
        self.builder.push(Instr::Local(0x20, timeout));
        self.builder.push(Instr::Op(0xa7));
        self.builder.push(Instr::Local(0x20, shell_flag));
        self.builder.push(Instr::Const(0x41, WVal::I32(0)));
        let soft = !self.attempts.is_empty();
        self.builder.push(Instr::Call(if soft {
            EXEC_RECORD_SOFT
        } else {
            EXEC_RECORD
        }));
        let block = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x21, block));
        if soft {
            // flags bit 1: the host could not start the command; report it as
            // the attempt error instead of trapping.
            self.builder.push(Instr::Local(0x20, block));
            self.builder.push(Instr::Mem(0x28, 2, 24));
            self.builder.push(Instr::Const(0x41, WVal::I32(2)));
            self.builder.push(Instr::Op(0x71));
            self.builder.push(Instr::Block {
                opcode: 0x04,
                end: 0,
                else_at: 0,
                start: 0,
            });
            self.labels.push(0x04);
            self.builder.push(Instr::Local(0x20, block));
            self.builder.push(Instr::Mem(0x28, 2, 12));
            self.builder.push(Instr::Local(0x20, block));
            self.builder.push(Instr::Mem(0x28, 2, 16));
            if let Some(ctx) = self.attempts.last().copied() {
                self.emit_attempt_error_pair(ctx.record);
            }
            self.emit_attempt_jump();
            self.labels.pop();
            self.builder.push(Instr::Op(0x0b));
        }
        let _shape = self.checker.exec_result_shape();
        self.emit_alloc_const(48);
        let record = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x21, record));
        let err_ptr = self.builder.add_local(ValType::I32);
        let err_len = self.builder.add_local(ValType::I32);
        let out_ptr = self.builder.add_local(ValType::I32);
        let out_len = self.builder.add_local(ValType::I32);
        let ms = self.builder.add_local(ValType::I32);
        let flags = self.builder.add_local(ValType::I32);
        for (slot, offset) in [
            (err_ptr, 12u32),
            (err_len, 16),
            (out_ptr, 4),
            (out_len, 8),
            (ms, 20),
            (flags, 24),
        ] {
            self.builder.push(Instr::Local(0x20, block));
            self.builder.push(Instr::Mem(0x28, 2, offset));
            self.builder.push(Instr::Local(0x21, slot));
        }
        // code (int at 0)
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Op(0xad));
        self.builder.push(Instr::Mem(0x37, 3, 0));
        // err (string at 8/12)
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, err_ptr));
        self.builder.push(Instr::Mem(0x36, 2, 8));
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, err_len));
        self.builder.push(Instr::Mem(0x36, 2, 12));
        // ms (int at 16)
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, ms));
        self.builder.push(Instr::Op(0xad));
        self.builder.push(Instr::Mem(0x37, 3, 16));
        // ok = code == 0 && flags & 1 == 0 (bool at 24)
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::Local(0x20, flags));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x71));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::Op(0x71));
        self.builder.push(Instr::Op(0xad));
        self.builder.push(Instr::Mem(0x37, 3, 24));
        // out (string at 32/36)
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, out_ptr));
        self.builder.push(Instr::Mem(0x36, 2, 32));
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, out_len));
        self.builder.push(Instr::Mem(0x36, 2, 36));
        // timeout = flags & 1 != 0 (bool at 40)
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Local(0x20, flags));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x71));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::Op(0xad));
        self.builder.push(Instr::Mem(0x37, 3, 40));
        self.builder.push(Instr::Local(0x20, record));
        Ok(Ty::Obj(self.checker.exec_result_shape()))
    }

    /// Element storage mode for a comparator sort; every element occupies one
    /// 8-byte slot, so runs can be copied raw and only the callback arguments
    /// need per-kind conversion.
    fn emit_merge_sort(
        &mut self,
        array: &Expr,
        comparator: &Expr,
        input_ty: Ty,
        mode: SortMode,
        sig: &FnSig,
    ) -> Result<(), String> {
        let arr = self.builder.add_local(ValType::I32);
        let count = self.builder.add_local(ValType::I32);
        let temp = self.builder.add_local(ValType::I32);
        let cb_env = self.builder.add_local(ValType::I32);
        self.emit_expr(array)?;
        self.builder.push(Instr::Local(0x21, arr));
        self.emit_expr(comparator)?;
        self.builder.push(Instr::Local(0x21, cb_env));
        self.builder.push(Instr::Local(0x20, arr));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, count));
        // temp = alloc(count * 8 + 8); slot 0 lives at temp + 4 so the shared
        // slot-address helper works for both the array and the scratch buffer.
        self.builder.push(Instr::Local(0x20, count));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6a));
        self.emit_alloc_dynamic();
        self.builder.push(Instr::Local(0x21, temp));
        let width = self.builder.add_local(ValType::I32);
        let index = self.builder.add_local(ValType::I32);
        let mid = self.builder.add_local(ValType::I32);
        let hi = self.builder.add_local(ValType::I32);
        let left = self.builder.add_local(ValType::I32);
        let right = self.builder.add_local(ValType::I32);
        let out = self.builder.add_local(ValType::I32);
        let addr_a = self.builder.add_local(ValType::I32);
        let addr_b = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Local(0x21, width));
        // while width < count
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
        self.builder.push(Instr::Local(0x20, width));
        self.builder.push(Instr::Local(0x20, count));
        self.builder.push(Instr::Op(0x48));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        self.builder.push(Instr::Const(0x41, WVal::I32(0)));
        self.builder.push(Instr::Local(0x21, index));
        // for each pair of runs
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
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Local(0x20, count));
        self.builder.push(Instr::Op(0x48));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        // mid = min(index + width, count); hi = min(index + 2*width, count)
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Local(0x20, width));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, mid));
        self.emit_clamp_i32(mid, count);
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Local(0x20, width));
        self.builder.push(Instr::Const(0x41, WVal::I32(2)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, hi));
        self.emit_clamp_i32(hi, count);
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Local(0x21, left));
        self.builder.push(Instr::Local(0x20, mid));
        self.builder.push(Instr::Local(0x21, right));
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Local(0x21, out));
        // merge the two runs
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
        self.builder.push(Instr::Local(0x20, left));
        self.builder.push(Instr::Local(0x20, mid));
        self.builder.push(Instr::Op(0x48));
        self.builder.push(Instr::Local(0x20, right));
        self.builder.push(Instr::Local(0x20, hi));
        self.builder.push(Instr::Op(0x48));
        self.builder.push(Instr::Op(0x71));
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        self.emit_slot_addr(arr, left, addr_a);
        self.emit_slot_addr(arr, right, addr_b);
        self.emit_sort_compare(mode, sig, cb_env, addr_a, addr_b);
        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
        self.builder.push(Instr::Op(0x57));
        self.builder.push(Instr::Block {
            opcode: 0x04,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x04);
        self.emit_slot_copy(temp, out, arr, left);
        self.builder.push(Instr::Local(0x20, left));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, left));
        self.builder.push(Instr::Else { end: 0 });
        self.emit_slot_copy(temp, out, arr, right);
        self.builder.push(Instr::Local(0x20, right));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, right));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.builder.push(Instr::Local(0x20, out));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, out));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        // copy the left tail
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
        self.builder.push(Instr::Local(0x20, left));
        self.builder.push(Instr::Local(0x20, mid));
        self.builder.push(Instr::Op(0x4f));
        self.builder.push(Instr::BrIf(1));
        self.emit_slot_copy(temp, out, arr, left);
        self.builder.push(Instr::Local(0x20, left));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, left));
        self.builder.push(Instr::Local(0x20, out));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, out));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        // copy the right tail
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
        self.builder.push(Instr::Local(0x20, right));
        self.builder.push(Instr::Local(0x20, hi));
        self.builder.push(Instr::Op(0x4f));
        self.builder.push(Instr::BrIf(1));
        self.emit_slot_copy(temp, out, arr, right);
        self.builder.push(Instr::Local(0x20, right));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, right));
        self.builder.push(Instr::Local(0x20, out));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, out));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        // index += 2*width
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Local(0x20, width));
        self.builder.push(Instr::Const(0x41, WVal::I32(2)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, index));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        // arr slots <- temp
        self.builder.push(Instr::Local(0x20, arr));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, temp));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, count));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::MemoryCopy);
        // width *= 2
        self.builder.push(Instr::Local(0x20, width));
        self.builder.push(Instr::Const(0x41, WVal::I32(2)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Local(0x21, width));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.builder.push(Instr::Local(0x20, arr));
        let _ = input_ty;
        Ok(())
    }

    /// `if target > bound { target = bound }`
    fn emit_clamp_i32(&mut self, target: u32, bound: u32) {
        self.builder.push(Instr::Local(0x20, target));
        self.builder.push(Instr::Local(0x20, bound));
        self.builder.push(Instr::Op(0x4a));
        self.builder.push(Instr::Block {
            opcode: 0x04,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x04);
        self.builder.push(Instr::Local(0x20, bound));
        self.builder.push(Instr::Local(0x21, target));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
    }

    /// Push the address of `array[slot]` into the `out` local.
    fn emit_slot_addr(&mut self, base: u32, slot: u32, out: u32) {
        self.builder.push(Instr::Local(0x20, base));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, slot));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, out));
    }

    /// Copy one 8-byte element slot (raw copy: valid for every sort mode).
    fn emit_slot_copy(&mut self, dst_base: u32, dst_slot: u32, src_base: u32, src_slot: u32) {
        let dst = self.builder.add_local(ValType::I32);
        let src = self.builder.add_local(ValType::I32);
        self.emit_slot_addr(dst_base, dst_slot, dst);
        self.emit_slot_addr(src_base, src_slot, src);
        self.builder.push(Instr::Local(0x20, dst));
        self.builder.push(Instr::Local(0x20, src));
        self.builder.push(Instr::Mem(0x29, 3, 0));
        self.builder.push(Instr::Mem(0x37, 3, 0));
    }

    /// Call the comparator on the two slots, leaving its i64 result.
    fn emit_sort_compare(&mut self, mode: SortMode, sig: &FnSig, cb_env: u32, a: u32, b: u32) {
        self.builder.push(Instr::Local(0x20, cb_env));
        match mode {
            SortMode::Int => {
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x29, 3, 0));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x29, 3, 0));
            }
            SortMode::Str => {
                for (slot, offset) in [(a, 0u32), (a, 4), (b, 0), (b, 4)] {
                    self.builder.push(Instr::Local(0x20, slot));
                    self.builder.push(Instr::Mem(0x28, 2, offset));
                }
            }
            SortMode::Float => {
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x29, 3, 0));
                self.builder.push(Instr::Op(0xbf));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x29, 3, 0));
                self.builder.push(Instr::Op(0xbf));
            }
            SortMode::Ptr => {
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x28, 2, 0));
            }
        }
        self.builder.push(Instr::Call(sig.index));
    }

    /// `attempt(fn() { ... })`: run the literal body inline, turning the subset's
    /// guest-detected failures (bounds, division by zero, explicit `error()`,
    /// min/max/reduce/get/pop guards) into the VM's `{error, ok, value}` record.
    fn emit_attempt(&mut self, callback: &Expr) -> Result<Ty, String> {
        let Ty::Func(id) = self.type_of(callback)? else {
            return Err("wasm target attempt() needs a function literal".to_string());
        };
        let sig = self
            .checker
            .lit_sigs
            .get(id as usize)
            .cloned()
            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
        if !sig.params.is_empty() {
            return Err("wasm target attempt() callbacks take no parameters".to_string());
        }
        let result_ty = sig.result.unwrap_or(Ty::Int);
        let shape = self.checker.attempt_shape(result_ty)?;
        let record = self.builder.add_local(ValType::I32);
        self.emit_alloc_const(24);
        self.builder.push(Instr::Local(0x21, record));
        // error = "", ok = true, value = zero value of the result kind
        for (offset, value) in [(0u32, 0i32), (4, 0)] {
            self.builder.push(Instr::Local(0x20, record));
            self.builder.push(Instr::Const(0x41, WVal::I32(value)));
            self.builder.push(Instr::Mem(0x36, 2, offset));
        }
        self.builder.push(Instr::Local(0x20, record));
        self.builder.push(Instr::Const(0x42, WVal::I64(1)));
        self.builder.push(Instr::Mem(0x37, 3, 8));
        match result_ty {
            Ty::Str => {
                self.builder.push(Instr::Local(0x20, record));
                self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                self.builder.push(Instr::Mem(0x36, 2, 16));
                self.builder.push(Instr::Local(0x20, record));
                self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                self.builder.push(Instr::Mem(0x36, 2, 20));
            }
            Ty::Float => {
                self.builder.push(Instr::Local(0x20, record));
                self.builder.push(Instr::Const(0x44, WVal::F64(0.0)));
                self.builder.push(Instr::Mem(0x39, 3, 16));
            }
            _ => {
                self.builder.push(Instr::Local(0x20, record));
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.builder.push(Instr::Mem(0x37, 3, 16));
            }
        }
        self.builder.push(Instr::Block {
            opcode: 0x02,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x05);
        self.attempts.push(AttemptCtx { record, result_ty });
        self.scoped.push(HashMap::new());
        let Expr::Func(_, body, _) = callback else {
            return Err("wasm target attempt() needs a function literal".to_string());
        };
        // The body is inlined into the current function, so `let` must use frame
        // locals (the callback still also exists as its own function; cleanup
        // below keeps its names out of the enclosing scope afterwards).
        let saved_mode = self.fn_mode;
        self.fn_mode = true;
        let declared_before: Vec<String> = self.locals.keys().cloned().collect();
        let mut outcome = Ok(());
        for stmt in body {
            if let Err(error) = self.emit_stmt(stmt) {
                outcome = Err(error);
                break;
            }
        }
        let declared_after: Vec<String> = self.locals.keys().cloned().collect();
        for name in declared_after {
            if !declared_before.contains(&name) {
                self.locals.remove(&name);
            }
        }
        self.fn_mode = saved_mode;
        outcome?;
        self.scoped.pop();
        self.attempts.pop();
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.builder.push(Instr::Local(0x20, record));
        Ok(Ty::Obj(shape))
    }

    /// Non-attempt soft call: trap with the block's runtime message on failure,
    /// leaving nothing on the stack afterwards.
    fn emit_soft_io_panic(&mut self, block: u32) {
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Block {
            opcode: 0x04,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x04);
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Mem(0x28, 2, 4));
        self.builder.push(Instr::Call(PANIC));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
    }

    /// `[status][len][data]` block from the soft file imports: branch to the
    /// active attempt with the message when status is non-zero.
    fn emit_soft_io_check(&mut self, block: u32) {
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Block {
            opcode: 0x04,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x04);
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, block));
        self.builder.push(Instr::Mem(0x28, 2, 4));
        if let Some(ctx) = self.attempts.last().copied() {
            self.emit_attempt_error_pair(ctx.record);
        }
        self.emit_attempt_jump();
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
    }
    /// Emit the address of an assignable record field / array element.
    fn emit_target_address(&mut self, target: &Expr) -> Result<Ty, String> {
        match target {
            Expr::Member(object, key, _) => {
                let Ty::Obj(shape) = self.type_of(object)? else {
                    return Err("wasm target writes record fields only".to_string());
                };
                let index = self.checker.field_index(shape, key)?;
                self.emit_expr(object)?;
                self.builder
                    .push(Instr::Const(0x41, WVal::I32((8 * index) as i32)));
                self.builder.push(Instr::Op(0x6a));
                Ok(Ty::Int)
            }
            Expr::Index(array, index, _) => {
                let array_tmp = self.builder.add_local(ValType::I32);
                let index_tmp = self.builder.add_local(ValType::I64);
                self.emit_expr(array)?;
                self.builder.push(Instr::Local(0x21, array_tmp));
                if self.type_of(index)? == Ty::Float {
                    self.emit_f64(index)?;
                    self.builder.push(Instr::Op(0xb0));
                } else {
                    self.emit_expr(index)?;
                }
                self.builder.push(Instr::Local(0x21, index_tmp));
                // negative indexes count from the end
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.builder.push(Instr::Op(0x53));
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Local(0x20, array_tmp));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Op(0xad));
                self.builder.push(Instr::Op(0x7c));
                self.builder.push(Instr::Local(0x21, index_tmp));
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                // bounds check: index < 0 || index >= len
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.builder.push(Instr::Op(0x53));
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Local(0x20, array_tmp));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Op(0xad));
                self.builder.push(Instr::Op(0x59));
                self.builder.push(Instr::Op(0x72));
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.emit_panic("index out of bounds");
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.builder.push(Instr::Local(0x20, array_tmp));
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Op(0xa7));
                self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                self.builder.push(Instr::Op(0x6c));
                self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Op(0x6a));
                Ok(Ty::Int)
            }
            _ => Err(
                "wasm target supports assignment to names, record fields and array elements only"
                    .to_string(),
            ),
        }
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

    /// Expression-form `pop(arr)`: read the last element, shrink the array and
    /// return the removed value (the empty case aborts; the VM returns null).
    fn emit_pop_value(&mut self, name: &str) -> Result<Ty, String> {
        let arr_ty = match self.local_of(name) {
            Some(slot) => slot.ty,
            None => self.checker.binding(name)?.ty,
        };
        let element = element_ty(arr_ty)
            .ok_or_else(|| format!("wasm target pops arrays only, got {}", ty_name(arr_ty)))?;
        let arr_tmp = self.builder.add_local(ValType::I32);
        let len_tmp = self.builder.add_local(ValType::I32);
        self.emit_load(name)?;
        self.builder.push(Instr::Local(0x21, arr_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, len_tmp));
        self.builder.push(Instr::Local(0x20, len_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(0)));
        self.builder.push(Instr::Op(0x46));
        self.builder.push(Instr::Block {
            opcode: 0x04,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x04);
        self.emit_panic("pop from empty array");
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        // Element address = array + 4 + 8 * (len - 1).
        let addr_tmp = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, len_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6b));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, addr_tmp));
        match element {
            Ty::Int => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Mem(0x29, 3, 0));
            }
            Ty::Float => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Mem(0x2b, 3, 0));
            }
            Ty::Str => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Mem(0x28, 2, 4));
            }
            _ => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Mem(0x28, 2, 0));
            }
        }
        // Shrink: store len - 1 back into the array header.
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Local(0x20, len_tmp));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6b));
        self.builder.push(Instr::Mem(0x36, 2, 0));
        Ok(element)
    }

    /// `arr.push(value)`: allocate a larger block, copy the old one, append and
    /// write the new pointer back into the array variable.
    fn emit_push(&mut self, name: &str, value: &Expr) -> Result<(), String> {
        let arr_ty = match self.local_of(name) {
            Some(slot) => slot.ty,
            None => self.checker.binding(name)?.ty,
        };
        let element = match arr_ty {
            Ty::Arr => Ty::Int,
            Ty::ArrStr => Ty::Str,
            Ty::ArrObj(shape) => Ty::Obj(shape),
            Ty::ArrArr => Ty::Arr,
            Ty::ArrF => Ty::Float,
            Ty::ArrStrArr => Ty::ArrStr,
            other => {
                return Err(format!(
                    "wasm target pushes into arrays only, got {}",
                    ty_name(other)
                ))
            }
        };
        let arr_tmp = self.builder.add_local(ValType::I32);
        let new_tmp = self.builder.add_local(ValType::I32);
        let value_ptr = self.builder.add_local(ValType::I32);
        let value_len = self.builder.add_local(ValType::I32);
        let value_f = self.builder.add_local(ValType::F64);
        let value_i64 = self.builder.add_local(ValType::I64);
        match element {
            Ty::Int => {
                let actual = self.emit_expr(value)?;
                if actual != Ty::Int {
                    return Err("wasm target arrays hold ints only".to_string());
                }
                self.builder.push(Instr::Local(0x21, value_i64));
            }
            Ty::Str => {
                self.emit_str(value)?;
                self.builder.push(Instr::Local(0x21, value_len));
                self.builder.push(Instr::Local(0x21, value_ptr));
            }
            Ty::Obj(_) => {
                let actual = self.emit_expr(value)?;
                if !matches!(actual, Ty::Obj(_)) {
                    return Err("wasm target record arrays hold records only".to_string());
                }
                self.builder.push(Instr::Local(0x21, value_ptr));
            }
            Ty::Float => {
                let tmp = self.builder.add_local(ValType::F64);
                self.emit_f64(value)?;
                self.builder.push(Instr::Local(0x21, tmp));
                self.builder.push(Instr::Local(0x20, tmp));
                self.builder.push(Instr::Local(0x21, value_f));
            }
            Ty::ArrStr => {
                let actual = self.emit_expr(value)?;
                if actual != Ty::ArrStr {
                    return Err("wasm target string-row arrays hold string arrays only".to_string());
                }
                self.builder.push(Instr::Local(0x21, value_ptr));
            }
            _ => {
                let actual = self.emit_expr(value)?;
                if actual != Ty::Arr {
                    return Err("wasm target nested arrays hold int arrays only".to_string());
                }
                self.builder.push(Instr::Local(0x21, value_ptr));
            }
        }
        self.emit_load(name)?;
        self.builder.push(Instr::Local(0x21, arr_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(12)));
        self.builder.push(Instr::Op(0x6a));
        self.emit_alloc_dynamic();
        self.builder.push(Instr::Local(0x21, new_tmp));
        self.builder.push(Instr::Local(0x20, new_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::MemoryCopy);
        self.builder.push(Instr::Local(0x20, new_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Mem(0x36, 2, 0));
        // value slot address = new + 4 + 8*len
        let addr_tmp = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x20, new_tmp));
        self.builder.push(Instr::Local(0x20, arr_tmp));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, addr_tmp));
        match element {
            Ty::Int => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Local(0x20, value_i64));
                self.builder.push(Instr::Mem(0x37, 3, 0));
            }
            Ty::Str => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Local(0x20, value_ptr));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x20, value_len));
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
            Ty::Float => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Local(0x20, value_f));
                self.builder.push(Instr::Mem(0x39, 3, 0));
            }
            _ => {
                self.builder.push(Instr::Local(0x20, addr_tmp));
                self.builder.push(Instr::Local(0x20, value_ptr));
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
        }
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
            Expr::Neg(inner) => match self.type_of(inner)? {
                Ty::Int => Ok(Ty::Int),
                Ty::Float => Ok(Ty::Float),
                other => Err(format!(
                    "wasm target negates numbers only, got {}",
                    ty_name(other)
                )),
            },

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
                if op == &BinOp::Add && (lt == Ty::Str || rt == Ty::Str) {
                    let other = if lt == Ty::Str { rt } else { lt };
                    if matches!(other, Ty::Str | Ty::Int | Ty::Float | Ty::Bool) {
                        return Ok(Ty::Str);
                    }
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
            Expr::MethodCall(object, key, args, _) => {
                let base = self.type_of(object)?;
                if key == "index_of" && args.len() == 1 {
                    let _ = base;
                    return Ok(Ty::Int);
                }
                if matches!(key.as_str(), "contains" | "starts_with" | "ends_with") {
                    return Ok(Ty::Bool);
                }
                if key == "len" && args.is_empty() {
                    return match base {
                        Ty::Str
                        | Ty::Arr
                        | Ty::ArrStr
                        | Ty::ArrObj(_)
                        | Ty::ArrArr
                        | Ty::ArrF
                        | Ty::ArrStrArr
                        | Ty::Map => Ok(Ty::Int),
                        other => Err(format!("wasm target cannot take len of {}", ty_name(other))),
                    };
                }
                self.checker.infer(expr)
            }
            Expr::Call(callee, args, _) => {
                if let Expr::Ident(name) = &**callee {
                    if let Some(sig) = self.checker.fn_sig(name) {
                        return sig.result.ok_or_else(|| {
                            format!("wasm target function '{}' returns no value", name)
                        });
                    }
                    if let Ok(Ty::Func(id)) = self.type_of(callee) {
                        return self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .and_then(|sig| sig.result)
                            .ok_or_else(|| {
                                "wasm target function literal returns no value".to_string()
                            });
                    }
                    if matches!(name.as_str(), "min" | "max") && args.len() == 1 {
                        return match self.type_of(&args[0])? {
                            Ty::ArrStr => Ok(Ty::Str),
                            _ => Ok(Ty::Int),
                        };
                    }
                    if name == "abs" && args.len() == 1 {
                        return self.type_of(&args[0]);
                    }
                    let known = match name.as_str() {
                        "float" => Some(Ty::Float),
                        "floor" | "ceil" | "round" | "sqrt" => Some(Ty::Float),
                        "trim" | "upper" | "lower" | "replace" | "repeat" => Some(Ty::Str),
                        "index_of" => Some(Ty::Int),
                        "type" => Some(Ty::Str),
                        "str" => Some(Ty::Str),
                        "int" => Some(Ty::Int),
                        "len" => Some(Ty::Int),
                        "argc" | "run" | "capture" | "write_file" => Some(Ty::Int),
                        "arg" | "read_file" => Some(Ty::Str),
                        "map_new" => Some(Ty::Map),
                        "map_get" | "map_has" | "map_len" | "file_exists" => Some(Ty::Int),
                        "exists" => Some(Ty::Bool),
                        "now" | "mkdir" => Some(Ty::Int),
                        "cwd" | "env" => Some(Ty::Str),
                        "performance_now" => Some(Ty::Float),
                        "gl_create" | "gl_pixel" | "gl_png" => Some(Ty::Int),
                        _ => None,
                    };
                    if let Some(ty) = known {
                        for arg in args {
                            self.type_of(arg)?;
                        }
                        return Ok(ty);
                    }
                }
                if let Some((_, result, _)) = self.callable(callee)? {
                    return result.ok_or_else(|| {
                        "wasm target function literal returns no value".to_string()
                    });
                }
                self.checker.infer(expr)
            }

            Expr::Index(array, _, _) => match self.type_of(array)? {
                Ty::ArrStr | Ty::Str => Ok(Ty::Str),
                Ty::ArrObj(shape) => Ok(Ty::Obj(shape)),
                Ty::ArrArr => Ok(Ty::Arr),
                Ty::ArrStrArr => Ok(Ty::ArrStr),
                Ty::ArrF => Ok(Ty::Float),
                _ => Ok(Ty::Int),
            },
            Expr::Array(items) => {
                if items.is_empty() {
                    return Ok(Ty::Arr);
                }
                let types: Vec<Ty> = items
                    .iter()
                    .map(|item| self.type_of(item))
                    .collect::<Result<_, _>>()?;
                if types.iter().all(|ty| *ty == Ty::ArrStr) {
                    return Ok(Ty::ArrStrArr);
                }
                if types.contains(&Ty::Str) {
                    return Ok(Ty::ArrStr);
                }
                if types.iter().all(|ty| *ty == Ty::Arr) {
                    return Ok(Ty::ArrArr);
                }
                if let Ty::Obj(shape) = types[0] {
                    if types.iter().all(|ty| *ty == Ty::Obj(shape)) {
                        return Ok(Ty::ArrObj(shape));
                    }
                }
                if types.contains(&Ty::Float) {
                    return Ok(Ty::ArrF);
                }
                Ok(Ty::Arr)
            }
            Expr::Object(fields) => {
                let mut keys: Vec<String> = Vec::new();
                let mut kinds: Vec<u8> = Vec::new();
                for (key, value) in fields {
                    keys.push(key.clone());
                    kinds.push(field_kind(self.type_of(value)?).unwrap_or(0));
                }
                Ok(Ty::Obj(self.checker.shape_id(&keys, &kinds)))
            }
            Expr::Member(object, key, _) => {
                let Ty::Obj(shape) = self.type_of(object)? else {
                    return Err("wasm target reads record fields only".to_string());
                };
                let index = self.checker.field_index(shape, key)?;
                Ok(kind_ty(self.checker.shape_kinds(shape)?[index]))
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
                let divisor = self.builder.add_local(ValType::F64);
                self.emit_f64(right)?;
                self.builder.push(Instr::Local(0x21, divisor));
                self.builder.push(Instr::Local(0x20, divisor));
                self.builder.push(Instr::Const(0x44, WVal::F64(0.0)));
                self.builder.push(Instr::Op(0x61));
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.emit_panic("division by zero");
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.emit_f64(left)?;
                self.builder.push(Instr::Local(0x20, divisor));
                self.builder.push(Instr::Op(0xa3));
                Ok(Ty::Float)
            }
            BinOp::Mod if !both_int => {
                let divisor = self.builder.add_local(ValType::F64);
                self.emit_f64(right)?;
                self.builder.push(Instr::Local(0x21, divisor));
                self.builder.push(Instr::Local(0x20, divisor));
                self.builder.push(Instr::Const(0x44, WVal::F64(0.0)));
                self.builder.push(Instr::Op(0x61));
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.emit_panic("modulo by zero");
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.emit_f64(left)?;
                self.builder.push(Instr::Local(0x20, divisor));
                self.builder.push(Instr::Call(9));
                Ok(Ty::Float)
            }
            BinOp::Mod => {
                let divisor = self.builder.add_local(ValType::I64);
                self.emit_expr(right)?;
                self.builder.push(Instr::Local(0x21, divisor));
                self.builder.push(Instr::Local(0x20, divisor));
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.builder.push(Instr::Op(0x51));
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.emit_panic("modulo by zero");
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.emit_expr(left)?;
                self.builder.push(Instr::Local(0x20, divisor));
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
                self.emit_cmp(*op, left, right)
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

    /// Store `expr` into a fresh 16-byte cell in slot format and return a local
    /// holding its address (used by structural equality and array search).
    fn emit_value_cell(&mut self, expr: &Expr, ty: Ty) -> Result<u32, String> {
        let cell = self.builder.add_local(ValType::I32);
        self.emit_alloc_const(16);
        self.builder.push(Instr::Local(0x21, cell));
        match ty {
            Ty::Int => {
                self.builder.push(Instr::Local(0x20, cell));
                self.emit_expr(expr)?;
                self.builder.push(Instr::Mem(0x37, 3, 0));
            }
            Ty::Float => {
                self.builder.push(Instr::Local(0x20, cell));
                self.emit_f64(expr)?;
                self.builder.push(Instr::Mem(0x39, 3, 0));
            }
            Ty::Bool => {
                self.builder.push(Instr::Local(0x20, cell));
                self.emit_expr(expr)?;
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
            Ty::Str => {
                self.emit_str(expr)?;
                self.builder.push(Instr::Local(0x21, self.temp(LEN_A)));
                self.builder.push(Instr::Local(0x21, self.temp(PTR_A)));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Local(0x20, self.temp(PTR_A)));
                self.builder.push(Instr::Mem(0x36, 2, 0));
                self.builder.push(Instr::Local(0x20, cell));
                self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                self.builder.push(Instr::Mem(0x36, 2, 4));
            }
            _ => {
                self.builder.push(Instr::Local(0x20, cell));
                self.emit_expr(expr)?;
                self.builder.push(Instr::Mem(0x36, 2, 0));
            }
        }
        Ok(cell)
    }

    /// Compare the values stored at two slot addresses, leaving an i32 bool.
    fn emit_eq_at(&mut self, ty: Ty, a: u32, b: u32) -> Result<(), String> {
        match ty {
            Ty::Int => {
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x29, 3, 0));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x29, 3, 0));
                self.builder.push(Instr::Op(0x51));
            }
            Ty::Float => {
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x2b, 3, 0));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x2b, 3, 0));
                self.builder.push(Instr::Op(0x61));
            }
            Ty::Bool => {
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Op(0x46));
            }
            Ty::Str => {
                for (slot, offset) in [(a, 0u32), (a, 4), (b, 0), (b, 4)] {
                    self.builder.push(Instr::Local(0x20, slot));
                    self.builder.push(Instr::Mem(0x28, 2, offset));
                }
                self.builder.push(Instr::Call(STR_COMPARE));
                self.builder.push(Instr::Op(0x45));
            }
            Ty::Obj(shape) => {
                let rec_a = self.builder.add_local(ValType::I32);
                let rec_b = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x21, rec_a));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x21, rec_b));
                let kinds = self.checker.shape_kinds(shape)?;
                let acc = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                self.builder.push(Instr::Local(0x21, acc));
                for (index, kind) in kinds.iter().enumerate() {
                    let field_ty = kind_ty(*kind);
                    let fa = self.builder.add_local(ValType::I32);
                    let fb = self.builder.add_local(ValType::I32);
                    for (slot, rec) in [(fa, rec_a), (fb, rec_b)] {
                        self.builder.push(Instr::Local(0x20, rec));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32((8 * index) as i32)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x21, slot));
                    }
                    self.emit_eq_at(field_ty, fa, fb)?;
                    self.builder.push(Instr::Local(0x21, self.temp(DST)));
                    self.builder.push(Instr::Local(0x20, acc));
                    self.builder.push(Instr::Local(0x20, self.temp(DST)));
                    self.builder.push(Instr::Op(0x71));
                    self.builder.push(Instr::Local(0x21, acc));
                }
                self.builder.push(Instr::Local(0x20, acc));
            }
            Ty::Arr | Ty::ArrF | Ty::ArrStr | Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrStrArr => {
                let elem = element_ty(ty).ok_or_else(|| "not an array".to_string())?;
                let head_a = self.builder.add_local(ValType::I32);
                let head_b = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x20, a));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x21, head_a));
                self.builder.push(Instr::Local(0x20, b));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x21, head_b));
                let same_len = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x20, head_a));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Local(0x20, head_b));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Op(0x46));
                self.builder.push(Instr::Local(0x21, same_len));
                let index = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                self.builder.push(Instr::Local(0x21, index));
                let acc = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x20, same_len));
                self.builder.push(Instr::Local(0x21, acc));
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
                self.builder.push(Instr::Local(0x20, index));
                self.builder.push(Instr::Local(0x20, head_a));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Op(0x46));
                self.builder.push(Instr::BrIf(1));
                let ea = self.builder.add_local(ValType::I32);
                let eb = self.builder.add_local(ValType::I32);
                for (slot, head) in [(ea, head_a), (eb, head_b)] {
                    self.builder.push(Instr::Local(0x20, head));
                    self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                    self.builder.push(Instr::Op(0x6a));
                    self.builder.push(Instr::Local(0x20, index));
                    self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                    self.builder.push(Instr::Op(0x6c));
                    self.builder.push(Instr::Op(0x6a));
                    self.builder.push(Instr::Local(0x21, slot));
                }
                self.emit_eq_at(elem, ea, eb)?;
                self.builder.push(Instr::Local(0x21, self.temp(DST)));
                self.builder.push(Instr::Local(0x20, acc));
                self.builder.push(Instr::Local(0x20, self.temp(DST)));
                self.builder.push(Instr::Op(0x71));
                self.builder.push(Instr::Local(0x21, acc));
                self.builder.push(Instr::Local(0x20, index));
                self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x21, index));
                self.builder.push(Instr::Br(0));
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.builder.push(Instr::Local(0x20, acc));
            }
            _ => return Err("wasm target cannot compare these values".to_string()),
        }
        Ok(())
    }

    /// Linear search of `value` in an array of `elem` kind. With `want_index`
    /// the index (or -1) is pushed; otherwise a bool is pushed.
    fn emit_array_search(
        &mut self,
        array: &Expr,
        value: &Expr,
        elem: Ty,
        want_index: bool,
    ) -> Result<(), String> {
        let head = self.builder.add_local(ValType::I32);
        self.emit_expr(array)?;
        self.builder.push(Instr::Local(0x21, head));
        let cell = self.emit_value_cell(value, elem)?;
        let len = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x20, head));
        self.builder.push(Instr::Mem(0x28, 2, 0));
        self.builder.push(Instr::Local(0x21, len));
        let index = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Const(0x41, WVal::I32(0)));
        self.builder.push(Instr::Local(0x21, index));
        let result = self.builder.add_local(ValType::I64);
        self.builder.push(Instr::Const(0x42, WVal::I64(-1)));
        self.builder.push(Instr::Local(0x21, result));
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
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Local(0x20, len));
        self.builder.push(Instr::Op(0x4f));
        self.builder.push(Instr::BrIf(1));
        let ea = self.builder.add_local(ValType::I32);
        self.builder.push(Instr::Local(0x20, head));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
        self.builder.push(Instr::Op(0x6c));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, ea));
        self.builder.push(Instr::Local(0x20, result));
        self.builder.push(Instr::Const(0x42, WVal::I64(-1)));
        self.builder.push(Instr::Op(0x51));
        self.emit_eq_at(elem, ea, cell)?;
        self.builder.push(Instr::Op(0x71));
        self.builder.push(Instr::Block {
            opcode: 0x04,
            end: 0,
            else_at: 0,
            start: 0,
        });
        self.labels.push(0x04);
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Op(0xad));
        self.builder.push(Instr::Local(0x21, result));
        self.builder.push(Instr::Br(2));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.builder.push(Instr::Local(0x20, index));
        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x21, index));
        self.builder.push(Instr::Br(0));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.labels.pop();
        self.builder.push(Instr::Op(0x0b));
        self.builder.push(Instr::Local(0x20, result));
        if !want_index {
            self.builder.push(Instr::Const(0x42, WVal::I64(0)));
            self.builder.push(Instr::Op(0x59));
        }
        Ok(())
    }

    fn emit_value_eq(&mut self, left: &Expr, right: &Expr, ty: Ty) -> Result<(), String> {
        let a = self.emit_value_cell(left, ty)?;
        let b = self.emit_value_cell(right, ty)?;
        self.emit_eq_at(ty, a, b)
    }
    /// Emit a comparison, leaving an i32 bool on the stack.
    fn emit_cmp(&mut self, op: BinOp, left: &Expr, right: &Expr) -> Result<(), String> {
        let lt = self.type_of(left)?;
        let rt = self.type_of(right)?;
        if lt == rt
            && matches!(
                lt,
                Ty::Obj(_)
                    | Ty::Arr
                    | Ty::ArrF
                    | Ty::ArrStr
                    | Ty::ArrObj(_)
                    | Ty::ArrArr
                    | Ty::ArrStrArr
            )
            && matches!(op, BinOp::Eq | BinOp::Ne)
        {
            self.emit_value_eq(left, right, lt)?;
            if op == BinOp::Ne {
                self.builder.push(Instr::Op(0x45));
            }
            return Ok(());
        }
        if lt == Ty::Bool && rt == Ty::Bool {
            self.emit_expr(left)?;
            self.emit_expr(right)?;
            let opcode = match op {
                BinOp::Eq => 0x46,
                BinOp::Ne => 0x47,
                BinOp::Lt => 0x48,
                BinOp::Gt => 0x4a,
                BinOp::Le => 0x4c,
                _ => 0x4e,
            };
            self.builder.push(Instr::Op(opcode));
            return Ok(());
        }
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
    /// Emit a loop-exit test: branches out when `cond` is false. Direct
    /// comparisons use their inverse opcode so the extra `eqz` disappears.
    fn emit_cond_exit(&mut self, cond: &Expr) -> Result<(), String> {
        if let Expr::Bin(op, left, right) = cond {
            if matches!(
                op,
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
            ) {
                let inverse = match op {
                    BinOp::Eq => BinOp::Ne,
                    BinOp::Ne => BinOp::Eq,
                    BinOp::Lt => BinOp::Ge,
                    BinOp::Le => BinOp::Gt,
                    BinOp::Gt => BinOp::Le,
                    _ => BinOp::Lt,
                };
                self.emit_cmp(inverse, left, right)?;
                self.builder.push(Instr::BrIf(1));
                return Ok(());
            }
        }
        self.emit_cond(cond)?;
        self.builder.push(Instr::Op(0x45));
        self.builder.push(Instr::BrIf(1));
        Ok(())
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
            self.emit_cond_exit(cond)?;
        }
        if step.is_some() {
            self.push_continue_block();
        } else {
            self.continue_targets.push(self.labels.len() - 1);
        }
        for stmt in body {
            self.emit_stmt(stmt)?;
        }
        if step.is_some() {
            self.pop_continue_block();
        } else {
            self.continue_targets.pop();
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
    /// Emit an expression as a `(ptr, len)` string pair; numbers and bools are
    /// stringified first (used by interpolation and mixed `+` concatenation).
    fn emit_str_operand(&mut self, expr: &Expr) -> Result<(), String> {
        match self.type_of(expr)? {
            Ty::Str => self.emit_str(expr),
            Ty::Int => {
                self.emit_expr(expr)?;
                self.builder.push(Instr::Call(INT_TO_STR));
                self.push_block_as_pair();
                Ok(())
            }
            Ty::Float => {
                self.emit_f64(expr)?;
                self.builder.push(Instr::Call(F64_TO_STR));
                self.push_block_as_pair();
                Ok(())
            }
            Ty::Bool => {
                self.emit_expr(expr)?;
                self.builder.push(Instr::Call(BOOL_TO_STR));
                self.push_block_as_pair();
                Ok(())
            }
            other => Err(format!(
                "wasm target cannot use {} in a string concatenation",
                ty_name(other)
            )),
        }
    }

    /// Turn a `[len][utf8]` block pointer on the stack into a `(ptr, len)` pair.
    fn push_block_as_pair(&mut self) {
        self.builder.push(Instr::Local(0x21, ARG_TMP));
        self.builder.push(Instr::Local(0x20, ARG_TMP));
        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
        self.builder.push(Instr::Op(0x6a));
        self.builder.push(Instr::Local(0x20, ARG_TMP));
        self.builder.push(Instr::Mem(0x28, 2, 0));
    }
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
                // Own locals: a string operand such as a record field clobbers
                // the shared PTR_A/LEN_A temporaries, so the left side must not
                // live there while the right side is evaluated.
                let len_a = self.builder.add_local(ValType::I32);
                let ptr_a = self.builder.add_local(ValType::I32);
                let len_b = self.builder.add_local(ValType::I32);
                let ptr_b = self.builder.add_local(ValType::I32);
                self.emit_str_operand(left)?;
                self.builder.push(Instr::Local(0x21, len_a));
                self.builder.push(Instr::Local(0x21, ptr_a));
                self.emit_str_operand(right)?;
                self.builder.push(Instr::Local(0x21, len_b));
                self.builder.push(Instr::Local(0x21, ptr_b));
                self.builder.push(Instr::Local(0x20, len_a));
                self.builder.push(Instr::Local(0x20, len_b));
                self.builder.push(Instr::Op(0x6a));
                self.emit_alloc_dynamic();
                let dst = self.builder.add_local(ValType::I32);
                self.builder.push(Instr::Local(0x21, dst));
                self.builder.push(Instr::Local(0x20, dst));
                self.builder.push(Instr::Local(0x20, ptr_a));
                self.builder.push(Instr::Local(0x20, len_a));
                self.builder.push(Instr::MemoryCopy);
                self.builder.push(Instr::Local(0x20, dst));
                self.builder.push(Instr::Local(0x20, len_a));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Local(0x20, ptr_b));
                self.builder.push(Instr::Local(0x20, len_b));
                self.builder.push(Instr::MemoryCopy);
                self.builder.push(Instr::Local(0x20, dst));
                self.builder.push(Instr::Local(0x20, len_a));
                self.builder.push(Instr::Local(0x20, len_b));
                self.builder.push(Instr::Op(0x6a));
                Ok(())
            }
            other => match self.type_of(other)? {
                Ty::Str => {
                    self.emit_expr(other)?;
                    Ok(())
                }
                Ty::Int | Ty::Float | Ty::Bool => self.emit_str_operand(other),
                _ => Err(format!(
                    "wasm target does not support this string expression yet: {}",
                    expr_kind(other)
                )),
            },
        }
    }

    /// Load one container slot of `kind` (strings push ptr then len).
    fn emit_load_slot(emitter: &mut Emitter, arr: u32, idx: u32, kind: Ty) {
        emitter.builder.push(Instr::Local(0x20, arr));
        emitter.builder.push(Instr::Local(0x20, idx));
        emitter.builder.push(Instr::Op(0xa7));
        emitter.builder.push(Instr::Const(0x41, WVal::I32(8)));
        emitter.builder.push(Instr::Op(0x6c));
        emitter.builder.push(Instr::Const(0x41, WVal::I32(4)));
        emitter.builder.push(Instr::Op(0x6a));
        emitter.builder.push(Instr::Op(0x6a));
        match kind {
            Ty::Int => emitter.builder.push(Instr::Mem(0x29, 3, 0)),
            Ty::Float => emitter.builder.push(Instr::Mem(0x2b, 3, 0)),
            Ty::Obj(_) | Ty::Arr | Ty::ArrStr => emitter.builder.push(Instr::Mem(0x28, 2, 0)),
            Ty::Str => {
                emitter.builder.push(Instr::Mem(0x28, 2, 0));
                emitter.builder.push(Instr::Local(0x20, arr));
                emitter.builder.push(Instr::Local(0x20, idx));
                emitter.builder.push(Instr::Op(0xa7));
                emitter.builder.push(Instr::Const(0x41, WVal::I32(8)));
                emitter.builder.push(Instr::Op(0x6c));
                emitter.builder.push(Instr::Const(0x41, WVal::I32(8)));
                emitter.builder.push(Instr::Op(0x6a));
                emitter.builder.push(Instr::Op(0x6a));
                emitter.builder.push(Instr::Mem(0x28, 2, 0));
            }
            _ => {}
        }
    }

    /// Push a reduce/sort accumulator of `kind` from its local slots.
    fn emit_push_acc(&mut self, kind: Ty, i64: u32, f64: u32, ptr: u32, len: u32) {
        match kind {
            Ty::Int => self.builder.push(Instr::Local(0x20, i64)),
            Ty::Float => self.builder.push(Instr::Local(0x20, f64)),
            Ty::Str => {
                self.builder.push(Instr::Local(0x20, ptr));
                self.builder.push(Instr::Local(0x20, len));
            }
            _ => {}
        }
    }

    /// Pop the stack into a reduce/sort accumulator of `kind`.
    fn emit_store_acc(&mut self, kind: Ty, i64: u32, f64: u32, ptr: u32, len: u32) {
        match kind {
            Ty::Int => self.builder.push(Instr::Local(0x21, i64)),
            Ty::Float => self.builder.push(Instr::Local(0x21, f64)),
            Ty::Str => {
                self.builder.push(Instr::Local(0x21, len));
                self.builder.push(Instr::Local(0x21, ptr));
            }
            _ => {}
        }
    }
    /// Store the value already on the operand stack into `addr` as `kind`.
    #[allow(clippy::too_many_arguments)]
    fn emit_store_slot(
        emitter: &mut Emitter,
        addr: u32,
        kind: Ty,
        val_i64: u32,
        val_f64: u32,
        val_i32: u32,
        ptr_tmp: u32,
        len_tmp: u32,
    ) {
        let b = &mut emitter.builder;
        match kind {
            Ty::Int => {
                b.push(Instr::Local(0x21, val_i64));
                b.push(Instr::Local(0x20, addr));
                b.push(Instr::Local(0x20, val_i64));
                b.push(Instr::Mem(0x37, 3, 0));
            }
            Ty::Float => {
                b.push(Instr::Local(0x21, val_f64));
                b.push(Instr::Local(0x20, addr));
                b.push(Instr::Local(0x20, val_f64));
                b.push(Instr::Mem(0x39, 3, 0));
            }
            Ty::Obj(_) | Ty::Arr | Ty::ArrStr => {
                b.push(Instr::Local(0x21, val_i32));
                b.push(Instr::Local(0x20, addr));
                b.push(Instr::Local(0x20, val_i32));
                b.push(Instr::Mem(0x36, 2, 0));
            }
            Ty::Str => {
                b.push(Instr::Local(0x21, len_tmp));
                b.push(Instr::Local(0x21, ptr_tmp));
                b.push(Instr::Local(0x20, addr));
                b.push(Instr::Local(0x20, ptr_tmp));
                b.push(Instr::Mem(0x36, 2, 0));
                b.push(Instr::Local(0x20, addr));
                b.push(Instr::Const(0x41, WVal::I32(4)));
                b.push(Instr::Op(0x6a));
                b.push(Instr::Local(0x20, len_tmp));
                b.push(Instr::Mem(0x36, 2, 0));
            }
            _ => {}
        }
    }

    fn array_of(kind: Ty) -> Ty {
        match kind {
            Ty::Int => Ty::Arr,
            Ty::Float => Ty::ArrF,
            Ty::Str => Ty::ArrStr,
            Ty::Obj(shape) => Ty::ArrObj(shape),
            Ty::Arr => Ty::ArrArr,
            Ty::ArrStr => Ty::ArrStrArr,
            other => other,
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
                let captures = self
                    .analysis
                    .lit_captures
                    .get(id as usize)
                    .cloned()
                    .unwrap_or_default();
                if captures.is_empty() {
                    self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                    return Ok(Ty::Func(id));
                }
                let env_tmp = self.builder.add_local(ValType::I32);
                self.emit_alloc_const(4 * captures.len() as u32);
                self.builder.push(Instr::Local(0x21, env_tmp));
                for (index, capture) in captures.iter().enumerate() {
                    self.builder.push(Instr::Local(0x20, env_tmp));
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32((4 * index) as i32)));
                    self.builder.push(Instr::Op(0x6a));
                    match capture.source {
                        CaptureSource::Local => {
                            let slot = self.local_of(&capture.name).ok_or_else(|| {
                                format!("wasm target lost the captured binding '{}'", capture.name)
                            })?;
                            if !slot.boxed {
                                return Err(format!(
                                    "wasm target captured '{}' without a cell",
                                    capture.name
                                ));
                            }
                            self.builder.push(Instr::Local(0x20, slot.slot));
                        }
                        CaptureSource::Env(offset) => {
                            let env = self.env_slot.ok_or_else(|| {
                                "wasm target lost a literal environment".to_string()
                            })?;
                            self.builder.push(Instr::Local(0x20, env));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32((4 * offset) as i32)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                        }
                    }
                    self.builder.push(Instr::Mem(0x36, 2, 0));
                }
                self.builder.push(Instr::Local(0x20, env_tmp));
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
                    Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrF | Ty::ArrStrArr => {
                        let base = self.checker.binding(name)?.base;
                        self.builder.push(Instr::Global(0x23, base));
                    }
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
                    | Ty::Bool
                    | Ty::ArrStr => self.push_global(name, false)?,
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
                let ty = self.type_of(expr)?;
                if ty == Ty::Str {
                    let ptr_slot = self.builder.add_local(ValType::I32);
                    let len_slot = self.builder.add_local(ValType::I32);
                    self.emit_cond(cond)?;
                    self.builder.push(Instr::Block {
                        opcode: 0x04,
                        end: 0,
                        else_at: 0,
                        start: 0,
                    });
                    self.labels.push(0x04);
                    self.emit_str(then)?;
                    self.builder.push(Instr::Local(0x21, len_slot));
                    self.builder.push(Instr::Local(0x21, ptr_slot));
                    self.builder.push(Instr::Else { end: 0 });
                    self.emit_str(other)?;
                    self.builder.push(Instr::Local(0x21, len_slot));
                    self.builder.push(Instr::Local(0x21, ptr_slot));
                    self.labels.pop();
                    self.builder.push(Instr::Op(0x0b));
                    self.builder.push(Instr::Local(0x20, ptr_slot));
                    self.builder.push(Instr::Local(0x20, len_slot));
                    return Ok(Ty::Str);
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
                if self.type_of(inner)? == Ty::Float {
                    self.emit_f64(inner)?;
                    self.builder.push(Instr::Op(0x9a));
                    return Ok(Ty::Float);
                }
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.emit_expr(inner)?;
                self.builder.push(Instr::Op(0x7d));
                Ok(Ty::Int)
            }
            Expr::Bin(BinOp::Add, left, right) => {
                if self.type_of(left)? == Ty::Str || self.type_of(right)? == Ty::Str {
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
                let mut kinds: Vec<u8> = Vec::new();
                for (key, value) in fields {
                    let field_ty = self.type_of(value)?;
                    let kind = field_kind(field_ty).ok_or_else(|| {
                        format!(
                            "wasm target record fields hold int, string, float or bool values, got {}",
                            ty_name(field_ty)
                        )
                    })?;
                    keys.push(key.clone());
                    kinds.push(kind);
                }
                let shape = Ty::Obj(self.checker.shape_id(&keys, &kinds));
                let size = 8 * fields.len() as u32;
                self.emit_alloc_const(size);
                self.builder.push(Instr::Local(0x21, self.temp(ARR_TMP)));
                for (index, ((_, value), kind)) in fields.iter().zip(kinds.iter()).enumerate() {
                    let offset = (8 * index) as u32;
                    match kind {
                        1 => {
                            self.emit_str(value)?;
                            self.builder.push(Instr::Local(0x21, self.temp(LEN_A)));
                            self.builder.push(Instr::Local(0x21, self.temp(PTR_A)));
                            self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                            self.builder.push(Instr::Local(0x20, self.temp(PTR_A)));
                            self.builder.push(Instr::Mem(0x36, 2, offset));
                            self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                            self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                            self.builder.push(Instr::Mem(0x36, 2, offset + 4));
                        }
                        2 => {
                            self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                            self.emit_f64(value)?;
                            self.builder.push(Instr::Mem(0x39, 3, offset));
                        }
                        3 => {
                            self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                            self.emit_expr(value)?;
                            self.builder.push(Instr::Op(0xad));
                            self.builder.push(Instr::Mem(0x37, 3, offset));
                        }
                        _ => {
                            self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                            self.emit_expr(value)?;
                            self.builder.push(Instr::Mem(0x37, 3, offset));
                        }
                    }
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
                let kind = self.checker.shape_kinds(shape)?[index];
                self.emit_expr(object)?;
                let offset = (8 * index) as u32;
                match kind {
                    1 => {
                        let rec_tmp = self.builder.add_local(ValType::I32);
                        self.builder.push(Instr::Local(0x21, rec_tmp));
                        self.builder.push(Instr::Local(0x20, rec_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, offset));
                        self.builder.push(Instr::Local(0x21, self.temp(PTR_A)));
                        self.builder.push(Instr::Local(0x20, rec_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, offset + 4));
                        self.builder.push(Instr::Local(0x21, self.temp(LEN_A)));
                        self.builder.push(Instr::Local(0x20, self.temp(PTR_A)));
                        self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                        Ok(Ty::Str)
                    }
                    2 => {
                        self.builder.push(Instr::Mem(0x2b, 3, offset));
                        Ok(Ty::Float)
                    }
                    3 => {
                        self.builder.push(Instr::Mem(0x28, 2, offset));
                        Ok(Ty::Bool)
                    }
                    _ => {
                        self.builder.push(Instr::Mem(0x29, 3, offset));
                        Ok(Ty::Int)
                    }
                }
            }
            Expr::Array(items) => {
                let count = items.len() as u32;
                if !items.is_empty() && self.type_of(&items[0])? == Ty::Str {
                    let size = 4 + 8 * count;
                    self.emit_alloc_const(size);
                    self.builder.push(Instr::Local(0x21, self.temp(ARR_TMP)));
                    self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                    self.builder
                        .push(Instr::Const(0x41, WVal::I32(count as i32)));
                    self.builder.push(Instr::Mem(0x36, 2, 0));
                    let ptr_tmp = self.builder.add_local(ValType::I32);
                    let len_tmp = self.builder.add_local(ValType::I32);
                    for (index, item) in items.iter().enumerate() {
                        let offset = 4 + 8 * index as u32;
                        self.emit_str(item)?;
                        self.builder.push(Instr::Local(0x21, len_tmp));
                        self.builder.push(Instr::Local(0x21, ptr_tmp));
                        self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(offset as i32)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, ptr_tmp));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                        self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32((offset + 4) as i32)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, len_tmp));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                    }
                    self.builder.push(Instr::Local(0x20, self.temp(ARR_TMP)));
                    return Ok(Ty::ArrStr);
                }
                if !items.is_empty() {
                    let numeric_float = {
                        let mut any_float = false;
                        let mut all_numeric = true;
                        for item in items {
                            match self.type_of(item)? {
                                Ty::Float => any_float = true,
                                Ty::Int => {}
                                _ => all_numeric = false,
                            }
                        }
                        all_numeric && any_float
                    };
                    if numeric_float {
                        let arr_local = self.builder.add_local(ValType::I32);
                        let size = 4 + 8 * count;
                        self.emit_alloc_const(size);
                        self.builder.push(Instr::Local(0x21, arr_local));
                        self.builder.push(Instr::Local(0x20, arr_local));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                        for (index, item) in items.iter().enumerate() {
                            self.builder.push(Instr::Local(0x20, arr_local));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32((4 + 8 * index as u32) as i32)));
                            self.builder.push(Instr::Op(0x6a));
                            self.emit_f64(item)?;
                            self.builder.push(Instr::Mem(0x39, 3, 0));
                        }
                        self.builder.push(Instr::Local(0x20, arr_local));
                        return Ok(Ty::ArrF);
                    }
                    let text_rows = items.iter().any(|item| {
                        self.type_of(item)
                            .map(|ty| ty == Ty::ArrStr)
                            .unwrap_or(false)
                    });
                    if text_rows {
                        let arr_local = self.builder.add_local(ValType::I32);
                        let size = 4 + 8 * count;
                        self.emit_alloc_const(size);
                        self.builder.push(Instr::Local(0x21, arr_local));
                        self.builder.push(Instr::Local(0x20, arr_local));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                        for (index, item) in items.iter().enumerate() {
                            self.builder.push(Instr::Local(0x20, arr_local));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32((4 + 8 * index as u32) as i32)));
                            self.builder.push(Instr::Op(0x6a));
                            self.emit_expr(item)?;
                            self.builder.push(Instr::Mem(0x36, 2, 0));
                        }
                        self.builder.push(Instr::Local(0x20, arr_local));
                        return Ok(Ty::ArrStrArr);
                    }
                    if self.type_of(&items[0])? == Ty::Arr {
                        let arr_local = self.builder.add_local(ValType::I32);
                        let size = 4 + 8 * count;
                        self.emit_alloc_const(size);
                        self.builder.push(Instr::Local(0x21, arr_local));
                        self.builder.push(Instr::Local(0x20, arr_local));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                        for (index, item) in items.iter().enumerate() {
                            self.builder.push(Instr::Local(0x20, arr_local));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32((4 + 8 * index as u32) as i32)));
                            self.builder.push(Instr::Op(0x6a));
                            self.emit_expr(item)?;
                            self.builder.push(Instr::Mem(0x36, 2, 0));
                        }
                        self.builder.push(Instr::Local(0x20, arr_local));
                        return Ok(Ty::ArrArr);
                    }
                    if let Ty::Obj(shape) = self.type_of(&items[0])? {
                        // Records allocate their own storage while being emitted, so
                        // keep the array pointer in a dedicated local (the shared
                        // ARR_TMP temp would be clobbered by each record literal).
                        let arr_local = self.builder.add_local(ValType::I32);
                        let size = 4 + 8 * count;
                        self.emit_alloc_const(size);
                        self.builder.push(Instr::Local(0x21, arr_local));
                        self.builder.push(Instr::Local(0x20, arr_local));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                        for (index, item) in items.iter().enumerate() {
                            self.builder.push(Instr::Local(0x20, arr_local));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32((4 + 8 * index as u32) as i32)));
                            self.builder.push(Instr::Op(0x6a));
                            self.emit_expr(item)?;
                            self.builder.push(Instr::Mem(0x36, 2, 0));
                        }
                        self.builder.push(Instr::Local(0x20, arr_local));
                        return Ok(Ty::ArrObj(shape));
                    }
                }
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
                let array_ty = self.type_of(array)?;
                if array_ty == Ty::Str {
                    let ptr_tmp = self.builder.add_local(ValType::I32);
                    let len_tmp = self.builder.add_local(ValType::I32);
                    let index_tmp = self.builder.add_local(ValType::I64);
                    self.emit_str(array)?;
                    self.builder.push(Instr::Local(0x21, len_tmp));
                    self.builder.push(Instr::Local(0x21, ptr_tmp));
                    if self.type_of(index)? == Ty::Float {
                        self.emit_f64(index)?;
                        self.builder.push(Instr::Op(0xb0));
                    } else {
                        self.emit_expr(index)?;
                    }
                    self.builder.push(Instr::Local(0x21, index_tmp));
                    self.builder.push(Instr::Local(0x20, ptr_tmp));
                    self.builder.push(Instr::Local(0x20, len_tmp));
                    self.builder.push(Instr::Local(0x20, index_tmp));
                    self.builder.push(Instr::Op(0xa7));
                    self.builder.push(Instr::Call(STR_CHAR_AT));
                    self.builder.push(Instr::Local(0x21, ptr_tmp));
                    self.builder.push(Instr::Local(0x20, ptr_tmp));
                    self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                    self.builder.push(Instr::Op(0x6a));
                    self.builder.push(Instr::Local(0x20, ptr_tmp));
                    self.builder.push(Instr::Mem(0x28, 2, 0));
                    return Ok(Ty::Str);
                }
                if array_ty == Ty::ArrStr {
                    let array_tmp = self.builder.add_local(ValType::I32);
                    let index_tmp = self.builder.add_local(ValType::I64);
                    self.emit_expr(array)?;
                    self.builder.push(Instr::Local(0x21, array_tmp));
                    self.emit_expr(index)?;
                    self.builder.push(Instr::Local(0x21, index_tmp));
                    self.builder.push(Instr::Local(0x20, index_tmp));
                    self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                    self.builder.push(Instr::Op(0x53));
                    self.builder.push(Instr::Block {
                        opcode: 0x04,
                        end: 0,
                        else_at: 0,
                        start: 0,
                    });
                    self.labels.push(0x04);
                    self.builder.push(Instr::Local(0x20, index_tmp));
                    self.builder.push(Instr::Local(0x20, array_tmp));
                    self.builder.push(Instr::Mem(0x28, 2, 0));
                    self.builder.push(Instr::Op(0xad));
                    self.builder.push(Instr::Op(0x7c));
                    self.builder.push(Instr::Local(0x21, index_tmp));
                    self.labels.pop();
                    self.builder.push(Instr::Op(0x0b));
                    self.builder.push(Instr::Local(0x20, index_tmp));
                    self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                    self.builder.push(Instr::Op(0x53));
                    self.builder.push(Instr::Local(0x20, index_tmp));
                    self.builder.push(Instr::Local(0x20, array_tmp));
                    self.builder.push(Instr::Mem(0x28, 2, 0));
                    self.builder.push(Instr::Op(0xad));
                    self.builder.push(Instr::Op(0x59));
                    self.builder.push(Instr::Op(0x72));
                    self.builder.push(Instr::Block {
                        opcode: 0x04,
                        end: 0,
                        else_at: 0,
                        start: 0,
                    });
                    self.labels.push(0x04);
                    self.emit_panic("index out of bounds");
                    self.labels.pop();
                    self.builder.push(Instr::Op(0x0b));
                    self.builder.push(Instr::Local(0x20, array_tmp));
                    self.builder.push(Instr::Local(0x20, index_tmp));
                    self.builder.push(Instr::Op(0xa7));
                    self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                    self.builder.push(Instr::Op(0x6c));
                    self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                    self.builder.push(Instr::Op(0x6a));
                    self.builder.push(Instr::Op(0x6a));
                    self.builder.push(Instr::Mem(0x28, 2, 0));
                    self.builder.push(Instr::Local(0x20, array_tmp));
                    self.builder.push(Instr::Local(0x20, index_tmp));
                    self.builder.push(Instr::Op(0xa7));
                    self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                    self.builder.push(Instr::Op(0x6c));
                    self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                    self.builder.push(Instr::Op(0x6a));
                    self.builder.push(Instr::Op(0x6a));
                    self.builder.push(Instr::Mem(0x28, 2, 0));
                    return Ok(Ty::Str);
                }
                if !matches!(
                    array_ty,
                    Ty::Arr | Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrF | Ty::ArrStrArr
                ) {
                    return Err(format!(
                        "wasm target indexes arrays and strings only, got {}",
                        ty_name(array_ty)
                    ));
                }
                let array_tmp = self.builder.add_local(ValType::I32);
                let index_tmp = self.builder.add_local(ValType::I64);
                self.emit_expr(array)?;
                self.builder.push(Instr::Local(0x21, array_tmp));
                if self.type_of(index)? == Ty::Float {
                    self.emit_f64(index)?;
                    self.builder.push(Instr::Op(0xb0));
                } else {
                    self.emit_expr(index)?;
                }
                self.builder.push(Instr::Local(0x21, index_tmp));
                // negative indexes count from the end, as in the VM
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.builder.push(Instr::Op(0x53));
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Local(0x20, array_tmp));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Op(0xad));
                self.builder.push(Instr::Op(0x7c));
                self.builder.push(Instr::Local(0x21, index_tmp));
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                // bounds check: index < 0 || index >= len
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                self.builder.push(Instr::Op(0x53));
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Local(0x20, array_tmp));
                self.builder.push(Instr::Mem(0x28, 2, 0));
                self.builder.push(Instr::Op(0xad));
                self.builder.push(Instr::Op(0x59));
                self.builder.push(Instr::Op(0x72));
                self.builder.push(Instr::Block {
                    opcode: 0x04,
                    end: 0,
                    else_at: 0,
                    start: 0,
                });
                self.labels.push(0x04);
                self.emit_panic("index out of bounds");
                self.labels.pop();
                self.builder.push(Instr::Op(0x0b));
                self.builder.push(Instr::Local(0x20, array_tmp));
                self.builder.push(Instr::Local(0x20, index_tmp));
                self.builder.push(Instr::Op(0xa7));
                self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                self.builder.push(Instr::Op(0x6c));
                self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                self.builder.push(Instr::Op(0x6a));
                self.builder.push(Instr::Op(0x6a));
                if array_ty == Ty::Arr {
                    self.builder.push(Instr::Mem(0x29, 3, 0));
                    return Ok(Ty::Int);
                }
                if array_ty == Ty::ArrF {
                    self.builder.push(Instr::Mem(0x2b, 3, 0));
                    return Ok(Ty::Float);
                }
                self.builder.push(Instr::Mem(0x28, 2, 0));
                if array_ty == Ty::ArrArr {
                    return Ok(Ty::Arr);
                }
                if array_ty == Ty::ArrStrArr {
                    return Ok(Ty::ArrStr);
                }
                let Ty::ArrObj(shape) = array_ty else {
                    return Err("wasm target lost the record-array shape".to_string());
                };
                Ok(Ty::Obj(shape))
            }
            Expr::MethodCall(object, key, args, _) => {
                match key.as_str() {
                    "trim" | "upper" | "lower" if args.is_empty() => {
                        self.emit_str(object)?;
                        let import = match key.as_str() {
                            "trim" => STR_TRIM,
                            "upper" => STR_UPPER,
                            _ => STR_LOWER,
                        };
                        self.builder.push(Instr::Call(import));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    "repeat" if args.len() == 1 => {
                        self.emit_str(object)?;
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Call(STR_REPEAT));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    "split" if args.len() == 1 => {
                        self.emit_str(object)?;
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(STR_SPLIT));
                        return Ok(Ty::ArrStr);
                    }
                    "slice" if args.len() == 2 => {
                        self.emit_str(object)?;
                        self.emit_expr(&args[0])?;
                        self.emit_expr(&args[1])?;
                        self.builder.push(Instr::Call(STR_SLICE));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    "replace" if args.len() == 2 => {
                        self.emit_str(object)?;
                        self.emit_str(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Call(STR_REPLACE));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    "index_of" if args.len() == 1 && self.type_of(object)? == Ty::Str => {
                        self.emit_str(object)?;
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(STR_INDEX_OF));
                        return Ok(Ty::Int);
                    }
                    _ => {}
                }
                if key == "index_of" && args.len() == 1 {
                    match self.type_of(object)? {
                        Ty::Arr => {
                            self.emit_expr(object)?;
                            self.emit_expr(&args[0])?;
                            self.builder.push(Instr::Call(ARR_INDEX_OF));
                            return Ok(Ty::Int);
                        }
                        Ty::ArrF => {
                            self.emit_expr(object)?;
                            self.emit_f64(&args[0])?;
                            self.builder.push(Instr::Call(ARR_INDEX_OF_F));
                            return Ok(Ty::Int);
                        }
                        Ty::ArrStr => {
                            self.emit_expr(object)?;
                            self.emit_str(&args[0])?;
                            self.builder.push(Instr::Call(ARR_INDEX_OF_STR));
                            return Ok(Ty::Int);
                        }
                        _ => {
                            self.emit_str(object)?;
                            self.emit_str(&args[0])?;
                            self.builder.push(Instr::Call(STR_INDEX_OF));
                            return Ok(Ty::Int);
                        }
                    }
                }
                if matches!(key.as_str(), "contains" | "starts_with" | "ends_with") {
                    if args.len() != 1 {
                        return Err(format!("wasm target .{} takes one string", key));
                    }
                    if key == "contains" {
                        match self.type_of(object)? {
                            Ty::Arr => {
                                self.emit_expr(object)?;
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF));
                                self.builder.push(Instr::Const(0x42, WVal::I64(-1)));
                                self.builder.push(Instr::Op(0x52));
                                return Ok(Ty::Bool);
                            }
                            Ty::ArrF => {
                                self.emit_expr(object)?;
                                self.emit_f64(&args[0])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF_F));
                                self.builder.push(Instr::Const(0x42, WVal::I64(-1)));
                                self.builder.push(Instr::Op(0x52));
                                return Ok(Ty::Bool);
                            }
                            Ty::ArrStr => {
                                self.emit_expr(object)?;
                                self.emit_str(&args[0])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF_STR));
                                self.builder.push(Instr::Const(0x42, WVal::I64(-1)));
                                self.builder.push(Instr::Op(0x52));
                                return Ok(Ty::Bool);
                            }
                            _ => {}
                        }
                    }
                    if self.type_of(object)? != Ty::Str || self.type_of(&args[0])? != Ty::Str {
                        return Err(format!("wasm target .{} works on strings only", key));
                    }
                    let mode = match key.as_str() {
                        "contains" => 0,
                        "starts_with" => 1,
                        _ => 2,
                    };
                    self.emit_str(object)?;
                    self.emit_str(&args[0])?;
                    self.builder.push(Instr::Const(0x41, WVal::I32(mode)));
                    self.builder.push(Instr::Call(STR_TEST));
                    return Ok(Ty::Bool);
                }
                if key == "len" && args.is_empty() {
                    return match self.type_of(object)? {
                        Ty::Map => {
                            self.emit_expr(object)?;
                            self.builder.push(Instr::Call(MAP_LEN));
                            self.builder.push(Instr::Op(0xad));
                            Ok(Ty::Int)
                        }
                        Ty::Arr | Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrF | Ty::ArrStrArr => {
                            self.emit_expr(object)?;
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                            self.builder.push(Instr::Op(0xad));
                            Ok(Ty::Int)
                        }
                        Ty::Str => {
                            self.emit_str(object)?;
                            self.builder.push(Instr::Call(STR_LEN));
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
                if let Expr::Ident(name) = &**callee {
                    if name == "exit" && args.len() == 1 {
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Call(EXIT));
                        return Ok(Ty::Int);
                    }
                    if name == "error" && args.len() == 1 {
                        if let Some(ctx) = self.attempts.last().copied() {
                            self.emit_str(&args[0])?;
                            self.emit_attempt_error_pair(ctx.record);
                            self.emit_attempt_jump();
                            return Ok(Ty::Int);
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(ERROR));
                        return Ok(Ty::Int);
                    }
                    if name == "assert" && (args.len() == 1 || args.len() == 2) {
                        self.emit_cond(&args[0])?;
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::Block {
                            opcode: 0x04,
                            end: 0,
                            else_at: 0,
                            start: 0,
                        });
                        self.labels.push(0x04);
                        match args.get(1) {
                            Some(message) => self.emit_str(message)?,
                            None => self.emit_str(&Expr::Str("assertion failed".to_string()))?,
                        }
                        self.builder.push(Instr::Call(ERROR));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        return Ok(Ty::Int);
                    }
                }
                if let Some((params, result, index)) = self.callable(callee)? {
                    let literal_call = match &**callee {
                        Expr::Ident(name) => self.checker.fn_sig(name).is_none(),
                        _ => true,
                    };
                    if literal_call {
                        self.emit_expr(callee)?;
                    }
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
                        if !self.attempts.is_empty() {
                            self.emit_str(&args[0])?;
                            self.builder.push(Instr::Call(READ_FILE_SOFT));
                            self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                            self.emit_soft_io_check(self.temp(ARG_TMP));
                            self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                            self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                            self.builder.push(Instr::Mem(0x28, 2, 4));
                            return Ok(Ty::Str);
                        }
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
                    if name == "read" && args.len() == 1 {
                        if !self.attempts.is_empty() {
                            self.emit_str(&args[0])?;
                            self.builder.push(Instr::Call(READ_FILE_SOFT));
                            self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                            self.emit_soft_io_check(self.temp(ARG_TMP));
                            self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                            self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                            self.builder.push(Instr::Mem(0x28, 2, 4));
                            return Ok(Ty::Str);
                        }
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
                    if name == "write" && args.len() == 2 {
                        if !self.attempts.is_empty() {
                            self.emit_str(&args[0])?;
                            self.emit_str(&args[1])?;
                            self.builder.push(Instr::Call(WRITE_FILE_SOFT));
                            self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                            self.emit_soft_io_check(self.temp(ARG_TMP));
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Op(0xac));
                            return Ok(Ty::Int);
                        }
                        self.emit_str(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Call(5));
                        self.builder.push(Instr::Op(0xac));
                        return Ok(Ty::Int);
                    }
                    if name == "which" && args.len() == 1 {
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(WHICH));
                        self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, self.temp(ARG_TMP)));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        return Ok(Ty::Str);
                    }
                    if name == "write_file" && args.len() == 2 {
                        if !self.attempts.is_empty() {
                            self.emit_str(&args[0])?;
                            self.emit_str(&args[1])?;
                            self.builder.push(Instr::Call(WRITE_FILE_SOFT));
                            self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                            self.emit_soft_io_check(self.temp(ARG_TMP));
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Op(0xac));
                            return Ok(Ty::Int);
                        }
                        self.emit_str(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Call(5));
                        self.builder.push(Instr::Op(0xac));
                        return Ok(Ty::Int);
                    }
                    if name == "mkdir" && args.len() == 1 {
                        if self.type_of(&args[0])? != Ty::Str {
                            return Err("wasm target mkdir(path) takes a string".to_string());
                        }
                        if !self.attempts.is_empty() {
                            self.emit_str(&args[0])?;
                            self.builder.push(Instr::Call(MKDIR_SOFT));
                            self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                            self.emit_soft_io_check(self.temp(ARG_TMP));
                            self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                            return Ok(Ty::Int);
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(MKDIR_SOFT));
                        self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                        self.emit_soft_io_panic(self.temp(ARG_TMP));
                        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
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

                    if name == "exists" && args.len() == 1 {
                        if self.type_of(&args[0])? != Ty::Str {
                            return Err("wasm target exists(path) needs a string".to_string());
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(FILE_EXISTS));
                        return Ok(Ty::Bool);
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
                    if name == "attempt" && args.len() == 1 {
                        return self.emit_attempt(&args[0]);
                    }
                    if name == "exec" && (args.len() == 1 || args.len() == 2) {
                        return self.emit_exec_record(args, false);
                    }
                    if name == "sh" && args.len() == 1 {
                        return self.emit_exec_record(args, true);
                    }
                    if name == "push" && args.len() == 2 {
                        let Expr::Ident(array) = &args[0] else {
                            return Err(
                                "wasm target pushes into a named array variable only".to_string()
                            );
                        };
                        self.emit_push(array, &args[1])?;
                        let ty = self.checker.binding(array)?.ty;
                        self.emit_load(array)?;
                        return Ok(ty);
                    }
                    if name == "pop" && args.len() == 1 {
                        let Expr::Ident(array) = &args[0] else {
                            return Err("wasm target pops a named array variable only".to_string());
                        };
                        return self.emit_pop_value(array);
                    }
                    if name == "contains" && args.len() == 2 {
                        match self.type_of(&args[0])? {
                            Ty::Arr => {
                                self.emit_expr(&args[0])?;
                                self.emit_expr(&args[1])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF));
                            }
                            Ty::ArrStr => {
                                self.emit_expr(&args[0])?;
                                self.emit_str(&args[1])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF_STR));
                            }
                            Ty::ArrF => {
                                self.emit_expr(&args[0])?;
                                self.emit_f64(&args[1])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF_F));
                            }
                            Ty::ArrObj(shape) => {
                                self.emit_array_search(&args[0], &args[1], Ty::Obj(shape), false)?;
                                return Ok(Ty::Bool);
                            }
                            Ty::ArrArr => {
                                self.emit_array_search(&args[0], &args[1], Ty::Arr, false)?;
                                return Ok(Ty::Bool);
                            }
                            Ty::ArrStrArr => {
                                self.emit_array_search(&args[0], &args[1], Ty::ArrStr, false)?;
                                return Ok(Ty::Bool);
                            }
                            other => {
                                return Err(format!(
                                    "wasm target contains() supports int, string and float arrays, got {}",
                                    ty_name(other)
                                ))
                            }
                        }
                        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                        self.builder.push(Instr::Op(0x59));
                        return Ok(Ty::Bool);
                    }
                    if name == "float" && args.len() == 1 {
                        match self.type_of(&args[0])? {
                            Ty::Str => {
                                self.emit_str(&args[0])?;
                                self.builder.push(Instr::Call(STR_TO_FLOAT));
                            }
                            Ty::Int => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Op(0xb9));
                            }
                            _ => self.emit_f64(&args[0])?,
                        }
                        return Ok(Ty::Float);
                    }
                    if name == "abs" && args.len() == 1 {
                        return match self.type_of(&args[0])? {
                            Ty::Int => {
                                let temp = self.builder.add_local(ValType::I64);
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Local(0x21, temp));
                                self.builder.push(Instr::Local(0x20, temp));
                                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                                self.builder.push(Instr::Op(0x53));
                                self.builder.push(Instr::Block {
                                    opcode: 0x04,
                                    end: 0,
                                    else_at: 0,
                                    start: 0,
                                });
                                self.labels.push(0x04);
                                self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                                self.builder.push(Instr::Local(0x20, temp));
                                self.builder.push(Instr::Op(0x7d));
                                self.builder.push(Instr::Local(0x21, temp));
                                self.labels.pop();
                                self.builder.push(Instr::Op(0x0b));
                                self.builder.push(Instr::Local(0x20, temp));
                                Ok(Ty::Int)
                            }
                            Ty::Float => {
                                self.emit_f64(&args[0])?;
                                self.builder.push(Instr::Op(0x99));
                                Ok(Ty::Float)
                            }
                            other => Err(format!(
                                "wasm target abs() takes a number, got {}",
                                ty_name(other)
                            )),
                        };
                    }
                    if matches!(name.as_str(), "floor" | "ceil") && args.len() == 1 {
                        self.emit_f64(&args[0])?;
                        let opcode = if name == "floor" { 0x9c } else { 0x9b };
                        self.builder.push(Instr::Op(opcode));
                        return Ok(Ty::Float);
                    }
                    if name == "round" && args.len() == 1 {
                        // The VM rounds half away from zero, while wasm's nearest is
                        // ties-to-even; lower it to floor(x+0.5)/ceil(x-0.5).
                        let temp = self.builder.add_local(ValType::F64);
                        self.emit_f64(&args[0])?;
                        self.builder.push(Instr::Local(0x21, temp));
                        self.builder.push(Instr::Local(0x20, temp));
                        self.builder.push(Instr::Const(0x44, WVal::F64(0.0)));
                        self.builder.push(Instr::Op(0x66));
                        self.builder.push(Instr::Block {
                            opcode: 0x04,
                            end: 0,
                            else_at: 0,
                            start: 0,
                        });
                        self.labels.push(0x04);
                        self.builder.push(Instr::Local(0x20, temp));
                        self.builder.push(Instr::Const(0x44, WVal::F64(0.5)));
                        self.builder.push(Instr::Op(0xa0));
                        self.builder.push(Instr::Op(0x9c));
                        self.builder.push(Instr::Local(0x21, temp));
                        self.builder.push(Instr::Else { end: 0 });
                        self.builder.push(Instr::Local(0x20, temp));
                        self.builder.push(Instr::Const(0x44, WVal::F64(0.5)));
                        self.builder.push(Instr::Op(0xa1));
                        self.builder.push(Instr::Op(0x9b));
                        self.builder.push(Instr::Local(0x21, temp));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, temp));
                        return Ok(Ty::Float);
                    }
                    if name == "sqrt" && args.len() == 1 {
                        self.emit_f64(&args[0])?;
                        self.builder.push(Instr::Op(0x9f));
                        return Ok(Ty::Float);
                    }
                    if matches!(name.as_str(), "trim" | "upper" | "lower") && args.len() == 1 {
                        let import = match name.as_str() {
                            "trim" => STR_TRIM,
                            "upper" => STR_UPPER,
                            _ => STR_LOWER,
                        };
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(import));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    if name == "replace" && args.len() == 3 {
                        for arg in args {
                            self.emit_str(arg)?;
                        }
                        self.builder.push(Instr::Call(STR_REPLACE));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    if name == "index_of" && args.len() == 2 {
                        match self.type_of(&args[0])? {
                            Ty::Arr => {
                                self.emit_expr(&args[0])?;
                                self.emit_expr(&args[1])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF));
                                return Ok(Ty::Int);
                            }
                            Ty::ArrF => {
                                self.emit_expr(&args[0])?;
                                self.emit_f64(&args[1])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF_F));
                                return Ok(Ty::Int);
                            }
                            Ty::ArrStr => {
                                self.emit_expr(&args[0])?;
                                self.emit_str(&args[1])?;
                                self.builder.push(Instr::Call(ARR_INDEX_OF_STR));
                                return Ok(Ty::Int);
                            }
                            Ty::ArrObj(shape) => {
                                self.emit_array_search(&args[0], &args[1], Ty::Obj(shape), true)?;
                                return Ok(Ty::Int);
                            }
                            Ty::ArrArr => {
                                self.emit_array_search(&args[0], &args[1], Ty::Arr, true)?;
                                return Ok(Ty::Int);
                            }
                            Ty::ArrStrArr => {
                                self.emit_array_search(&args[0], &args[1], Ty::ArrStr, true)?;
                                return Ok(Ty::Int);
                            }
                            _ => {
                                self.emit_str(&args[0])?;
                                self.emit_str(&args[1])?;
                                self.builder.push(Instr::Call(STR_INDEX_OF));
                                return Ok(Ty::Int);
                            }
                        }
                    }
                    if matches!(name.as_str(), "min" | "max") && args.len() == 1 {
                        let want_min = name == "min";
                        return match self.type_of(&args[0])? {
                            Ty::Arr => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Const(
                                    0x41,
                                    WVal::I32(if want_min { 1 } else { 0 }),
                                ));
                                self.builder.push(Instr::Call(ARR_MINMAX));
                                Ok(Ty::Int)
                            }
                            Ty::ArrF => {
                                let arr_tmp = self.builder.add_local(ValType::I32);
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Local(0x21, arr_tmp));
                                self.builder.push(Instr::Local(0x20, arr_tmp));
                                self.builder.push(Instr::Mem(0x28, 2, 0));
                                self.builder.push(Instr::Op(0x45));
                                self.builder.push(Instr::Block {
                                    opcode: 0x04,
                                    end: 0,
                                    else_at: 0,
                                    start: 0,
                                });
                                self.labels.push(0x04);
                                self.emit_panic("min()/max() of an empty array");
                                self.labels.pop();
                                self.builder.push(Instr::Op(0x0b));
                                self.builder.push(Instr::Local(0x20, arr_tmp));
                                self.builder.push(Instr::Const(
                                    0x41,
                                    WVal::I32(if want_min { 1 } else { 0 }),
                                ));
                                self.builder.push(Instr::Call(ARR_MINMAX_F));
                                Ok(Ty::Float)
                            }
                            Ty::ArrStr => {
                                let arr_tmp = self.builder.add_local(ValType::I32);
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Local(0x21, arr_tmp));
                                self.builder.push(Instr::Local(0x20, arr_tmp));
                                self.builder.push(Instr::Mem(0x28, 2, 0));
                                self.builder.push(Instr::Op(0x45));
                                self.builder.push(Instr::Block {
                                    opcode: 0x04,
                                    end: 0,
                                    else_at: 0,
                                    start: 0,
                                });
                                self.labels.push(0x04);
                                self.emit_panic("min()/max() of an empty array");
                                self.labels.pop();
                                self.builder.push(Instr::Op(0x0b));
                                self.builder.push(Instr::Local(0x20, arr_tmp));
                                self.builder.push(Instr::Const(
                                    0x41,
                                    WVal::I32(if want_min { 1 } else { 0 }),
                                ));
                                self.builder.push(Instr::Call(ARR_MINMAX_STR));
                                self.push_block_as_pair();
                                Ok(Ty::Str)
                            }
                            _ => {
                                Err("wasm target min()/max() takes an int or string array"
                                    .to_string())
                            }
                        };
                    }
                    if name == "repeat" && args.len() == 2 {
                        self.emit_str(&args[0])?;
                        self.emit_expr(&args[1])?;
                        self.builder.push(Instr::Call(STR_REPEAT));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    if name == "type" && args.len() == 1 {
                        let ty = self.type_of(&args[0])?;
                        let label = match ty {
                            Ty::Int => "int",
                            Ty::Float => "float",
                            Ty::Str => "str",
                            Ty::Bool => "bool",
                            Ty::Arr
                            | Ty::ArrStr
                            | Ty::ArrObj(_)
                            | Ty::ArrArr
                            | Ty::ArrF
                            | Ty::ArrStrArr => "array",
                            Ty::Obj(_) | Ty::Map => "object",
                            Ty::Func(_) => "function",
                        };
                        let offset = self.builder.add_data(label.as_bytes());
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(offset as i32)));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(label.len() as i32)));
                        return Ok(Ty::Str);
                    }
                    if name == "split" && args.len() == 2 {
                        self.emit_str(&args[0])?;
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Call(STR_SPLIT));
                        return Ok(Ty::ArrStr);
                    }
                    if name == "join" && (args.len() == 1 || args.len() == 2) {
                        let base = self.type_of(&args[0])?;
                        let import = match base {
                            Ty::Arr => ARR_JOIN_INT,
                            Ty::ArrF => ARR_JOIN_F,
                            Ty::ArrStr => STR_JOIN,
                            other => {
                                return Err(format!(
                                    "wasm target join() takes a numeric or string array, got {}",
                                    ty_name(other)
                                ))
                            }
                        };
                        self.emit_expr(&args[0])?;
                        if args.len() == 2 {
                            self.emit_str(&args[1])?;
                        } else {
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                        }
                        self.builder.push(Instr::Call(import));
                        self.push_block_as_pair();
                        return Ok(Ty::Str);
                    }
                    if name == "slice" && (args.len() == 2 || args.len() == 3) {
                        let base = self.type_of(&args[0])?;
                        match base {
                            Ty::Str => {
                                let ptr_tmp = self.builder.add_local(ValType::I32);
                                let len_tmp = self.builder.add_local(ValType::I32);
                                self.emit_str(&args[0])?;
                                self.builder.push(Instr::Local(0x21, len_tmp));
                                self.builder.push(Instr::Local(0x21, ptr_tmp));
                                self.builder.push(Instr::Local(0x20, ptr_tmp));
                                self.builder.push(Instr::Local(0x20, len_tmp));
                                self.emit_expr(&args[1])?;
                                if args.len() == 3 {
                                    self.emit_expr(&args[2])?;
                                } else {
                                    self.builder.push(Instr::Local(0x20, len_tmp));
                                    self.builder.push(Instr::Op(0xad));
                                }
                                self.builder.push(Instr::Call(STR_SLICE));
                                self.push_block_as_pair();
                                return Ok(Ty::Str);
                            }
                            Ty::Arr
                            | Ty::ArrF
                            | Ty::ArrStr
                            | Ty::ArrObj(_)
                            | Ty::ArrArr
                            | Ty::ArrStrArr => {
                                let ptr_tmp = self.builder.add_local(ValType::I32);
                                let len_tmp = self.builder.add_local(ValType::I32);
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Local(0x21, ptr_tmp));
                                self.builder.push(Instr::Local(0x20, ptr_tmp));
                                self.builder.push(Instr::Mem(0x28, 2, 0));
                                self.builder.push(Instr::Local(0x21, len_tmp));
                                self.builder.push(Instr::Local(0x20, ptr_tmp));
                                self.emit_expr(&args[1])?;
                                if args.len() == 3 {
                                    self.emit_expr(&args[2])?;
                                } else {
                                    self.builder.push(Instr::Local(0x20, len_tmp));
                                    self.builder.push(Instr::Op(0xad));
                                }
                                self.builder.push(Instr::Call(ARR_SLICE));
                                return Ok(base);
                            }
                            other => {
                                return Err(format!("wasm target cannot slice {}", ty_name(other)))
                            }
                        };
                    }
                    if name == "range" && (args.len() == 1 || args.len() == 2 || args.len() == 3) {
                        if args.len() == 1 {
                            self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                            self.emit_expr(&args[0])?;
                            self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        } else {
                            self.emit_expr(&args[0])?;
                            self.emit_expr(&args[1])?;
                            // The VM ignores range()'s third argument; always step by 1.
                            self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        }
                        self.builder.push(Instr::Call(ARR_RANGE));
                        return Ok(Ty::Arr);
                    }
                    if name == "sum" && args.len() == 1 {
                        return match self.type_of(&args[0])? {
                            Ty::ArrF => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Call(ARR_SUM_F));
                                Ok(Ty::Float)
                            }
                            Ty::Arr => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Call(ARR_SUM));
                                Ok(Ty::Int)
                            }
                            other => Err(format!(
                                "wasm target sum() takes a numeric array, got {}",
                                ty_name(other)
                            )),
                        };
                    }
                    if name == "sort"
                        && args.len() == 2
                        && matches!(
                            self.type_of(&args[0])?,
                            Ty::ArrF | Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrStrArr
                        )
                    {
                        let input_ty = self.type_of(&args[0])?;
                        let mode = match input_ty {
                            Ty::ArrF => SortMode::Float,
                            _ => SortMode::Ptr,
                        };
                        let Ty::Func(id) = self.type_of(&args[1])? else {
                            return Err("wasm target sort() needs a function literal comparator"
                                .to_string());
                        };
                        let sig = self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        self.emit_merge_sort(&args[0], &args[1], input_ty, mode, &sig)?;
                        return Ok(input_ty);
                    }
                    if name == "sort" && args.len() == 2 && self.type_of(&args[0])? == Ty::ArrStr {
                        let Ty::Func(id) = self.type_of(&args[1])? else {
                            return Err("wasm target sort() needs a function literal comparator"
                                .to_string());
                        };
                        let sig = self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        self.emit_merge_sort(&args[0], &args[1], Ty::ArrStr, SortMode::Str, &sig)?;
                        return Ok(Ty::ArrStr);
                    }
                    if name == "sort" && args.len() == 2 {
                        let Ty::Func(id) = self.type_of(&args[1])? else {
                            return Err("wasm target sort() needs a function literal comparator"
                                .to_string());
                        };
                        let sig = self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        self.emit_merge_sort(&args[0], &args[1], Ty::Arr, SortMode::Int, &sig)?;
                        return Ok(Ty::Arr);
                    }
                    if matches!(name.as_str(), "reverse" | "sort") && args.len() == 1 {
                        let ty = self.type_of(&args[0])?;
                        let import = match (name.as_str(), ty) {
                            (
                                "reverse",
                                Ty::Arr
                                | Ty::ArrF
                                | Ty::ArrStr
                                | Ty::ArrObj(_)
                                | Ty::ArrArr
                                | Ty::ArrStrArr,
                            ) => ARR_REVERSE,
                            ("sort", Ty::Arr) => ARR_SORT,
                            ("sort", Ty::ArrF) => ARR_SORT_F,
                            ("sort", Ty::ArrStr) => ARR_SORT_STR,
                            _ => {
                                return Err(format!(
                                    "wasm target {}() takes a numeric or string array, got {}",
                                    name,
                                    ty_name(ty)
                                ))
                            }
                        };
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Call(import));
                        return Ok(ty);
                    }
                    if matches!(name.as_str(), "map" | "filter") && args.len() == 2 {
                        let input_ty = self.type_of(&args[0])?;
                        let elem = match input_ty {
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            other => {
                                return Err(format!(
                                    "wasm target {}() takes an array, got {}",
                                    name,
                                    ty_name(other)
                                ))
                            }
                        };
                        let Ty::Func(id) = self.type_of(&args[1])? else {
                            return Err(format!(
                                "wasm target {}() needs a function literal callback",
                                name
                            ));
                        };
                        let sig = self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        let cb_env = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[1])?;
                        self.builder.push(Instr::Local(0x21, cb_env));
                        let result_elem = if name == "map" {
                            sig.result.ok_or_else(|| {
                                "wasm target map() callback returns nothing".to_string()
                            })?
                        } else {
                            elem
                        };
                        if !matches!(
                            result_elem,
                            Ty::Int | Ty::Float | Ty::Str | Ty::Obj(_) | Ty::Arr | Ty::ArrStr
                        ) {
                            return Err(
                                "wasm target map() callbacks return numbers, strings or records"
                                    .to_string(),
                            );
                        }
                        let arr_tmp = self.builder.add_local(ValType::I32);
                        let idx_tmp = self.builder.add_local(ValType::I64);
                        let out_tmp = self.builder.add_local(ValType::I32);
                        let count_tmp = self.builder.add_local(ValType::I32);
                        let addr_tmp = self.builder.add_local(ValType::I32);
                        let ptr_tmp = self.builder.add_local(ValType::I32);
                        let len_tmp = self.builder.add_local(ValType::I32);
                        let val_i64 = self.builder.add_local(ValType::I64);
                        let val_f64 = self.builder.add_local(ValType::F64);
                        let val_i32 = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, arr_tmp));
                        self.builder.push(Instr::Local(0x20, arr_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.emit_alloc_dynamic();
                        self.builder.push(Instr::Local(0x21, out_tmp));
                        if name == "map" {
                            self.builder.push(Instr::Local(0x20, out_tmp));
                            self.builder.push(Instr::Local(0x20, arr_tmp));
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                            self.builder.push(Instr::Mem(0x36, 2, 0));
                        } else {
                            self.builder.push(Instr::Local(0x20, out_tmp));
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Mem(0x36, 2, 0));
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Local(0x21, count_tmp));
                        }
                        self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                        self.builder.push(Instr::Local(0x21, idx_tmp));
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
                        self.builder.push(Instr::Local(0x20, idx_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Local(0x20, arr_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder.push(Instr::Op(0x48));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::BrIf(1));
                        if name == "map" {
                            self.builder.push(Instr::Local(0x20, out_tmp));
                            self.builder.push(Instr::Local(0x20, idx_tmp));
                            self.builder.push(Instr::Op(0xa7));
                            self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            self.builder.push(Instr::Op(0x6c));
                            self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x21, addr_tmp));
                            self.builder.push(Instr::Local(0x20, cb_env));
                            Self::emit_load_slot(self, arr_tmp, idx_tmp, elem);
                            self.builder.push(Instr::Call(sig.index));
                            Self::emit_store_slot(
                                self,
                                addr_tmp,
                                result_elem,
                                val_i64,
                                val_f64,
                                val_i32,
                                ptr_tmp,
                                len_tmp,
                            );
                        } else {
                            self.builder.push(Instr::Local(0x20, cb_env));
                            Self::emit_load_slot(self, arr_tmp, idx_tmp, elem);
                            self.builder.push(Instr::Call(sig.index));
                            self.builder.push(Instr::Block {
                                opcode: 0x04,
                                end: 0,
                                else_at: 0,
                                start: 0,
                            });
                            self.labels.push(0x04);
                            self.builder.push(Instr::Local(0x20, out_tmp));
                            self.builder.push(Instr::Local(0x20, count_tmp));
                            self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            self.builder.push(Instr::Op(0x6c));
                            self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x21, addr_tmp));
                            Self::emit_load_slot(self, arr_tmp, idx_tmp, elem);
                            Self::emit_store_slot(
                                self, addr_tmp, elem, val_i64, val_f64, val_i32, ptr_tmp, len_tmp,
                            );
                            self.builder.push(Instr::Local(0x20, count_tmp));
                            self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x21, count_tmp));
                            self.labels.pop();
                            self.builder.push(Instr::Op(0x0b));
                        }
                        self.builder.push(Instr::Local(0x20, idx_tmp));
                        self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        self.builder.push(Instr::Op(0x7c));
                        self.builder.push(Instr::Local(0x21, idx_tmp));
                        self.builder.push(Instr::Br(0));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        if name == "filter" {
                            self.builder.push(Instr::Local(0x20, out_tmp));
                            self.builder.push(Instr::Local(0x20, count_tmp));
                            self.builder.push(Instr::Mem(0x36, 2, 0));
                        }
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        let output_ty = if name == "map" {
                            Self::array_of(result_elem)
                        } else {
                            input_ty
                        };
                        return Ok(output_ty);
                    }
                    if name == "reduce" && (args.len() == 2 || args.len() == 3) {
                        let input_ty = self.type_of(&args[0])?;
                        let elem = match input_ty {
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            other => {
                                return Err(format!(
                                    "wasm target reduce() takes an array, got {}",
                                    ty_name(other)
                                ))
                            }
                        };
                        let acc_ty = if args.len() == 3 {
                            self.type_of(&args[2])?
                        } else {
                            elem
                        };
                        if !matches!(acc_ty, Ty::Int | Ty::Float | Ty::Str) {
                            return Err(
                                "wasm target reduce() accumulator must be a number or string"
                                    .to_string(),
                            );
                        }
                        let Ty::Func(id) = self.type_of(&args[1])? else {
                            return Err("wasm target reduce() needs a function literal callback"
                                .to_string());
                        };
                        let sig = self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        let cb_env = self.builder.add_local(ValType::I32);
                        let arr_tmp = self.builder.add_local(ValType::I32);
                        let idx_tmp = self.builder.add_local(ValType::I64);
                        let acc_i64 = self.builder.add_local(ValType::I64);
                        let acc_f64 = self.builder.add_local(ValType::F64);
                        let acc_ptr = self.builder.add_local(ValType::I32);
                        let acc_len = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[1])?;
                        self.builder.push(Instr::Local(0x21, cb_env));
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, arr_tmp));
                        if args.len() == 3 {
                            self.emit_expr(&args[2])?;
                            Self::emit_store_acc(self, acc_ty, acc_i64, acc_f64, acc_ptr, acc_len);
                            self.builder.push(Instr::Const(0x42, WVal::I64(0)));
                        } else {
                            self.builder.push(Instr::Local(0x20, arr_tmp));
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                            self.builder.push(Instr::Op(0x45));
                            self.builder.push(Instr::Block {
                                opcode: 0x04,
                                end: 0,
                                else_at: 0,
                                start: 0,
                            });
                            self.labels.push(0x04);
                            self.emit_panic("reduce of empty array with no initial value");
                            self.labels.pop();
                            self.builder.push(Instr::Op(0x0b));
                            let zero_idx = self.builder.add_local(ValType::I64);
                            Self::emit_load_slot(self, arr_tmp, zero_idx, elem);
                            Self::emit_store_acc(self, acc_ty, acc_i64, acc_f64, acc_ptr, acc_len);
                            self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        }
                        self.builder.push(Instr::Local(0x21, idx_tmp));
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
                        self.builder.push(Instr::Local(0x20, idx_tmp));
                        self.builder.push(Instr::Local(0x20, arr_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder.push(Instr::Op(0xad));
                        self.builder.push(Instr::Op(0x53));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::BrIf(1));
                        self.builder.push(Instr::Local(0x20, cb_env));
                        Self::emit_push_acc(self, acc_ty, acc_i64, acc_f64, acc_ptr, acc_len);
                        Self::emit_load_slot(self, arr_tmp, idx_tmp, elem);
                        self.builder.push(Instr::Call(sig.index));
                        Self::emit_store_acc(self, acc_ty, acc_i64, acc_f64, acc_ptr, acc_len);
                        self.builder.push(Instr::Local(0x20, idx_tmp));
                        self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        self.builder.push(Instr::Op(0x7c));
                        self.builder.push(Instr::Local(0x21, idx_tmp));
                        self.builder.push(Instr::Br(0));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        Self::emit_push_acc(self, acc_ty, acc_i64, acc_f64, acc_ptr, acc_len);
                        return Ok(acc_ty);
                    }
                    if name == "keys" && args.len() == 1 && self.type_of(&args[0])? == Ty::Map {
                        let map_tmp = self.builder.add_local(ValType::I32);
                        let out_tmp = self.builder.add_local(ValType::I32);
                        let index_tmp = self.builder.add_local(ValType::I32);
                        let count_tmp = self.builder.add_local(ValType::I32);
                        let block_tmp = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, map_tmp));
                        self.builder.push(Instr::Local(0x20, map_tmp));
                        self.builder.push(Instr::Call(MAP_LEN));
                        self.builder.push(Instr::Local(0x21, count_tmp));
                        self.builder.push(Instr::Local(0x20, count_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.emit_alloc_dynamic();
                        self.builder.push(Instr::Local(0x21, out_tmp));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder.push(Instr::Local(0x20, count_tmp));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
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
                        self.builder.push(Instr::Local(0x20, map_tmp));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Call(MAP_KEY_AT));
                        self.builder.push(Instr::Local(0x21, block_tmp));
                        // out entry ptr at out + 4 + 8*index
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, block_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                        // out entry len at the next word
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, block_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x21, index_tmp));
                        self.builder.push(Instr::Br(0));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        return Ok(Ty::ArrStr);
                    }
                    if name == "keys" && args.len() == 1 {
                        let Ty::Obj(shape) = self.type_of(&args[0])? else {
                            return Err("wasm target keys() takes a record".to_string());
                        };
                        let (table, count) = self.record_table(shape)?;
                        let record_tmp = self.builder.add_local(ValType::I32);
                        let out_tmp = self.builder.add_local(ValType::I32);
                        let index_tmp = self.builder.add_local(ValType::I64);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, record_tmp));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32((4 + 8 * count) as i32)));
                        self.emit_alloc_dynamic();
                        self.builder.push(Instr::Local(0x21, out_tmp));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
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
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Op(0x48));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::BrIf(1));
                        // key block address = table + load(table + 4*index)
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(table as i32)));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(table as i32)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x21, record_tmp));
                        // original record no longer needed; reuse temp as block ptr
                        let store_pair = |emitter: &mut Emitter, block_tmp: u32, index_tmp: u32| {
                            emitter.builder.push(Instr::Local(0x20, out_tmp));
                            emitter.builder.push(Instr::Local(0x20, index_tmp));
                            emitter.builder.push(Instr::Op(0xa7));
                            emitter.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            emitter.builder.push(Instr::Op(0x6c));
                            emitter.builder.push(Instr::Const(0x41, WVal::I32(4)));
                            emitter.builder.push(Instr::Op(0x6a));
                            emitter.builder.push(Instr::Op(0x6a));
                            emitter.builder.push(Instr::Local(0x20, block_tmp));
                            emitter.builder.push(Instr::Const(0x41, WVal::I32(4)));
                            emitter.builder.push(Instr::Op(0x6a));
                            emitter.builder.push(Instr::Mem(0x36, 2, 0));
                            emitter.builder.push(Instr::Local(0x20, out_tmp));
                            emitter.builder.push(Instr::Local(0x20, index_tmp));
                            emitter.builder.push(Instr::Op(0xa7));
                            emitter.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            emitter.builder.push(Instr::Op(0x6c));
                            emitter.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            emitter.builder.push(Instr::Op(0x6a));
                            emitter.builder.push(Instr::Op(0x6a));
                            emitter.builder.push(Instr::Local(0x20, block_tmp));
                            emitter.builder.push(Instr::Mem(0x28, 2, 0));
                            emitter.builder.push(Instr::Mem(0x36, 2, 0));
                        };
                        store_pair(self, record_tmp, index_tmp);
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        self.builder.push(Instr::Op(0x7c));
                        self.builder.push(Instr::Local(0x21, index_tmp));
                        self.builder.push(Instr::Br(0));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        return Ok(Ty::ArrStr);
                    }
                    if name == "values" && args.len() == 1 && self.type_of(&args[0])? == Ty::Map {
                        let map_tmp = self.builder.add_local(ValType::I32);
                        let out_tmp = self.builder.add_local(ValType::I32);
                        let index_tmp = self.builder.add_local(ValType::I32);
                        let count_tmp = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, map_tmp));
                        self.builder.push(Instr::Local(0x20, map_tmp));
                        self.builder.push(Instr::Call(MAP_LEN));
                        self.builder.push(Instr::Local(0x21, count_tmp));
                        self.builder.push(Instr::Local(0x20, count_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.emit_alloc_dynamic();
                        self.builder.push(Instr::Local(0x21, out_tmp));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder.push(Instr::Local(0x20, count_tmp));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
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
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, map_tmp));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Call(MAP_VALUE_AT));
                        self.builder.push(Instr::Mem(0x37, 3, 0));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x21, index_tmp));
                        self.builder.push(Instr::Br(0));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        return Ok(Ty::Arr);
                    }
                    if name == "values" && args.len() == 1 {
                        let Ty::Obj(shape) = self.type_of(&args[0])? else {
                            return Err("wasm target values() takes a record".to_string());
                        };
                        let (_, count) = self.record_table(shape)?;
                        let record_tmp = self.builder.add_local(ValType::I32);
                        let out_tmp = self.builder.add_local(ValType::I32);
                        let index_tmp = self.builder.add_local(ValType::I64);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, record_tmp));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32((4 + 8 * count) as i32)));
                        self.emit_alloc_dynamic();
                        self.builder.push(Instr::Local(0x21, out_tmp));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Mem(0x36, 2, 0));
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
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Op(0x48));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::BrIf(1));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, record_tmp));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Mem(0x29, 3, 0));
                        self.builder.push(Instr::Mem(0x37, 3, 0));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        self.builder.push(Instr::Op(0x7c));
                        self.builder.push(Instr::Local(0x21, index_tmp));
                        self.builder.push(Instr::Br(0));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, out_tmp));
                        return Ok(Ty::Arr);
                    }
                    if name == "has" && args.len() == 2 && self.type_of(&args[0])? == Ty::Map {
                        let map_tmp = self.builder.add_local(ValType::I32);
                        let key_ptr = self.builder.add_local(ValType::I32);
                        let key_len = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, map_tmp));
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Local(0x21, key_len));
                        self.builder.push(Instr::Local(0x21, key_ptr));
                        self.builder.push(Instr::Local(0x20, map_tmp));
                        self.builder.push(Instr::Local(0x20, key_ptr));
                        self.builder.push(Instr::Local(0x20, key_len));
                        self.builder.push(Instr::Call(MAP_HAS));
                        return Ok(Ty::Bool);
                    }
                    if name == "has" && args.len() == 2 {
                        let Ty::Obj(shape) = self.type_of(&args[0])? else {
                            return Err("wasm target has() takes a record".to_string());
                        };
                        let (table, count) = self.record_table(shape)?;
                        let record_tmp = self.builder.add_local(ValType::I32);
                        let key_ptr = self.builder.add_local(ValType::I32);
                        let key_len = self.builder.add_local(ValType::I32);
                        let index_tmp = self.builder.add_local(ValType::I64);
                        let block_tmp = self.builder.add_local(ValType::I32);
                        let found_tmp = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, record_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                        self.builder.push(Instr::Local(0x21, found_tmp));
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Local(0x21, key_len));
                        self.builder.push(Instr::Local(0x21, key_ptr));
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
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Op(0x48));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::BrIf(1));
                        // block address
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(table as i32)));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6c));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(table as i32)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x21, block_tmp));
                        self.builder.push(Instr::Local(0x20, block_tmp));
                        self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                        self.builder.push(Instr::Op(0x6a));
                        self.builder.push(Instr::Local(0x20, block_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder.push(Instr::Local(0x20, key_ptr));
                        self.builder.push(Instr::Local(0x20, key_len));
                        self.builder.push(Instr::Call(STR_COMPARE));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::Block {
                            opcode: 0x04,
                            end: 0,
                            else_at: 0,
                            start: 0,
                        });
                        self.labels.push(0x04);
                        self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                        self.builder.push(Instr::Local(0x21, found_tmp));
                        self.builder.push(Instr::Br(2));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Const(0x42, WVal::I64(1)));
                        self.builder.push(Instr::Op(0x7c));
                        self.builder.push(Instr::Local(0x21, index_tmp));
                        self.builder.push(Instr::Br(0));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, found_tmp));
                        return Ok(Ty::Bool);
                    }
                    if name == "get"
                        && (args.len() == 2 || args.len() == 3)
                        && self.type_of(&args[0])? == Ty::Map
                    {
                        let map_tmp = self.builder.add_local(ValType::I32);
                        let key_ptr = self.builder.add_local(ValType::I32);
                        let key_len = self.builder.add_local(ValType::I32);
                        let found_tmp = self.builder.add_local(ValType::I32);
                        let result_tmp = self.builder.add_local(ValType::I64);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, map_tmp));
                        self.emit_str(&args[1])?;
                        self.builder.push(Instr::Local(0x21, key_len));
                        self.builder.push(Instr::Local(0x21, key_ptr));
                        self.builder.push(Instr::Local(0x20, map_tmp));
                        self.builder.push(Instr::Local(0x20, key_ptr));
                        self.builder.push(Instr::Local(0x20, key_len));
                        self.builder.push(Instr::Call(MAP_HAS));
                        self.builder.push(Instr::Local(0x21, found_tmp));
                        // found -> read the value
                        self.builder.push(Instr::Local(0x20, found_tmp));
                        self.builder.push(Instr::Block {
                            opcode: 0x04,
                            end: 0,
                            else_at: 0,
                            start: 0,
                        });
                        self.labels.push(0x04);
                        self.builder.push(Instr::Local(0x20, map_tmp));
                        self.builder.push(Instr::Local(0x20, key_ptr));
                        self.builder.push(Instr::Local(0x20, key_len));
                        self.builder.push(Instr::Call(MAP_GET));
                        self.builder.push(Instr::Local(0x21, result_tmp));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        // missing -> default or abort
                        self.builder.push(Instr::Local(0x20, found_tmp));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::Block {
                            opcode: 0x04,
                            end: 0,
                            else_at: 0,
                            start: 0,
                        });
                        self.labels.push(0x04);
                        if args.len() == 3 {
                            self.emit_expr(&args[2])?;
                            self.builder.push(Instr::Local(0x21, result_tmp));
                        } else {
                            self.emit_panic("get() missing key with no default");
                        }
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        self.builder.push(Instr::Local(0x20, result_tmp));
                        return Ok(Ty::Int);
                    }
                    if name == "get" && (args.len() == 2 || args.len() == 3) {
                        let Ty::Obj(shape) = self.type_of(&args[0])? else {
                            return Err("wasm target get() takes a record".to_string());
                        };
                        let Expr::Str(key) = &args[1] else {
                            // Dynamic key: fields must share one type (checked above).
                            let kinds = self.checker.shape_kinds(shape)?;
                            let kind = kinds[0];
                            let field_ty = kind_ty(kind);
                            let (table, count) = self.record_table(shape)?;
                            let rec_tmp = self.builder.add_local(ValType::I32);
                            self.emit_expr(&args[0])?;
                            self.builder.push(Instr::Local(0x21, rec_tmp));
                            let key_ptr = self.builder.add_local(ValType::I32);
                            let key_len = self.builder.add_local(ValType::I32);
                            self.emit_str(&args[1])?;
                            self.builder.push(Instr::Local(0x21, key_len));
                            self.builder.push(Instr::Local(0x21, key_ptr));
                            let index = self.builder.add_local(ValType::I32);
                            let found = self.builder.add_local(ValType::I32);
                            let block = self.builder.add_local(ValType::I32);
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Local(0x21, index));
                            self.builder.push(Instr::Const(0x41, WVal::I32(-1)));
                            self.builder.push(Instr::Local(0x21, found));
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
                            self.builder.push(Instr::Local(0x20, index));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32(count as i32)));
                            self.builder.push(Instr::Op(0x4f));
                            self.builder.push(Instr::BrIf(1));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32(table as i32)));
                            self.builder.push(Instr::Local(0x20, index));
                            self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                            self.builder.push(Instr::Op(0x6c));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32(table as i32)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x21, block));
                            self.builder.push(Instr::Local(0x20, block));
                            self.builder.push(Instr::Const(0x41, WVal::I32(4)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x20, block));
                            self.builder.push(Instr::Mem(0x28, 2, 0));
                            self.builder.push(Instr::Local(0x20, key_ptr));
                            self.builder.push(Instr::Local(0x20, key_len));
                            self.builder.push(Instr::Call(STR_COMPARE));
                            self.builder.push(Instr::Op(0x45));
                            self.builder.push(Instr::Block {
                                opcode: 0x04,
                                end: 0,
                                else_at: 0,
                                start: 0,
                            });
                            self.labels.push(0x04);
                            self.builder.push(Instr::Local(0x20, index));
                            self.builder.push(Instr::Local(0x21, found));
                            self.builder.push(Instr::Br(2));
                            self.labels.pop();
                            self.builder.push(Instr::Op(0x0b));
                            self.builder.push(Instr::Local(0x20, index));
                            self.builder.push(Instr::Const(0x41, WVal::I32(1)));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x21, index));
                            self.builder.push(Instr::Br(0));
                            self.labels.pop();
                            self.builder.push(Instr::Op(0x0b));
                            self.labels.pop();
                            self.builder.push(Instr::Op(0x0b));
                            self.builder.push(Instr::Local(0x20, found));
                            self.builder.push(Instr::Const(0x41, WVal::I32(0)));
                            self.builder.push(Instr::Op(0x48));
                            let result_i64 = self.builder.add_local(ValType::I64);
                            let result_f64 = self.builder.add_local(ValType::F64);
                            let result_i32 = self.builder.add_local(ValType::I32);
                            let result_ptr = self.builder.add_local(ValType::I32);
                            let result_len = self.builder.add_local(ValType::I32);
                            self.builder.push(Instr::Block {
                                opcode: 0x04,
                                end: 0,
                                else_at: 0,
                                start: 0,
                            });
                            self.labels.push(0x04);
                            // Missing key: store the default (or abort).
                            match args.get(2) {
                                Some(default) => match field_ty {
                                    Ty::Str => {
                                        self.emit_str(default)?;
                                        self.builder.push(Instr::Local(0x21, result_len));
                                        self.builder.push(Instr::Local(0x21, result_ptr));
                                    }
                                    Ty::Float => {
                                        self.emit_f64(default)?;
                                        self.builder.push(Instr::Local(0x21, result_f64));
                                    }
                                    Ty::Bool => {
                                        self.emit_expr(default)?;
                                        self.builder.push(Instr::Local(0x21, result_i32));
                                    }
                                    _ => {
                                        self.emit_expr(default)?;
                                        self.builder.push(Instr::Local(0x21, result_i64));
                                    }
                                },
                                None => {
                                    self.emit_panic("get() missing key with no default");
                                }
                            }
                            self.builder.push(Instr::Else { end: 0 });
                            let addr = self.builder.add_local(ValType::I32);
                            self.builder.push(Instr::Local(0x20, rec_tmp));
                            self.builder.push(Instr::Local(0x20, found));
                            self.builder.push(Instr::Const(0x41, WVal::I32(8)));
                            self.builder.push(Instr::Op(0x6c));
                            self.builder.push(Instr::Op(0x6a));
                            self.builder.push(Instr::Local(0x21, addr));
                            match field_ty {
                                Ty::Str => {
                                    self.builder.push(Instr::Local(0x20, addr));
                                    self.builder.push(Instr::Mem(0x28, 2, 0));
                                    self.builder.push(Instr::Local(0x21, result_ptr));
                                    self.builder.push(Instr::Local(0x20, addr));
                                    self.builder.push(Instr::Mem(0x28, 2, 4));
                                    self.builder.push(Instr::Local(0x21, result_len));
                                }
                                Ty::Float => {
                                    self.builder.push(Instr::Local(0x20, addr));
                                    self.builder.push(Instr::Mem(0x2b, 3, 0));
                                    self.builder.push(Instr::Local(0x21, result_f64));
                                }
                                Ty::Bool => {
                                    self.builder.push(Instr::Local(0x20, addr));
                                    self.builder.push(Instr::Mem(0x28, 2, 0));
                                    self.builder.push(Instr::Local(0x21, result_i32));
                                }
                                _ => {
                                    self.builder.push(Instr::Local(0x20, addr));
                                    self.builder.push(Instr::Mem(0x29, 3, 0));
                                    self.builder.push(Instr::Local(0x21, result_i64));
                                }
                            }
                            self.labels.pop();
                            self.builder.push(Instr::Op(0x0b));
                            match field_ty {
                                Ty::Str => {
                                    self.builder.push(Instr::Local(0x20, result_ptr));
                                    self.builder.push(Instr::Local(0x20, result_len));
                                }
                                Ty::Float => {
                                    self.builder.push(Instr::Local(0x20, result_f64));
                                }
                                Ty::Bool => {
                                    self.builder.push(Instr::Local(0x20, result_i32));
                                }
                                _ => {
                                    self.builder.push(Instr::Local(0x20, result_i64));
                                }
                            }
                            return Ok(field_ty);
                        };
                        let rec_tmp = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Local(0x21, rec_tmp));
                        let keys = self.checker.shape_keys(shape)?;
                        let Some(position) = keys.iter().position(|known| known == key) else {
                            match args.get(2) {
                                Some(default) => return self.emit_expr(default),
                                None => {
                                    self.emit_panic("get() missing key with no default");
                                    return Ok(Ty::Int);
                                }
                            }
                        };
                        let kind = self.checker.shape_kinds(shape)?[position];
                        let offset = (8 * position) as u32;
                        match kind {
                            1 => {
                                self.builder.push(Instr::Local(0x20, rec_tmp));
                                self.builder.push(Instr::Mem(0x28, 2, offset));
                                self.builder.push(Instr::Local(0x21, self.temp(PTR_A)));
                                self.builder.push(Instr::Local(0x20, rec_tmp));
                                self.builder.push(Instr::Mem(0x28, 2, offset + 4));
                                self.builder.push(Instr::Local(0x21, self.temp(LEN_A)));
                                self.builder.push(Instr::Local(0x20, self.temp(PTR_A)));
                                self.builder.push(Instr::Local(0x20, self.temp(LEN_A)));
                                return Ok(Ty::Str);
                            }
                            2 => {
                                self.builder.push(Instr::Local(0x20, rec_tmp));
                                self.builder.push(Instr::Mem(0x2b, 3, offset));
                                return Ok(Ty::Float);
                            }
                            3 => {
                                self.builder.push(Instr::Local(0x20, rec_tmp));
                                self.builder.push(Instr::Mem(0x28, 2, offset));
                                return Ok(Ty::Bool);
                            }
                            _ => {
                                self.builder.push(Instr::Local(0x20, rec_tmp));
                                self.builder.push(Instr::Mem(0x29, 3, offset));
                                return Ok(Ty::Int);
                            }
                        }
                    }
                    if name == "merge" && args.len() == 2 {
                        let Ty::Obj(union) = self.type_of(expr)? else {
                            return Err("wasm target merge() takes two records".to_string());
                        };
                        let Ty::Obj(left_shape) = self.type_of(&args[0])? else {
                            return Err("wasm target merge() takes two records".to_string());
                        };
                        let Ty::Obj(right_shape) = self.type_of(&args[1])? else {
                            return Err("wasm target merge() takes two records".to_string());
                        };
                        let union_keys = self.checker.shape_keys(union)?;
                        let left_keys = self.checker.shape_keys(left_shape)?;
                        let right_keys = self.checker.shape_keys(right_shape)?;
                        let dest = self.builder.add_local(ValType::I32);
                        self.emit_alloc_const(8 * union_keys.len() as u32);
                        self.builder.push(Instr::Local(0x21, dest));
                        for (index, key) in union_keys.iter().enumerate() {
                            let (argument, offset) = if let Some(at) =
                                right_keys.iter().position(|known| known == key)
                            {
                                (1usize, 8 * at as u32)
                            } else {
                                let at = left_keys
                                    .iter()
                                    .position(|known| known == key)
                                    .ok_or_else(|| "merge key is missing".to_string())?;
                                (0usize, 8 * at as u32)
                            };
                            self.builder.push(Instr::Local(0x20, dest));
                            self.builder
                                .push(Instr::Const(0x41, WVal::I32((8 * index) as i32)));
                            self.builder.push(Instr::Op(0x6a));
                            self.emit_expr(&args[argument])?;
                            self.builder.push(Instr::Mem(0x29, 3, offset));
                            self.builder.push(Instr::Mem(0x37, 3, 0));
                        }
                        self.builder.push(Instr::Local(0x20, dest));
                        return Ok(Ty::Obj(union));
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
                            Ty::Bool => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Op(0xad));
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
                            Ty::Map => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Call(MAP_LEN));
                                self.builder.push(Instr::Op(0xad));
                                return Ok(Ty::Int);
                            }
                            Ty::Arr
                            | Ty::ArrStr
                            | Ty::ArrObj(_)
                            | Ty::ArrArr
                            | Ty::ArrF
                            | Ty::ArrStrArr => {
                                self.emit_expr(&args[0])?;
                                self.builder.push(Instr::Mem(0x28, 2, 0));
                                self.builder.push(Instr::Op(0xad));
                                return Ok(Ty::Int);
                            }
                            Ty::Str => {
                                self.emit_str(&args[0])?;
                                self.builder.push(Instr::Call(STR_LEN));
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
                match &**callee {
                    Expr::Ident(name) => Err(format!(
                        "wasm target does not support {}() with {} argument(s)",
                        name,
                        args.len()
                    )),
                    _ => Err("wasm target does not support this call yet".to_string()),
                }
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
                        Ty::Arr
                        | Ty::ArrStr
                        | Ty::ArrObj(_)
                        | Ty::ArrArr
                        | Ty::ArrF
                        | Ty::ArrStrArr
                        | Ty::Obj(_)
                        | Ty::Map
                        | Ty::Func(_)
                        | Ty::Bool => self.builder.add_local(ValType::I32),
                    };
                    if self.boxed.contains(name) {
                        self.box_local(name, ty)?;
                        return Ok(());
                    }
                    self.locals.insert(
                        name.clone(),
                        LocalSlot {
                            ty,
                            slot,
                            boxed: false,
                            capture: None,
                        },
                    );
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
                if let Expr::Member(object, key, _) = &**target {
                    let Ty::Obj(shape) = self.type_of(object)? else {
                        return Err("wasm target writes record fields only".to_string());
                    };
                    let index = self.checker.field_index(shape, key)?;
                    let kind = self.checker.shape_kinds(shape)?[index];
                    let offset = (8 * index) as u32;
                    let rec_tmp = self.builder.add_local(ValType::I32);
                    self.emit_expr(object)?;
                    self.builder.push(Instr::Local(0x21, rec_tmp));
                    match kind {
                        1 => {
                            let ptr_tmp = self.builder.add_local(ValType::I32);
                            let len_tmp = self.builder.add_local(ValType::I32);
                            self.emit_str(value)?;
                            self.builder.push(Instr::Local(0x21, len_tmp));
                            self.builder.push(Instr::Local(0x21, ptr_tmp));
                            self.builder.push(Instr::Local(0x20, rec_tmp));
                            self.builder.push(Instr::Local(0x20, ptr_tmp));
                            self.builder.push(Instr::Mem(0x36, 2, offset));
                            self.builder.push(Instr::Local(0x20, rec_tmp));
                            self.builder.push(Instr::Local(0x20, len_tmp));
                            self.builder.push(Instr::Mem(0x36, 2, offset + 4));
                        }
                        2 => {
                            self.builder.push(Instr::Local(0x20, rec_tmp));
                            self.emit_f64(value)?;
                            self.builder.push(Instr::Mem(0x39, 3, offset));
                        }
                        3 => {
                            self.builder.push(Instr::Local(0x20, rec_tmp));
                            self.emit_expr(value)?;
                            self.builder.push(Instr::Op(0xad));
                            self.builder.push(Instr::Mem(0x37, 3, offset));
                        }
                        _ => {
                            self.builder.push(Instr::Local(0x20, rec_tmp));
                            self.emit_expr(value)?;
                            self.builder.push(Instr::Mem(0x37, 3, offset));
                        }
                    }
                    return Ok(());
                }
                if !matches!(**target, Expr::Ident(_)) {
                    if matches!(**target, Expr::Index(..)) {
                        if let Expr::Index(array, _, _) = &**target {
                            if self.type_of(array)? == Ty::ArrStr {
                                let address = self.builder.add_local(ValType::I32);
                                let ptr_tmp = self.builder.add_local(ValType::I32);
                                let len_tmp = self.builder.add_local(ValType::I32);
                                self.emit_target_address(target)?;
                                self.builder.push(Instr::Local(0x21, address));
                                self.emit_str(value)?;
                                self.builder.push(Instr::Local(0x21, len_tmp));
                                self.builder.push(Instr::Local(0x21, ptr_tmp));
                                self.builder.push(Instr::Local(0x20, address));
                                self.builder.push(Instr::Local(0x20, ptr_tmp));
                                self.builder.push(Instr::Mem(0x36, 2, 0));
                                self.builder.push(Instr::Local(0x20, address));
                                self.builder.push(Instr::Local(0x20, len_tmp));
                                self.builder.push(Instr::Mem(0x36, 2, 4));
                                return Ok(());
                            }
                        }
                    }
                    if let Expr::Index(array, _, _) = &**target {
                        let array_ty = self.type_of(array)?;
                        if array_ty == Ty::ArrF {
                            let address = self.builder.add_local(ValType::I32);
                            self.emit_target_address(target)?;
                            self.builder.push(Instr::Local(0x21, address));
                            self.builder.push(Instr::Local(0x20, address));
                            self.emit_f64(value)?;
                            self.builder.push(Instr::Mem(0x39, 3, 0));
                            return Ok(());
                        }
                        if matches!(array_ty, Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrStrArr) {
                            let address = self.builder.add_local(ValType::I32);
                            self.emit_target_address(target)?;
                            self.builder.push(Instr::Local(0x21, address));
                            self.builder.push(Instr::Local(0x20, address));
                            let ty = self.emit_expr(value)?;
                            let ok = match array_ty {
                                Ty::ArrArr => ty == Ty::Arr,
                                Ty::ArrStrArr => ty == Ty::ArrStr,
                                _ => matches!(ty, Ty::Obj(_)),
                            };
                            if !ok {
                                return Err(
                                    "wasm target nested arrays hold int arrays only".to_string()
                                );
                            }
                            self.builder.push(Instr::Mem(0x36, 2, 0));
                            return Ok(());
                        }
                    }
                    self.emit_target_address(target)?;
                    let ty = self.emit_expr(value)?;
                    if ty != Ty::Int {
                        return Err("wasm target records and arrays hold ints".to_string());
                    }
                    self.builder.push(Instr::Mem(0x37, 3, 0));
                    return Ok(());
                }
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
            Stmt::Expr(expr @ Expr::Call(callee, args, _), _) => {
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
                    "attempt" => {
                        self.emit_attempt(&args[0])?;
                        self.builder.push(Instr::Op(0x1a));
                        return Ok(());
                    }
                    "exec" | "sh" => {
                        let shell = name == "sh";
                        self.emit_exec_record(args, shell)?;
                        self.builder.push(Instr::Op(0x1a));
                        return Ok(());
                    }
                    "push" => {
                        let Expr::Ident(array) = &args[0] else {
                            return Err(
                                "wasm target pushes into a named array variable only".to_string()
                            );
                        };
                        self.emit_push(array, &args[1])?;
                        return Ok(());
                    }
                    "pop" => {
                        let Expr::Ident(array) = &args[0] else {
                            return Err("wasm target pops a named array variable only".to_string());
                        };
                        self.emit_pop(array)?;
                        return Ok(());
                    }
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
                    "reverse" | "sort" | "slice" | "join" | "sum" | "min" | "max" | "index_of"
                    | "map" | "filter" | "reduce" | "range" | "keys" | "values" | "split"
                    | "trim" | "upper" | "lower" | "replace" | "repeat" | "str" | "int" => {
                        let ty = self.emit_expr(expr)?;
                        match ty {
                            Ty::Str => {
                                self.builder.push(Instr::Op(0x1a));
                                self.builder.push(Instr::Op(0x1a));
                            }
                            _ => self.builder.push(Instr::Op(0x1a)),
                        }
                        return Ok(());
                    }
                    "each" => {
                        if args.len() != 2 {
                            return Err(
                                "wasm target each() takes an array and a callback".to_string()
                            );
                        }
                        let Ty::Func(id) = self.type_of(&args[1])? else {
                            return Err(
                                "wasm target each() needs a function literal callback".to_string()
                            );
                        };
                        let sig = self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .cloned()
                            .ok_or_else(|| "wasm target lost a function literal".to_string())?;
                        let cb_env = self.builder.add_local(ValType::I32);
                        self.emit_expr(&args[1])?;
                        self.builder.push(Instr::Local(0x21, cb_env));
                        let array_tmp = self.builder.add_local(ValType::I32);
                        let index_tmp = self.builder.add_local(ValType::I64);
                        self.emit_expr(&args[0])?;
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
                        self.builder.push(Instr::Local(0x20, index_tmp));
                        self.builder.push(Instr::Local(0x20, array_tmp));
                        self.builder.push(Instr::Mem(0x28, 2, 0));
                        self.builder.push(Instr::Op(0xad));
                        self.builder.push(Instr::Op(0x53));
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::BrIf(1));
                        let input_ty = self.type_of(&args[0])?;
                        let elem = match input_ty {
                            Ty::Arr => Ty::Int,
                            Ty::ArrF => Ty::Float,
                            Ty::ArrStr => Ty::Str,
                            Ty::ArrObj(shape) => Ty::Obj(shape),
                            Ty::ArrArr => Ty::Arr,
                            Ty::ArrStrArr => Ty::ArrStr,
                            other => {
                                return Err(format!(
                                    "wasm target each() takes an array, got {}",
                                    ty_name(other)
                                ))
                            }
                        };
                        self.builder.push(Instr::Local(0x20, cb_env));
                        Self::emit_load_slot(self, array_tmp, index_tmp, elem);
                        self.builder.push(Instr::Call(sig.index));
                        match sig.result {
                            Some(Ty::Str) => {
                                self.builder.push(Instr::Op(0x1a));
                                self.builder.push(Instr::Op(0x1a));
                            }
                            Some(_) => self.builder.push(Instr::Op(0x1a)),
                            None => {}
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
                        if !self.attempts.is_empty() {
                            self.emit_str(&args[0])?;
                            self.builder.push(Instr::Call(MKDIR_SOFT));
                            self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                            self.emit_soft_io_check(self.temp(ARG_TMP));
                            return Ok(());
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(MKDIR_SOFT));
                        self.builder.push(Instr::Local(0x21, self.temp(ARG_TMP)));
                        self.emit_soft_io_panic(self.temp(ARG_TMP));
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
                    "exit" => {
                        self.emit_expr(&args[0])?;
                        self.builder.push(Instr::Op(0xa7));
                        self.builder.push(Instr::Call(EXIT));
                        return Ok(());
                    }
                    "error" => {
                        if let Some(ctx) = self.attempts.last().copied() {
                            self.emit_str(&args[0])?;
                            self.emit_attempt_error_pair(ctx.record);
                            self.emit_attempt_jump();
                            return Ok(());
                        }
                        self.emit_str(&args[0])?;
                        self.builder.push(Instr::Call(ERROR));
                        return Ok(());
                    }
                    "assert" => {
                        self.emit_cond(&args[0])?;
                        self.builder.push(Instr::Op(0x45));
                        self.builder.push(Instr::Block {
                            opcode: 0x04,
                            end: 0,
                            else_at: 0,
                            start: 0,
                        });
                        self.labels.push(0x04);
                        match args.get(1) {
                            Some(message) => self.emit_str(message)?,
                            None => self.emit_str(&Expr::Str("assertion failed".to_string()))?,
                        }
                        self.builder.push(Instr::Call(ERROR));
                        self.labels.pop();
                        self.builder.push(Instr::Op(0x0b));
                        return Ok(());
                    }
                    "print" | "console_log" => {}
                    _ => {
                        // Any other call is an expression statement: evaluate it
                        // and drop the value (the VM allows this everywhere).
                        self.emit_expr(expr)?;
                        self.builder.push(Instr::Op(0x1a));
                        return Ok(());
                    }
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
                    Ty::Arr => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        self.builder.push(Instr::Call(PRINT_ARR_I64));
                    }
                    Ty::ArrStr => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        self.builder.push(Instr::Call(PRINT_ARR_STR));
                    }
                    Ty::Obj(shape) => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        let value = self.builder.add_local(ValType::I32);
                        self.builder.push(Instr::Local(0x21, value));
                        let (table, count) = self.record_table(shape)?;
                        let kinds = self.record_kinds_table(shape)?;
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(kinds as i32)));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(table as i32)));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Local(0x20, value));
                        self.builder.push(Instr::Call(PRINT_RECORD_TYPED));
                    }
                    Ty::ArrStrArr => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        self.builder.push(Instr::Call(PRINT_ARR_STRARR));
                    }
                    Ty::ArrF => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        self.builder.push(Instr::Call(PRINT_ARR_F64));
                    }
                    Ty::ArrArr => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        self.builder.push(Instr::Call(PRINT_ARR_ARR));
                    }
                    Ty::Map => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        self.builder.push(Instr::Call(PRINT_MAP));
                    }
                    Ty::ArrObj(shape) => {
                        if web {
                            return Err("wasm target console_log prints scalars only".to_string());
                        }
                        let value = self.builder.add_local(ValType::I32);
                        self.builder.push(Instr::Local(0x21, value));
                        let (table, count) = self.record_table(shape)?;
                        let kinds = self.record_kinds_table(shape)?;
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(kinds as i32)));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(table as i32)));
                        self.builder
                            .push(Instr::Const(0x41, WVal::I32(count as i32)));
                        self.builder.push(Instr::Local(0x20, value));
                        self.builder.push(Instr::Call(PRINT_ARR_OBJ_TYPED));
                    }
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
                if !matches!(**target, Expr::Ident(_)) {
                    let address = self.builder.add_local(ValType::I32);
                    self.emit_target_address(target)?;
                    self.builder.push(Instr::Local(0x21, address));
                    self.builder.push(Instr::Local(0x20, address));
                    self.builder.push(Instr::Mem(0x29, 3, 0));
                    let ty = self.emit_expr(value)?;
                    if ty != Ty::Int {
                        return Err("wasm target records and arrays hold ints".to_string());
                    }
                    let opcode = match op {
                        BinOp::Add => 0x7c,
                        BinOp::Sub => 0x7d,
                        BinOp::Mul => 0x7e,
                        BinOp::Mod => 0x81,
                        _ => {
                            return Err("wasm target compound-assigns with + - * % only".to_string())
                        }
                    };
                    self.builder.push(Instr::Op(opcode));
                    let result = self.builder.add_local(ValType::I64);
                    self.builder.push(Instr::Local(0x21, result));
                    self.builder.push(Instr::Local(0x20, address));
                    self.builder.push(Instr::Local(0x20, result));
                    self.builder.push(Instr::Mem(0x37, 3, 0));
                    return Ok(());
                }
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
                    Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrF | Ty::ArrStrArr => {
                        self.builder.push(Instr::Op(0x1a));
                    }
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
                    | Ty::Bool
                    | Ty::ArrStr => self.builder.push(Instr::Op(0x1a)),
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
                if let Some(ctx) = self.attempts.last().copied() {
                    if let Some(value) = expr {
                        self.emit_attempt_value(ctx.record, ctx.result_ty, value)?;
                    }
                    self.emit_attempt_jump();
                    return Ok(());
                }
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
                let Some(target) = self.continue_targets.last().copied() else {
                    return Err("wasm target has no loop to continue".to_string());
                };
                let depth = self.labels.len() - 1 - target;
                self.builder.push(Instr::Br(depth as u32));
                Ok(())
            }
            other => Err(format!(
                "wasm target does not support this statement yet: {}",
                stmt_kind(other)
            )),
        }
    }
}

struct FnDecl<'a> {
    name: String,
    params: Vec<String>,
    body: &'a [Stmt],
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
    fn shape_id(&self, keys: &[String], kinds: &[u8]) -> u32 {
        let mut shapes = self.shapes.borrow_mut();
        if let Some(index) = shapes
            .iter()
            .position(|shape| shape.keys == keys && shape.kinds == kinds)
        {
            return index as u32;
        }
        shapes.push(Shape {
            keys: keys.to_vec(),
            kinds: kinds.to_vec(),
        });
        (shapes.len() - 1) as u32
    }

    fn field_ty(&self, shape: u32, key: &str) -> Result<Ty, String> {
        let shapes = self.shapes.borrow();
        let shape = shapes
            .get(shape as usize)
            .ok_or_else(|| "unknown object shape".to_string())?;
        let index = shape
            .keys
            .iter()
            .position(|known| known == key)
            .ok_or_else(|| format!("object has no field '{}'", key))?;
        Ok(kind_ty(shape.kinds[index]))
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
                if *op == BinOp::Add && (lt == Ty::Str || rt == Ty::Str) {
                    let other = if lt == Ty::Str { rt } else { lt };
                    if matches!(other, Ty::Str | Ty::Int | Ty::Float | Ty::Bool) {
                        return Some(Ty::Str);
                    }
                }
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
                let mut saw_str = false;
                let mut saw_arr = false;
                let mut saw_strarr = false;
                let mut saw_float = false;
                let mut saw_int = false;
                let mut obj_shape: Option<u32> = None;
                for item in items {
                    match self.soft(item, locals)? {
                        Ty::Str => saw_str = true,
                        Ty::Arr => saw_arr = true,
                        Ty::ArrStr => saw_strarr = true,
                        Ty::Float => saw_float = true,
                        Ty::Int => saw_int = true,
                        Ty::Obj(shape) => obj_shape = Some(shape),
                        _ => {}
                    }
                }
                let _ = saw_int;
                Some(if saw_strarr {
                    Ty::ArrStrArr
                } else {
                    match obj_shape {
                        Some(shape) => Ty::ArrObj(shape),
                        None if saw_str => Ty::ArrStr,
                        None if saw_arr => Ty::ArrArr,
                        None if saw_float => Ty::ArrF,
                        None => Ty::Arr,
                    }
                })
            }
            Expr::Object(fields) => {
                let mut keys = Vec::new();
                let mut kinds = Vec::new();
                for (key, value) in fields {
                    let ty = self.soft(value, locals);
                    keys.push(key.clone());
                    kinds.push(ty.and_then(field_kind).unwrap_or(0));
                }
                Some(Ty::Obj(self.shape_id(&keys, &kinds)))
            }
            Expr::Member(object, key, _) => {
                if let Some(Ty::Obj(shape)) = self.soft(object, locals) {
                    if let Ok(ty) = self.field_ty(shape, key) {
                        return Some(ty);
                    }
                }
                Some(Ty::Int)
            }
            Expr::Index(array, index, _) => {
                let base = self.soft(array, locals)?;
                self.soft(index, locals)?;
                Some(match base {
                    Ty::ArrStr => Ty::Str,
                    Ty::ArrObj(shape) => Ty::Obj(shape),
                    Ty::ArrArr => Ty::Arr,
                    Ty::ArrStrArr => Ty::ArrStr,
                    Ty::ArrF => Ty::Float,
                    _ => Ty::Int,
                })
            }
            Expr::Call(callee, args, _) => {
                if let Expr::Ident(name) = &**callee {
                    if name == "attempt" && args.len() == 1 {
                        if let Some(Ty::Func(id)) =
                            args.first().and_then(|arg| self.soft(arg, locals))
                        {
                            if let Some(result_ty) = Some(
                                self.results
                                    .get(self.decl_count + id as usize)
                                    .copied()
                                    .flatten()
                                    .unwrap_or(Ty::Int),
                            )
                            .filter(|ty| field_kind(*ty).is_some())
                            {
                                let kind = field_kind(result_ty).unwrap();
                                return Some(Ty::Obj(self.shape_id(
                                    &["error".to_string(), "ok".to_string(), "value".to_string()],
                                    &[1, 3, kind],
                                )));
                            }
                        }
                        return None;
                    }
                    if matches!(name.as_str(), "exec" | "sh") {
                        self.soft(&args[0], locals)?;
                        let (keys, kinds) = exec_result_keys();
                        return Some(Ty::Obj(self.shape_id(&keys, &kinds)));
                    }
                }
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
                    "len" | "argc" | "write_file" | "write" | "run" | "gl_pixel" | "gl_png"
                    | "int" | "abs" | "floor" | "ceil" | "round" | "sum" | "get" | "now"
                    | "sleep" | "mkdir" => Some(Ty::Int),
                    "reduce" => args
                        .get(2)
                        .and_then(|init| self.soft(init, locals))
                        .or_else(|| {
                            args.first()
                                .and_then(|arg| self.soft(arg, locals))
                                .map(|ty| match ty {
                                    Ty::ArrF => Ty::Float,
                                    Ty::ArrStr => Ty::Str,
                                    _ => Ty::Int,
                                })
                        }),
                    "str" | "type" | "arg" | "read_file" | "read" | "which" | "capture" | "cwd"
                    | "env" | "join" | "slice" | "trim" | "upper" | "lower" | "replace"
                    | "repeat" => Some(Ty::Str),
                    "float" | "sqrt" => Some(Ty::Float),
                    "exists" | "file_exists" | "has" => Some(Ty::Bool),
                    "split" | "keys" => Some(Ty::ArrStr),
                    "range" | "values" => Some(Ty::Arr),
                    "reverse" | "sort" => args.first().and_then(|arg| self.soft(arg, locals)),
                    "map" => {
                        if let Some(Ty::Func(id)) =
                            args.get(1).and_then(|arg| self.soft(arg, locals))
                        {
                            match self
                                .results
                                .get(self.decl_count + id as usize)
                                .copied()
                                .flatten()
                            {
                                Some(Ty::Str) => Some(Ty::ArrStr),
                                Some(Ty::ArrStr) => Some(Ty::ArrStrArr),
                                Some(Ty::Float) => Some(Ty::ArrF),
                                Some(Ty::Obj(shape)) => Some(Ty::ArrObj(shape)),
                                Some(Ty::Arr) => Some(Ty::ArrArr),
                                _ => Some(Ty::Arr),
                            }
                        } else {
                            Some(Ty::Arr)
                        }
                    }
                    "filter" => args.first().and_then(|arg| self.soft(arg, locals)),
                    _ => {
                        for arg in args {
                            self.soft(arg, locals)?;
                        }
                        None
                    }
                }
            }
            Expr::MethodCall(object, key, args, _) => {
                self.soft(object, locals)?;
                for arg in args {
                    self.soft(arg, locals)?;
                }
                match key.as_str() {
                    "len" => Some(Ty::Int),
                    "contains" | "starts_with" | "ends_with" => Some(Ty::Bool),
                    "trim" | "upper" | "lower" | "slice" | "replace" => Some(Ty::Str),
                    "split" => Some(Ty::ArrStr),
                    "index_of" => Some(Ty::Int),
                    "repeat" => Some(Ty::Str),
                    _ => None,
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
                // The fixpoint starts array elements at `int` before a callback's
                // return type is known (for example `map(range(n), fn(i) { ... })`
                // feeding `sort`); a later, more specific guess must win over that
                // provisional int, and a provisional int must not override a
                // specific type. Genuine conflicts still fail in the final check.
                if *current == Ty::Int && !is_number(ty) {
                    *current = ty;
                    return Ok(true);
                }
                if ty == Ty::Int && !is_number(*current) {
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
                    if let Some(init) = init {
                        changed |= self.walk_expr(init, locals)?;
                    }
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
                Stmt::ForIn(key, value, iter, body, _) => {
                    changed |= self.walk_expr(iter, locals)?;
                    let (key_ty, value_ty) = match self.soft(iter, locals) {
                        Some(Ty::ArrStr) => (Ty::Int, Ty::Str),
                        Some(Ty::Obj(_)) | Some(Ty::Map) => (Ty::Str, Ty::Int),
                        _ => (Ty::Int, Ty::Int),
                    };
                    let mut inner = locals.clone();
                    match value {
                        Some(value) => {
                            inner.insert(key.clone(), key_ty);
                            inner.insert(value.clone(), value_ty);
                        }
                        None => {
                            inner.insert(key.clone(), value_ty);
                        }
                    }
                    changed |= self.walk(body, &mut inner, fn_index)?;
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
                    if let Some(expr) = expr {
                        changed |= self.walk_expr(expr, locals)?;
                    }
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
                    // Comparator sort: unify the literal comparator with the element type.

                    if name == "sort" && args.len() == 2 {
                        if let Some(Ty::Func(id)) =
                            args.get(1).and_then(|arg| self.soft(arg, locals))
                        {
                            let index = self.decl_count + id as usize;
                            let elem = match args.first().and_then(|arg| self.soft(arg, locals)) {
                                Some(Ty::ArrStr) => Ty::Str,
                                Some(Ty::ArrF) => Ty::Float,
                                Some(Ty::ArrObj(shape)) => Ty::Obj(shape),
                                Some(Ty::ArrArr) => Ty::Arr,
                                Some(Ty::ArrStrArr) => Ty::ArrStr,
                                _ => Ty::Int,
                            };
                            changed |= self.unify_param(index, 0, elem)?;
                            changed |= self.unify_param(index, 1, elem)?;
                            changed |= self.unify_result(index, Ty::Int)?;
                            for arg in args {
                                changed |= self.walk_expr(arg, locals)?;
                            }
                            return Ok(changed);
                        }
                    } // Higher-order array helpers: unify the literal callback.
                    if matches!(name.as_str(), "map" | "filter" | "each" | "reduce") {
                        if let Some(Ty::Func(id)) =
                            args.get(1).and_then(|arg| self.soft(arg, locals))
                        {
                            let index = self.decl_count + id as usize;
                            let elem = match args.first().and_then(|arg| self.soft(arg, locals)) {
                                Some(Ty::ArrStr) => Ty::Str,
                                Some(Ty::ArrF) => Ty::Float,
                                Some(Ty::ArrObj(shape)) => Ty::Obj(shape),
                                Some(Ty::ArrArr) => Ty::Arr,
                                Some(Ty::ArrStrArr) => Ty::ArrStr,
                                _ => Ty::Int,
                            };
                            match name.as_str() {
                                "reduce" => {
                                    let acc = match args.get(2) {
                                        Some(init) => self.soft(init, locals).unwrap_or(elem),
                                        None => elem,
                                    };
                                    changed |= self.unify_param(index, 0, acc)?;
                                    changed |= self.unify_param(index, 1, elem)?;
                                    changed |= self.unify_result(index, acc)?;
                                }
                                "filter" => {
                                    changed |= self.unify_param(index, 0, elem)?;
                                    changed |= self.unify_result(index, Ty::Bool)?;
                                }
                                "map" | "each" => {
                                    changed |= self.unify_param(index, 0, elem)?;
                                }
                                _ => {}
                            }
                            for arg in args {
                                changed |= self.walk_expr(arg, locals)?;
                            }
                            return Ok(changed);
                        }
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
                if !matches!(&**callee, Expr::Ident(_)) {
                    changed |= self.walk_expr(callee, locals)?;
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

/// Which function frame owns a binding during capture analysis.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CaptureFrame {
    Start,
    Fn(usize),
    Lit(u32),
}

/// How a capturing literal obtains the *cell pointer* for one capture at its
/// creation site: from a boxed local of the immediately enclosing frame, or
/// from the enclosing literal's environment block.
#[derive(Clone, Copy)]
enum CaptureSource {
    Local,
    Env(u32),
}

#[derive(Clone)]
struct Capture {
    name: String,
    ty: Ty,
    source: CaptureSource,
}

#[derive(Default)]
struct CaptureAnalysis {
    /// Captures per literal id, in environment order.
    lit_captures: Vec<Vec<Capture>>,
    /// Literal id -> names that must live in heap cells (captured by an inner literal).
    lit_boxed: HashMap<u32, std::collections::HashSet<String>>,
    /// Named-function slot -> names that must live in heap cells.
    fn_boxed: HashMap<usize, std::collections::HashSet<String>>,
    /// Top-level (start function) loop variables captured by a literal.
    start_boxed: std::collections::HashSet<String>,
}

#[derive(Clone)]
struct FrameBinding {
    ty: Ty,
    loop_var: bool,
}

struct FrameState {
    id: CaptureFrame,
    scopes: Vec<HashMap<String, FrameBinding>>,
    captures: Vec<Capture>,
    boxed: std::collections::HashSet<String>,
}

struct CaptureWalker<'a> {
    checker: &'a Checker,
    index: &'a HashMap<String, usize>,
    literal_ids: &'a HashMap<usize, u32>,
    lit_params: Vec<Vec<String>>,
    frames: Vec<FrameState>,
    globals: HashMap<String, Ty>,
    analysis: CaptureAnalysis,
}

impl<'a> CaptureWalker<'a> {
    fn new(
        checker: &'a Checker,
        index: &'a HashMap<String, usize>,
        literal_ids: &'a HashMap<usize, u32>,
    ) -> Self {
        let literal_count = checker.lit_sigs.len();
        CaptureWalker {
            checker,
            index,
            literal_ids,
            lit_params: vec![Vec::new(); literal_count],
            frames: Vec::new(),
            globals: HashMap::new(),
            analysis: CaptureAnalysis {
                lit_captures: vec![Vec::new(); literal_count],
                lit_boxed: HashMap::new(),
                fn_boxed: HashMap::new(),
                start_boxed: std::collections::HashSet::new(),
            },
        }
    }

    fn frame(&mut self) -> &mut FrameState {
        self.frames
            .last_mut()
            .expect("capture frame stack is never empty")
    }

    fn binding_in(&self, frame: usize, name: &str) -> Option<FrameBinding> {
        for scope in self.frames[frame].scopes.iter().rev() {
            if let Some(binding) = scope.get(name) {
                return Some(binding.clone());
            }
        }
        None
    }

    fn lookup_ty(&self, name: &str) -> Option<Ty> {
        for frame in (0..self.frames.len()).rev() {
            if let Some(binding) = self.binding_in(frame, name) {
                return Some(binding.ty);
            }
        }
        if let Some(ty) = self.globals.get(name) {
            return Some(*ty);
        }
        if let Ok(ty) = self.checker.lookup(name) {
            return Some(ty);
        }
        None
    }

    fn shape_id(&self, keys: &[String], kinds: &[u8]) -> u32 {
        let mut shapes = self.checker.shapes.borrow_mut();
        if let Some(index) = shapes
            .iter()
            .position(|shape| shape.keys == keys && shape.kinds == kinds)
        {
            return index as u32;
        }
        shapes.push(Shape {
            keys: keys.to_vec(),
            kinds: kinds.to_vec(),
        });
        (shapes.len() - 1) as u32
    }

    /// Best-effort static type used only to give captured bindings a wasm type.
    fn ty_of(&self, expr: &Expr) -> Ty {
        match expr {
            Expr::Null => Ty::Int,
            Expr::Bool(_) => Ty::Bool,
            Expr::Int(_) => Ty::Int,
            Expr::Float(_) => Ty::Float,
            Expr::Str(_) => Ty::Str,
            Expr::Ident(name) => self.lookup_ty(name).unwrap_or(Ty::Int),
            Expr::Array(items) => {
                if items.iter().any(|item| matches!(item, Expr::Str(_))) {
                    return Ty::ArrStr;
                }
                if items.iter().all(|item| self.ty_of(item) == Ty::Arr) {
                    return Ty::ArrArr;
                }
                let shapes: Vec<Option<u32>> = items
                    .iter()
                    .map(|item| match self.ty_of(item) {
                        Ty::Obj(shape) => Some(shape),
                        _ => None,
                    })
                    .collect();
                if let Some(Some(shape)) = shapes.first() {
                    if shapes.iter().all(|known| *known == Some(*shape)) {
                        return Ty::ArrObj(*shape);
                    }
                }
                Ty::Arr
            }
            Expr::Object(fields) => {
                let keys: Vec<String> = fields.iter().map(|(key, _)| key.clone()).collect();
                let kinds: Vec<u8> = fields
                    .iter()
                    .map(|(_, value)| field_kind(self.ty_of(value)).unwrap_or(0))
                    .collect();
                Ty::Obj(self.shape_id(&keys, &kinds))
            }
            Expr::Neg(inner) => match self.ty_of(inner) {
                Ty::Float => Ty::Float,
                _ => Ty::Int,
            },
            Expr::Not(_) => Ty::Bool,
            Expr::And(..) | Expr::Or(..) => Ty::Bool,
            Expr::Bin(op, left, right) => {
                let lt = self.ty_of(left);
                let rt = self.ty_of(right);
                if matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                ) {
                    return Ty::Bool;
                }
                if *op == BinOp::Add {
                    if lt == Ty::Str || rt == Ty::Str {
                        return Ty::Str;
                    }
                    if matches!(lt, Ty::Str | Ty::Int | Ty::Float | Ty::Bool) {
                        return Ty::Str;
                    }
                }
                if *op == BinOp::Div || lt == Ty::Float || rt == Ty::Float {
                    Ty::Float
                } else {
                    Ty::Int
                }
            }
            Expr::Ternary(_, then, other, _) => {
                let a = self.ty_of(then);
                let b = self.ty_of(other);
                if a == Ty::Str || b == Ty::Str {
                    Ty::Str
                } else if a == Ty::ArrStr || b == Ty::ArrStr {
                    Ty::ArrStr
                } else if a == Ty::Float || b == Ty::Float {
                    Ty::Float
                } else {
                    Ty::Int
                }
            }
            Expr::MethodCall(object, key, _, _) => match key.as_str() {
                "len" => Ty::Int,
                "contains" | "starts_with" | "ends_with" => Ty::Bool,
                "pop" => match self.ty_of(object) {
                    Ty::ArrStr => Ty::Str,
                    _ => Ty::Int,
                },
                "push" => Ty::Int,
                _ => Ty::Int,
            },
            Expr::Index(array, _, _) => match self.ty_of(array) {
                Ty::ArrStr | Ty::Str | Ty::Map => Ty::Str,
                Ty::ArrObj(shape) => Ty::Obj(shape),
                Ty::ArrArr => Ty::Arr,
                Ty::ArrStrArr => Ty::ArrStr,
                Ty::ArrF => Ty::Float,
                _ => Ty::Int,
            },
            Expr::Member(object, key, _) => match self.ty_of(object) {
                Ty::Obj(shape) => self.checker.field_ty(shape, key).unwrap_or(Ty::Int),
                _ => Ty::Int,
            },
            Expr::Assign(target, _, _) => self.ty_of(target),
            Expr::Compound(target, _, _, _) => self.ty_of(target),
            Expr::Func(_, _, _) => self
                .literal_ids
                .get(&(expr as *const Expr as usize))
                .map(|id| Ty::Func(*id))
                .unwrap_or(Ty::Int),
            Expr::Call(callee, args, _) => {
                if let Expr::Ident(name) = &**callee {
                    if matches!(name.as_str(), "exec" | "sh") {
                        let _ = args;
                        let (keys, kinds) = exec_result_keys();
                        return Ty::Obj(self.shape_id(&keys, &kinds));
                    }
                }
                self.call_result_ty(callee, args)
            }
        }
    }

    fn call_result_ty(&self, callee: &Expr, args: &[Expr]) -> Ty {
        if let Expr::Ident(name) = callee {
            if let Some(slot) = self.index.get(name).copied() {
                if let Some(sig) = self.checker.fn_sig(name) {
                    let _ = slot;
                    return sig.result.unwrap_or(Ty::Int);
                }
            }
            if let Some(Ty::Func(_)) = self.lookup_ty(name).filter(|ty| matches!(ty, Ty::Func(_))) {
                // literal-valued variable
                if let Some(Ty::Func(id)) = self.lookup_ty(name) {
                    if let Some(sig) = self.checker.lit_sigs.get(id as usize) {
                        return sig.result.unwrap_or(Ty::Int);
                    }
                }
            }
            return match name.as_str() {
                "attempt" => {
                    if let Some(Ty::Func(id)) = args.first().map(|arg| self.ty_of(arg)) {
                        if let Some(result_ty) = self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .and_then(|sig| sig.result)
                            .filter(|ty| field_kind(*ty).is_some())
                        {
                            let kind = field_kind(result_ty).unwrap();
                            return Ty::Obj(self.shape_id(
                                &["error".to_string(), "ok".to_string(), "value".to_string()],
                                &[1, 3, kind],
                            ));
                        }
                    }
                    Ty::Int
                }
                "str" => Ty::Str,
                "int" => Ty::Int,
                "float" => Ty::Float,
                "len" | "argc" | "write_file" | "run" | "now" | "sleep" | "mkdir" | "get"
                | "sum" => Ty::Int,
                "reduce" => args
                    .get(2)
                    .map(|init| self.ty_of(init))
                    .unwrap_or_else(|| match args.first().map(|arg| self.ty_of(arg)) {
                        Some(Ty::ArrF) => Ty::Float,
                        Some(Ty::ArrStr) => Ty::Str,
                        _ => Ty::Int,
                    }),
                "abs" | "floor" | "ceil" | "round" => Ty::Int,
                "sqrt" => Ty::Float,
                "type" | "arg" | "read_file" | "capture" | "cwd" | "env" | "join" => Ty::Str,
                "exists" | "file_exists" | "has" => Ty::Bool,
                "split" | "keys" => Ty::ArrStr,
                "range" | "values" => Ty::Arr,
                "reverse" | "sort" | "slice" => {
                    args.first().map(|arg| self.ty_of(arg)).unwrap_or(Ty::Arr)
                }
                "map_new" => Ty::Map,
                "map_get" | "map_has" | "map_len" | "map_set" => Ty::Int,
                "merge" => Ty::Int,
                "map" => {
                    if let Some(Ty::Func(id)) = args.get(1).map(|arg| self.ty_of(arg)) {
                        match self
                            .checker
                            .lit_sigs
                            .get(id as usize)
                            .and_then(|sig| sig.result)
                        {
                            Some(Ty::Str) => Ty::ArrStr,
                            Some(Ty::ArrStr) => Ty::ArrStrArr,
                            Some(Ty::Float) => Ty::ArrF,
                            Some(Ty::Obj(shape)) => Ty::ArrObj(shape),
                            Some(Ty::Arr) => Ty::ArrArr,
                            _ => Ty::Arr,
                        }
                    } else {
                        Ty::Arr
                    }
                }
                "filter" => args.first().map(|arg| self.ty_of(arg)).unwrap_or(Ty::Arr),
                _ => Ty::Int,
            };
        }
        match self.ty_of(callee) {
            Ty::Func(id) => self
                .checker
                .lit_sigs
                .get(id as usize)
                .and_then(|sig| sig.result)
                .unwrap_or(Ty::Int),
            _ => Ty::Int,
        }
    }

    fn declare(&mut self, name: &str, ty: Ty, loop_var: bool) {
        let frame = self.frame();
        if let Some(scope) = frame.scopes.last_mut() {
            scope.insert(name.to_string(), FrameBinding { ty, loop_var });
        }
    }

    fn push_scope(&mut self) {
        self.frame().scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.frame().scopes.pop();
    }

    /// Ensure `frame` (a literal) captures `name`; returns how that frame sees
    /// the cell pointer (`Env(offset)` in its own environment).
    fn ensure_capture(&mut self, frame: usize, name: &str) -> Result<CaptureSource, String> {
        if let Some(position) = self.frames[frame]
            .captures
            .iter()
            .position(|cap| cap.name == name)
        {
            return Ok(CaptureSource::Env(position as u32));
        }
        let parent = frame - 1;
        if let Some(binding) = self.binding_in(parent, name) {
            let _ = binding.loop_var;
            self.frames[parent].boxed.insert(name.to_string());
            let ty = binding.ty;
            self.frames[frame].captures.push(Capture {
                name: name.to_string(),
                ty,
                source: CaptureSource::Local,
            });
            return Ok(CaptureSource::Env(
                (self.frames[frame].captures.len() - 1) as u32,
            ));
        }
        if self.frames[parent].id == CaptureFrame::Start {
            // Globals are read directly from inside literals; nothing to capture.
            return Err("wasm target internal: unexpected global capture".to_string());
        }
        let source = self.ensure_capture(parent, name)?;
        let ty = self
            .lookup_ty(name)
            .ok_or_else(|| format!("wasm target cannot type captured '{}'", name))?;
        self.frames[frame].captures.push(Capture {
            name: name.to_string(),
            ty,
            source,
        });
        Ok(CaptureSource::Env(
            (self.frames[frame].captures.len() - 1) as u32,
        ))
    }

    fn use_name(&mut self, name: &str) -> Result<(), String> {
        let current = self.frames.len() - 1;
        let mut found = None;
        for frame in (0..=current).rev() {
            if self.binding_in(frame, name).is_some() {
                found = Some(frame);
                break;
            }
        }
        let Some(owner) = found else {
            return Ok(());
        };
        if owner == current {
            return Ok(());
        }
        if self.frames[owner].id == CaptureFrame::Start && self.globals.contains_key(name) {
            return Ok(());
        }
        for frame in (owner + 1)..=current {
            if matches!(self.frames[frame].id, CaptureFrame::Lit(_)) {
                self.ensure_capture(frame, name)?;
            }
        }
        self.frames[owner].boxed.insert(name.to_string());
        Ok(())
    }

    fn walk_expr(&mut self, expr: &Expr) -> Result<(), String> {
        match expr {
            Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) => Ok(()),
            Expr::Ident(name) => self.use_name(name),
            Expr::Array(items) => {
                for item in items {
                    self.walk_expr(item)?;
                }
                Ok(())
            }
            Expr::Object(fields) => {
                for (_, value) in fields {
                    self.walk_expr(value)?;
                }
                Ok(())
            }
            Expr::Neg(inner) | Expr::Not(inner) => self.walk_expr(inner),
            Expr::Bin(_, left, right) => {
                self.walk_expr(left)?;
                self.walk_expr(right)
            }
            Expr::Ternary(cond, then, other, _) => {
                self.walk_expr(cond)?;
                self.walk_expr(then)?;
                self.walk_expr(other)
            }
            Expr::And(left, right) | Expr::Or(left, right) => {
                self.walk_expr(left)?;
                self.walk_expr(right)
            }
            Expr::Func(params, body, _) => {
                let Some(id) = self
                    .literal_ids
                    .get(&(expr as *const Expr as usize))
                    .copied()
                else {
                    return Err("wasm target lost a function literal id".to_string());
                };
                self.lit_params[id as usize] = params.clone();
                let param_tys: Vec<Ty> = self
                    .checker
                    .lit_sigs
                    .get(id as usize)
                    .map(|sig| sig.params.clone())
                    .unwrap_or_else(|| vec![Ty::Int; params.len()]);
                self.frames.push(FrameState {
                    id: CaptureFrame::Lit(id),
                    scopes: vec![HashMap::new()],
                    captures: Vec::new(),
                    boxed: std::collections::HashSet::new(),
                });
                for (param, ty) in params.iter().zip(param_tys.iter()) {
                    self.declare(param, *ty, false);
                }
                self.walk_stmts(body)?;
                let frame = self.frames.pop().expect("literal frame");
                let id = match frame.id {
                    CaptureFrame::Lit(id) => id,
                    _ => unreachable!(),
                };
                if !frame.boxed.is_empty() {
                    self.analysis.lit_boxed.insert(id, frame.boxed);
                }
                self.analysis.lit_captures[id as usize] = frame.captures;
                Ok(())
            }
            Expr::Call(callee, args, _) => {
                if let Expr::Ident(name) = &**callee {
                    let is_value = self
                        .lookup_ty(name)
                        .map(|ty| matches!(ty, Ty::Func(_)))
                        .unwrap_or(false);
                    if is_value {
                        self.use_name(name)?;
                    }
                } else {
                    self.walk_expr(callee)?;
                }
                for arg in args {
                    self.walk_expr(arg)?;
                }
                Ok(())
            }
            Expr::MethodCall(object, _, args, _) => {
                self.walk_expr(object)?;
                for arg in args {
                    self.walk_expr(arg)?;
                }
                Ok(())
            }
            Expr::Index(array, index, _) => {
                self.walk_expr(array)?;
                self.walk_expr(index)
            }
            Expr::Member(object, _, _) => self.walk_expr(object),
            Expr::Assign(target, value, _) | Expr::Compound(target, _, value, _) => {
                self.walk_expr(value)?;
                match &**target {
                    Expr::Ident(name) => self.use_name(name),
                    other => self.walk_expr(other),
                }
            }
        }
    }

    fn walk_stmts(&mut self, stmts: &[Stmt]) -> Result<(), String> {
        for stmt in stmts {
            self.walk_stmt(stmt)?;
        }
        Ok(())
    }

    fn walk_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let(name, init, _) => {
                if let Some(init) = init {
                    self.walk_expr(init)?;
                    let ty = self.ty_of(init);
                    self.declare(name, ty, false);
                } else {
                    self.declare(name, Ty::Int, false);
                }
                Ok(())
            }
            Stmt::Expr(expr, _) => self.walk_expr(expr),
            Stmt::Fn(..) => Ok(()),
            Stmt::Block(body) => {
                self.push_scope();
                let result = self.walk_stmts(body);
                self.pop_scope();
                result
            }
            Stmt::If(cond, then_body, else_body, _) => {
                self.walk_expr(cond)?;
                self.push_scope();
                self.walk_stmts(then_body)?;
                self.pop_scope();
                if let Some(else_body) = else_body {
                    self.push_scope();
                    self.walk_stmts(else_body)?;
                    self.pop_scope();
                }
                Ok(())
            }
            Stmt::While(cond, body, _) => {
                self.walk_expr(cond)?;
                self.push_scope();
                let result = self.walk_stmts(body);
                self.pop_scope();
                result
            }
            Stmt::ForC(init, cond, step, body, _) => {
                self.push_scope();
                if let Some(init) = init {
                    self.walk_stmt(init)?;
                }
                if let Some(cond) = cond {
                    self.walk_expr(cond)?;
                }
                if let Some(step) = step {
                    self.walk_expr(step)?;
                }
                let result = self.walk_stmts(body);
                self.pop_scope();
                result
            }
            Stmt::ForIn(key, value, iter, body, _) => {
                self.walk_expr(iter)?;
                let iter_ty = self.ty_of(iter);
                let (key_ty, value_ty) = match iter_ty {
                    Ty::ArrStr => (Ty::Int, Ty::Str),
                    Ty::ArrObj(shape) => (Ty::Int, Ty::Obj(shape)),
                    Ty::ArrArr => (Ty::Int, Ty::Arr),
                    Ty::ArrF => (Ty::Int, Ty::Float),
                    Ty::Obj(_) | Ty::Map => (Ty::Str, Ty::Int),
                    _ => (Ty::Int, Ty::Int),
                };
                self.push_scope();
                match value {
                    Some(value) => {
                        self.declare(key, key_ty, true);
                        self.declare(value, value_ty, true);
                    }
                    None => {
                        // A single loop variable binds the key for records/maps and
                        // the element for arrays.
                        let single_ty = match iter_ty {
                            Ty::Obj(_) | Ty::Map => key_ty,
                            _ => value_ty,
                        };
                        self.declare(key, single_ty, true);
                    }
                }
                let result = self.walk_stmts(body);
                self.pop_scope();
                result
            }
            Stmt::Return(Some(expr), _) => self.walk_expr(expr),
            Stmt::Return(None, _) | Stmt::Break(_) | Stmt::Continue(_) | Stmt::Use(..) => Ok(()),
        }
    }

    fn run(
        &mut self,
        stmts: &[Stmt],
        declarations: &[(String, Vec<String>, &[Stmt])],
    ) -> Result<(), String> {
        self.globals = self.checker.global_types();
        // Top-level frame: bindings are module globals, read directly by literals.
        self.frames.push(FrameState {
            id: CaptureFrame::Start,
            scopes: vec![HashMap::new()],
            captures: Vec::new(),
            boxed: std::collections::HashSet::new(),
        });
        for stmt in stmts {
            if let Stmt::Fn(..) = stmt {
                continue;
            }
            self.walk_stmt(stmt)?;
        }
        let start_frame = self.frames.pop().expect("start frame");
        self.analysis.start_boxed = start_frame.boxed;
        for (slot, (name, params, body)) in declarations.iter().enumerate() {
            let param_tys: Vec<Ty> = self
                .checker
                .fn_sig(name)
                .map(|sig| sig.params.clone())
                .unwrap_or_else(|| vec![Ty::Int; params.len()]);
            self.frames.push(FrameState {
                id: CaptureFrame::Fn(slot),
                scopes: vec![HashMap::new()],
                captures: Vec::new(),
                boxed: std::collections::HashSet::new(),
            });
            for (param, ty) in params.iter().zip(param_tys.iter()) {
                self.declare(param, *ty, false);
            }
            self.walk_stmts(body)?;
            let frame = self.frames.pop().expect("function frame");
            if !frame.boxed.is_empty() {
                self.analysis.fn_boxed.insert(slot, frame.boxed);
            }
        }
        Ok(())
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
                body: body.as_slice(),
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
    let (mut inferred_params, mut inferred_results) = {
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
            // One locals map across the statements: a `let` typed by an earlier
            // statement must be visible to later ones during the same pass.
            let mut top_locals = globals.clone();
            for stmt in &stmts {
                if let Stmt::Fn(..) = stmt {
                    continue;
                }
                changed |= infer.walk(std::slice::from_ref(stmt), &mut top_locals, None)?;
            }
            for (slot, decl) in declarations.iter().enumerate() {
                let mut locals = globals.clone();
                for (param, ty) in decl.params.iter().zip(infer.params[slot].iter()) {
                    locals.insert(param.clone(), ty.unwrap_or(Ty::Int));
                }
                changed |= infer.walk(decl.body, &mut locals, Some(slot))?;
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

    // Second seeding round: globals whose initialiser needs a literal signature
    // (for example `let f = make(1)`) could not be typed before inference; now
    // that provisional literal signatures exist, declare them so later calls
    // through those globals can be inferred.
    for _ in 0..4 {
        let mut declared_any = false;
        for stmt in &stmts {
            if let Stmt::Let(name, _, _) = stmt {
                if checker.lookup(name).is_err() && checker.check_stmt(stmt).is_ok() {
                    declared_any = true;
                }
            }
        }
        if !declared_any {
            break;
        }
    }
    // Capture analysis must run before literal bodies are checked so captured
    // outer bindings can be put in scope for them (and for inference pass B).
    let mut analysis = {
        let mut walker = CaptureWalker::new(&checker, &index, &literal_ids);
        let decls: Vec<(String, Vec<String>, &[Stmt])> = declarations
            .iter()
            .map(|decl| (decl.name.clone(), decl.params.clone(), decl.body))
            .collect();
        walker.run(&stmts, &decls)?;
        walker.analysis
    };
    if analysis.lit_captures.len() < lit_count {
        analysis.lit_captures.resize(lit_count, Vec::new());
    }
    let capture_types: Vec<Vec<(String, Ty)>> = analysis
        .lit_captures
        .iter()
        .map(|list| {
            list.iter()
                .map(|capture| (capture.name.clone(), capture.ty))
                .collect()
        })
        .collect();
    // Inference pass B: re-run with captured outer bindings in scope so callback
    // results that depend on a capture get a real type.
    {
        let globals = checker.global_types();
        let mut infer = InferState {
            index: &index,
            globals: &globals,
            shapes: &checker.shapes,
            literal_ids: &literal_ids,
            decl_count,
            params: inferred_params.clone(),
            results: inferred_results.clone(),
        };
        for _ in 0..8 {
            let mut changed = false;
            // One locals map across the statements: a `let` typed by an earlier
            // statement must be visible to later ones during the same pass.
            let mut top_locals = globals.clone();
            for stmt in &stmts {
                if let Stmt::Fn(..) = stmt {
                    continue;
                }
                changed |= infer.walk(std::slice::from_ref(stmt), &mut top_locals, None)?;
            }
            for (slot, decl) in declarations.iter().enumerate() {
                let mut locals = globals.clone();
                for (param, ty) in decl.params.iter().zip(infer.params[slot].iter()) {
                    locals.insert(param.clone(), ty.unwrap_or(Ty::Int));
                }
                changed |= infer.walk(decl.body, &mut locals, Some(slot))?;
            }
            for (id, decl) in lit_decls.iter().enumerate() {
                let slot = decl_count + id;
                let mut locals = globals.clone();
                for (param, ty) in decl.params.iter().zip(infer.params[slot].iter()) {
                    locals.insert(param.clone(), ty.unwrap_or(Ty::Int));
                }
                if let Some(list) = capture_types.get(id) {
                    for (name, ty) in list {
                        locals.insert(name.clone(), *ty);
                    }
                }
                changed |= infer.walk(decl.body, &mut locals, Some(slot))?;
            }
            if !changed {
                break;
            }
        }
        inferred_params = infer.params;
        inferred_results = infer.results;
    }
    for (slot, decl) in declarations.iter().enumerate() {
        let params: Vec<Ty> = inferred_params[slot]
            .iter()
            .map(|ty| ty.unwrap_or(Ty::Int))
            .collect();
        checker.set_fn_sig(&decl.name, params, inferred_results[slot]);
    }
    for (id, sig) in checker.lit_sigs.iter_mut().enumerate() {
        let slot = decl_count + id;
        sig.params = inferred_params[slot]
            .iter()
            .map(|ty| ty.unwrap_or(Ty::Int))
            .collect();
        sig.result = inferred_results[slot];
    }
    checker.check_literal_bodies(&lit_decls, &capture_types)?;
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
    let panic_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![]);
    let str_char_at_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    let str_unary_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let str_replace_ty = builder.add_type(
        vec![
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
        ],
        vec![ValType::I32],
    );
    let str_index_of_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I64],
    );
    let str_repeat_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I64],
        vec![ValType::I32],
    );
    let str_split_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    let str_join_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    let str_slice_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I64, ValType::I64],
        vec![ValType::I32],
    );
    let arr_slice_ty = builder.add_type(
        vec![ValType::I32, ValType::I64, ValType::I64],
        vec![ValType::I32],
    );
    let arr_range_ty = builder.add_type(
        vec![ValType::I64, ValType::I64, ValType::I64],
        vec![ValType::I32],
    );
    let arr_sum_ty = builder.add_type(vec![ValType::I32], vec![ValType::I64]);
    let arr_unary_ty = builder.add_type(vec![ValType::I32], vec![ValType::I32]);
    let print_arr_i64_ty = builder.add_type(vec![ValType::I32], vec![]);
    let print_arr_str_ty = builder.add_type(vec![ValType::I32], vec![]);
    let print_record_ty = builder.add_type(vec![ValType::I32, ValType::I32, ValType::I32], vec![]);
    let print_arr_obj_ty = builder.add_type(vec![ValType::I32, ValType::I32, ValType::I32], vec![]);
    let print_arr_arr_ty = builder.add_type(vec![ValType::I32], vec![]);
    let print_map_ty = builder.add_type(vec![ValType::I32], vec![]);
    let arr_index_of_ty = builder.add_type(vec![ValType::I32, ValType::I64], vec![ValType::I64]);
    let arr_index_of_str_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I64],
    );
    let arr_minmax_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I64]);
    let arr_minmax_str_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let print_arr_f64_ty = builder.add_type(vec![ValType::I32], vec![]);
    let print_arr_strarr_ty = builder.add_type(vec![ValType::I32], vec![]);
    let str_len_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I64]);
    let str_to_float_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::F64]);
    let which_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::I32]);
    let print_record_typed_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        vec![],
    );
    let print_arr_obj_typed_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        vec![],
    );
    let exec_record_ty = builder.add_type(
        vec![
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
        ],
        vec![ValType::I32],
    );
    let exit_ty = builder.add_type(vec![ValType::I32], vec![]);
    let error_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![]);
    let arr_sum_f_ty = builder.add_type(vec![ValType::I32], vec![ValType::F64]);
    let arr_minmax_f_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![ValType::F64]);
    let arr_index_of_f_ty = builder.add_type(vec![ValType::I32, ValType::F64], vec![ValType::I64]);
    let arr_sort_f_ty = builder.add_type(vec![ValType::I32], vec![ValType::I32]);
    let arr_join_ty = builder.add_type(
        vec![ValType::I32, ValType::I32, ValType::I32],
        vec![ValType::I32],
    );
    let str_test_ty = builder.add_type(
        vec![
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
        ],
        vec![ValType::I32],
    );
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
        let mut params: Vec<ValType> = vec![ValType::I32];
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
    builder.import_func("env", "str_test", str_test_ty);
    builder.import_func("env", "panic", panic_ty);
    builder.import_func("env", "str_char_at", str_char_at_ty);
    builder.import_func("env", "str_trim", str_unary_ty);
    builder.import_func("env", "str_upper", str_unary_ty);
    builder.import_func("env", "str_lower", str_unary_ty);
    builder.import_func("env", "str_replace", str_replace_ty);
    builder.import_func("env", "str_index_of", str_index_of_ty);
    builder.import_func("env", "str_repeat", str_repeat_ty);
    builder.import_func("env", "str_split", str_split_ty);
    builder.import_func("env", "str_join", str_join_ty);
    builder.import_func("env", "str_slice", str_slice_ty);
    builder.import_func("env", "arr_slice", arr_slice_ty);
    builder.import_func("env", "arr_range", arr_range_ty);
    builder.import_func("env", "arr_sum", arr_sum_ty);
    builder.import_func("env", "arr_reverse", arr_unary_ty);
    builder.import_func("env", "arr_sort", arr_unary_ty);
    builder.import_func("env", "print_arr_i64", print_arr_i64_ty);
    builder.import_func("env", "print_arr_str", print_arr_str_ty);
    builder.import_func("env", "print_record", print_record_ty);
    builder.import_func("env", "print_arr_obj", print_arr_obj_ty);
    builder.import_func("env", "print_arr_arr", print_arr_arr_ty);
    builder.import_func("env", "print_map", print_map_ty);
    builder.import_func("env", "arr_index_of", arr_index_of_ty);
    builder.import_func("env", "arr_index_of_str", arr_index_of_str_ty);
    builder.import_func("env", "arr_minmax", arr_minmax_ty);
    builder.import_func("env", "arr_minmax_str", arr_minmax_str_ty);
    builder.import_func("env", "print_arr_f64", print_arr_f64_ty);
    builder.import_func("env", "arr_sum_f", arr_sum_f_ty);
    builder.import_func("env", "arr_minmax_f", arr_minmax_f_ty);
    builder.import_func("env", "arr_index_of_f", arr_index_of_f_ty);
    builder.import_func("env", "arr_sort_f", arr_sort_f_ty);
    builder.import_func("env", "arr_sort_str", arr_sort_f_ty);
    builder.import_func("env", "arr_join_int", arr_join_ty);
    builder.import_func("env", "arr_join_f", arr_join_ty);
    builder.import_func("env", "print_arr_strarr", print_arr_strarr_ty);
    builder.import_func("env", "str_len", str_len_ty);
    builder.import_func("env", "exit", exit_ty);
    builder.import_func("env", "error", error_ty);
    builder.import_func("env", "str_to_float", str_to_float_ty);
    builder.import_func("bx", "which", which_ty);
    builder.import_func("env", "print_record_typed", print_record_typed_ty);
    builder.import_func("env", "print_arr_obj_typed", print_arr_obj_typed_ty);
    builder.import_func("bx", "exec_record", exec_record_ty);
    builder.import_func("bx", "exec_record_soft", exec_record_ty);
    builder.import_func("bx", "read_file_soft", read_file_ty);
    builder.import_func("bx", "write_file_soft", write_file_ty);
    builder.import_func("env", "mkdir_soft", mkdir_ty);
    builder.enable_memory(8);
    for name in &checker.order {
        let ty = checker.lookup(name)?;
        match ty {
            Ty::Int => builder.add_global(ValType::I64, true, WVal::I64(0)),
            Ty::Float => builder.add_global(ValType::F64, true, WVal::F64(0.0)),
            Ty::Arr | Ty::ArrObj(_) | Ty::ArrArr | Ty::ArrF | Ty::ArrStrArr => {
                builder.add_global(ValType::I32, true, WVal::I32(0))
            }
            Ty::Str => {
                builder.add_global(ValType::I32, true, WVal::I32(0));
                builder.add_global(ValType::I32, true, WVal::I32(0));
            }
            Ty::Obj(_) | Ty::Map | Ty::Func(_) | Ty::Bool | Ty::ArrStr => {
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
            record_tables: HashMap::new(),
            record_kind_tables: HashMap::new(),
            attempts: Vec::new(),
            continue_targets: Vec::new(),
            captures: Vec::new(),
            env_slot: None,
            boxed: analysis.start_boxed.clone(),
            analysis: &analysis,
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
                record_tables: HashMap::new(),
                record_kind_tables: HashMap::new(),
                attempts: Vec::new(),
                continue_targets: Vec::new(),
                captures: Vec::new(),
                env_slot: None,
                boxed: analysis.fn_boxed.get(&slot).cloned().unwrap_or_default(),
                analysis: &analysis,
            };
            let mut used = 0u32;
            for (param, ty) in decl.params.iter().zip(sig.params.iter()) {
                emitter.locals.insert(
                    param.clone(),
                    LocalSlot {
                        ty: *ty,
                        slot: used,
                        boxed: false,
                        capture: None,
                    },
                );
                used += wasm_types(*ty).len() as u32;
            }
            let boxed_params = emitter.boxed.clone();
            for name in boxed_params {
                if let Some(slot_info) = emitter.locals.get(&name).copied() {
                    emitter.emit_load(&name)?;
                    emitter.box_local(&name, slot_info.ty)?;
                }
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
                Some(
                    Ty::Arr
                    | Ty::ArrStr
                    | Ty::ArrObj(_)
                    | Ty::ArrArr
                    | Ty::ArrF
                    | Ty::ArrStrArr
                    | Ty::Obj(_)
                    | Ty::Map
                    | Ty::Func(_)
                    | Ty::Bool,
                ) => emitter.builder.push(Instr::Const(0x41, WVal::I32(0))),
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
        // Literal frames reserve wasm param 0 for the capture environment, so
        // the scratch locals start after env + declared params.
        let mut temp_base = 1u32;
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
                record_tables: HashMap::new(),
                record_kind_tables: HashMap::new(),
                attempts: Vec::new(),
                continue_targets: Vec::new(),
                captures: analysis.lit_captures.get(id).cloned().unwrap_or_default(),
                env_slot: Some(0),
                boxed: analysis
                    .lit_boxed
                    .get(&(id as u32))
                    .cloned()
                    .unwrap_or_default(),
                analysis: &analysis,
            };
            let mut used = 1u32;
            for (param, ty) in decl.params.iter().zip(sig.params.iter()) {
                emitter.locals.insert(
                    param.clone(),
                    LocalSlot {
                        ty: *ty,
                        slot: used,
                        boxed: false,
                        capture: None,
                    },
                );
                used += wasm_types(*ty).len() as u32;
            }
            let boxed_params = emitter.boxed.clone();
            for name in boxed_params {
                if let Some(slot) = emitter.locals.get(&name).copied() {
                    emitter.emit_load(&name)?;
                    emitter.box_local(&name, slot.ty)?;
                }
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
                Some(
                    Ty::Arr
                    | Ty::ArrStr
                    | Ty::ArrObj(_)
                    | Ty::ArrArr
                    | Ty::ArrF
                    | Ty::ArrStrArr
                    | Ty::Obj(_)
                    | Ty::Map
                    | Ty::Func(_)
                    | Ty::Bool,
                ) => emitter.builder.push(Instr::Const(0x41, WVal::I32(0))),
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
    // Validate with the absorbed core first: it catches the local/stack type
    // errors the in-tree decoder never checked.
    crate::wasm_core::validate(bytes)?;
    scan_module(bytes)
}

/// Minimal section reader for the tooling summary (no instruction decoding).
struct SectionReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SectionReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        SectionReader { data, pos: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(count).ok_or("wasm section overflow")?;
        if end > self.data.len() {
            return Err("wasm section is truncated".to_string());
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn uleb(&mut self) -> Result<u32, String> {
        let mut value: u32 = 0;
        let mut shift = 0;
        loop {
            let byte = self.byte()?;
            value |= ((byte & 0x7f) as u32) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 32 {
                return Err("wasm LEB128 is too long".to_string());
            }
        }
    }

    fn name(&mut self) -> Result<String, String> {
        let len = self.uleb()? as usize;
        let bytes = self.take(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn limits(&mut self) -> Result<(), String> {
        let flags = self.uleb()?;
        self.uleb()?;
        if flags & 0x01 != 0 {
            self.uleb()?;
        }
        Ok(())
    }
}

fn scan_module(bytes: &[u8]) -> Result<WasmInfo, String> {
    if bytes.len() < 8 || &bytes[0..4] != b"\0asm" {
        return Err("not a wasm module".to_string());
    }
    let mut reader = SectionReader::new(&bytes[8..]);
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut defined_functions = 0;
    while reader.pos < reader.data.len() {
        let id = reader.byte()?;
        let size = reader.uleb()? as usize;
        let body = reader.take(size)?;
        let mut section = SectionReader::new(body);
        match id {
            2 => {
                let count = section.uleb()?;
                for _ in 0..count {
                    let module = section.name()?;
                    let field = section.name()?;
                    let kind = section.byte()?;
                    match kind {
                        0 => {
                            section.uleb()?;
                        }
                        1 => {
                            section.byte()?;
                            section.limits()?;
                        }
                        2 => section.limits()?,
                        3 => {
                            section.byte()?;
                            section.byte()?;
                        }
                        other => return Err(format!("unknown wasm import kind {}", other)),
                    }
                    imports.push(format!("{}.{}", module, field));
                }
            }
            3 => defined_functions = section.uleb()? as usize,
            7 => {
                let count = section.uleb()?;
                for _ in 0..count {
                    let name = section.name()?;
                    let kind = section.byte()?;
                    section.uleb()?;
                    // Match the earlier decoder: only function exports are listed.
                    if kind == 0 {
                        exports.push(name);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(WasmInfo {
        imports,
        exports,
        defined_functions,
    })
}

/// Parse and validate a module without running it (used by the module tests).
/// Validate a module with the absorbed core (used by `bx run` caching).
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    crate::wasm_core::validate(bytes)
}

/// Execute through the absorbed `tinyvm` core (PRD_02_44 P1).
///
/// Sinks are owned because the core stores its callback table for the module's
/// lifetime. The CLI uses this entry; the in-tree interpreter stays reachable
/// through [`execute`] until P1 deletes it.
pub fn execute<F, G>(
    bytes: &[u8],
    argv: &[String],
    max_steps: u64,
    timeout_ms: u64,
    on_i64: F,
    on_str: G,
) -> Result<i64, String>
where
    F: FnMut(i64) + 'static,
    G: FnMut(&str) + 'static,
{
    crate::wasm_core::run_start(bytes, argv, max_steps, timeout_ms, on_i64, on_str)
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
    fn argv_capture_and_budgets_work() {
        let source =
            "let i = 0\nwhile (i < argc()) {\n  print(arg(i))\n  i = i + 1\n}\nprint(arg(99))";
        let bytes = compile_to_wasm(source).unwrap();
        let text = Arc::new(Mutex::new(Vec::new()));
        let numbers = Arc::new(Mutex::new(Vec::new()));
        let text_sink = text.clone();
        let number_sink = numbers.clone();
        execute(
            &bytes,
            &["hello world".to_string(), "héllo".to_string()],
            0,
            0,
            move |value| number_sink.lock().expect("lock").push(value),
            move |line| text_sink.lock().expect("lock").push(line.to_string()),
        )
        .unwrap();
        let text = text.lock().expect("lock").clone();
        let numbers = numbers.lock().expect("lock").clone();
        assert_eq!(numbers, Vec::<i64>::new());
        assert_eq!(
            text,
            vec![
                "hello world".to_string(),
                "héllo".to_string(),
                String::new(),
            ]
        );

        let capture = compile_to_wasm("print(capture(\"printf abc\"))").unwrap();
        let text = Arc::new(Mutex::new(Vec::new()));
        let text_sink = text.clone();
        execute(
            &capture,
            &[],
            0,
            0,
            |_| {},
            move |line| text_sink.lock().expect("lock").push(line.to_string()),
        )
        .unwrap();
        assert_eq!(*text.lock().expect("lock"), vec!["abc".to_string()]);

        let looping = compile_to_wasm("let n = 0\nwhile (true) { n = n + 1 }").unwrap();
        let error = execute(&looping, &[], 500, 0, |_| {}, |_| {}).unwrap_err();
        assert!(error.contains("step budget"), "got {error}");
    }

    #[test]
    fn file_and_map_failures_are_explicit() {
        let missing =
            compile_to_wasm("print(read_file(\"C:/definitely-missing-bx.txt\"))").unwrap();
        let error = execute(&missing, &[], 0, 0, |_| {}, |_| {}).unwrap_err();
        assert!(error.contains("cannot read"), "got {error}");

        let full = compile_to_wasm(
            "let m = map_new()\nlet i = 0\nwhile (i < 65) {\n  map_set(m, \"k\" + str(i), i)\n  i = i + 1\n}",
        )
        .unwrap();
        let error = execute(&full, &[], 0, 0, |_| {}, |_| {}).unwrap_err();
        assert!(error.contains("map is full"), "got {error}");

        let blocker = std::env::temp_dir().join(format!("bx-wasm-blocker-{}", std::process::id()));
        std::fs::write(&blocker, b"x").unwrap();
        let bad_path = blocker
            .join("child.txt")
            .to_string_lossy()
            .replace('\\', "/");
        let source = format!("print(write_file(\"{}\", \"d\"))", bad_path);
        let (numbers, _) = run_with_argv(&source, &[]);
        let _ = std::fs::remove_file(&blocker);
        assert_eq!(numbers, vec![-1]);
    }

    #[test]
    fn record_merge() {
        let (numbers, strings) = run_with_argv(
            "let o = merge({a: 1, b: 2}, {b: 3, c: 4})\nfor k in keys(o) {\n  print(k)\n}\nprint(o.a)\nprint(o.b)\nprint(o.c)\nprint(join(keys(merge({x: 1}, {y: 2})), \"+\"))",
            &[],
        );
        assert_eq!(numbers, vec![1, 3, 4]);
        assert_eq!(
            strings,
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "x+y".to_string(),
            ]
        );
    }

    #[test]
    fn record_builtins() {
        let (numbers, strings) = run_with_argv(
            "let o = {a: 1, b: 2}\nfor k in keys(o) {\n  print(k)\n}\nfor v in values(o) {\n  print(v)\n}\nprint(has(o, \"a\"))\nprint(get(o, \"b\"))\nprint(get(o, \"z\", 9))\nprint(join(keys(o), \"+\"))",
            &[],
        );
        assert_eq!(numbers, vec![1, 2, 2, 9]);
        assert_eq!(
            strings,
            vec![
                "a".to_string(),
                "b".to_string(),
                "true".to_string(),
                "a+b".to_string()
            ]
        );
    }

    #[test]
    fn higher_order_string_arrays() {
        let (numbers, strings) = run_with_argv(
            "for v in filter([\"a\", \"bb\", \"ccc\"], fn(s) { return s.len() > 1 }) {\n  print(v)\n}\nfor v in map([\"a\", \"b\"], fn(s) { return s + \"!\" }) {\n  print(v)\n}\nfor v in map([\"aa\", \"b\"], fn(s) { return s.len() }) {\n  print(v)\n}\nlet words = [\"x\", \"yy\"]\nwords = map(words, fn(s) { return \"[\" + s + \"]\" })\nprint(join(words, \",\"))",
            &[],
        );
        assert_eq!(numbers, vec![2, 1]);
        assert_eq!(
            strings,
            vec![
                "bb".to_string(),
                "ccc".to_string(),
                "a!".to_string(),
                "b!".to_string(),
                "[x],[yy]".to_string(),
            ]
        );
    }

    #[test]
    fn higher_order_array_builtins() {
        let (numbers, strings) = run_with_argv(
            "for v in map([1, 2, 3], fn(x) { return x * 2 }) {\n  print(v)\n}\nfor v in filter([1, 2, 3, 4], fn(x) { return x % 2 == 0 }) {\n  print(v)\n}\nprint(reduce([1, 2, 3], fn(acc, x) { return acc + x }, 0))\neach([5], fn(x) { print(x + 1) })",
            &[],
        );
        assert!(strings.is_empty());
        assert_eq!(numbers, vec![2, 4, 6, 2, 4, 6, 6]);
    }

    #[test]
    fn int_array_builtins() {
        let (numbers, strings) = run_with_argv(
            "let a = range(2, 6)\nprint(sum(a))\nprint(len(a))\nfor v in reverse(a) {\n  print(v)\n}\nlet b = sort([3, 1, 2])\nprint(b[0])\nprint(b[2])\nprint(slice(a, 0, 2)[1])",
            &[],
        );
        assert!(strings.is_empty());
        assert_eq!(numbers, vec![14, 4, 5, 4, 3, 2, 1, 3, 4]);
    }

    #[test]
    fn string_arrays_and_slices() {
        let (numbers, strings) = run_with_argv(
            "let parts = split(\"a,b,c\", \",\")\nprint(len(parts))\nprint(parts[0])\nprint(join(parts, \"-\"))\nprint(slice(\"hello\", 1, 3))\nfor i, p in parts {\n  print(i + \":\" + p)\n}\nparts[1] = \"B\"\nprint(join(parts, \"|\"))\nlet xs = [10, 20, 30, 40]\nfor v in slice(xs, 1, 3) {\n  print(v)\n}",
            &[],
        );
        assert_eq!(numbers, vec![3, 20, 30]);
        assert_eq!(
            strings,
            vec![
                "a".to_string(),
                "a-b-c".to_string(),
                "el".to_string(),
                "0:a".to_string(),
                "1:b".to_string(),
                "2:c".to_string(),
                "a|B|c".to_string(),
            ]
        );
    }

    #[test]
    fn numeric_string_and_type_builtins() {
        let (numbers, strings) = run_with_argv(
            "print(abs(-4))\nprint(floor(2.7))\nprint(round(2.5))\nprint(sqrt(9))\nprint(trim(\"  hi  \"))\nprint(upper(\"aBc\"))\nprint(replace(\"a-b\", \"-\", \"+\"))\nprint(index_of(\"hello\", \"ll\"))\nprint(repeat(\"ab\", 2))\nprint(type(3))\nprint(type(true))",
            &[],
        );
        assert_eq!(numbers, vec![4, 2]);
        assert_eq!(
            strings,
            vec![
                "2".to_string(),
                "3".to_string(),
                "3".to_string(),
                "hi".to_string(),
                "ABC".to_string(),
                "a+b".to_string(),
                "abab".to_string(),
                "int".to_string(),
                "bool".to_string(),
            ]
        );
    }

    #[test]
    fn interpolation_and_assignments_work() {
        let (numbers, strings) = run_with_argv(
            "let r = {a: 1, b: 2}\nr.a = 5\nr.b += 10\nprint(r.a + r.b)\nlet xs = [1, 2, 3]\nxs[0] = 9\nxs[-1] = 7\nxs[1] += 5\nprint(xs[0] + xs[1] + xs[2])\nlet s = \"v\"\nprint(\"x=${s} end\")\nprint(\"n=${1 + 2}\")\nprint(1 + \"x\")\nprint(-2.5)",
            &[],
        );
        assert_eq!(numbers, vec![17, 23]);
        assert_eq!(
            strings,
            vec![
                "x=v end".to_string(),
                "n=3".to_string(),
                "1x".to_string(),
                "-2.5".to_string(),
            ]
        );
    }

    #[test]
    fn indexes_support_negative_float_and_strings() {
        let (numbers, strings) = run_with_argv(
            "let a = [10, 20, 30]\nprint(a[-1])\nprint(a[-2])\nprint(a[1.7])\nprint(\"hello\"[0])\nprint(\"hello\"[-1])\nprint(\"héllo\"[1])",
            &[],
        );
        assert_eq!(numbers, vec![30, 20, 20]);
        assert_eq!(
            strings,
            vec!["h".to_string(), "o".to_string(), "é".to_string()]
        );
    }

    #[test]
    fn bool_comparisons_and_empty_string_tests() {
        let (numbers, strings) = run_with_argv(
            "print(true == false)\nprint(true != false)\nprint(false < true)\nprint(\"\".contains(\"\"))\nprint(\"\".starts_with(\"\"))\nprint(\"abc\".contains(\"\"))",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec![
                "false".to_string(),
                "true".to_string(),
                "true".to_string(),
                "true".to_string(),
                "true".to_string(),
                "true".to_string(),
            ]
        );
    }

    #[test]
    fn out_of_bounds_and_zero_division_abort() {
        let out_of_bounds = compile_to_wasm("let a = [1]\nprint(a[5])").unwrap();
        let error = execute(&out_of_bounds, &[], 0, 0, |_| {}, |_| {}).unwrap_err();
        assert!(error.contains("index out of bounds"), "got {error}");

        let divide = compile_to_wasm("print(7 / 0)").unwrap();
        let error = execute(&divide, &[], 0, 0, |_| {}, |_| {}).unwrap_err();
        assert_eq!(error, "division by zero");

        let modulo = compile_to_wasm("print(7 % 0)").unwrap();
        let error = execute(&modulo, &[], 0, 0, |_| {}, |_| {}).unwrap_err();
        assert_eq!(error, "modulo by zero");

        let negative = compile_to_wasm("let a = [1]\nprint(a[-5])").unwrap();
        let error = execute(&negative, &[], 0, 0, |_| {}, |_| {}).unwrap_err();
        assert!(error.contains("index out of bounds"), "got {error}");
    }

    #[test]
    fn scoped_loop_variables_work_inside_calls() {
        let (numbers, strings) = run_with_argv(
            "let m = {x: 1, y: 2}\nfor k, v in m {\n  print(k + \":\" + str(v))\n}\nfor v in [7, 8] {\n  print(v + 1)\n}",
            &[],
        );
        assert_eq!(numbers, vec![8, 9]);
        assert_eq!(strings, vec!["x:1".to_string(), "y:2".to_string()]);
    }

    #[test]
    fn continue_reaches_loop_steps() {
        let prints = run_source(
            "let sum = 0\nfor v in [1, 2, 3, 4] {\n  if (v == 2) { continue }\n  sum = sum + v\n}\nprint(sum)\nlet acc = 0\nfor (let i = 0; i < 5; i += 1) {\n  if (i % 2 == 0) { continue }\n  acc = acc + i\n}\nprint(acc)\nlet r = {a: 1, b: 2}\nfor k, v in r {\n  if (k == \"b\") { continue }\n  print(v)\n}",
        );
        assert_eq!(prints, vec![8, 4, 1]);
    }

    #[test]
    fn records_iterate_and_string_methods_work() {
        let (numbers, strings) = run_with_argv(
            "let o = {a: 1, b: 2}\nfor k, v in o {\n  print(k)\n  print(v)\n}\nprint(\"abc\".contains(\"b\"))\nprint(\"abc\".starts_with(\"ab\"))\nprint(\"abc\".ends_with(\"bd\"))",
            &[],
        );
        assert_eq!(numbers, vec![1, 2]);
        assert_eq!(
            strings,
            vec![
                "a".to_string(),
                "b".to_string(),
                "true".to_string(),
                "true".to_string(),
                "false".to_string(),
            ]
        );
    }

    #[test]
    fn exists_returns_a_bool() {
        let (numbers, strings) = run_with_argv(
            "print(exists(\".\"))\nprint(exists(\"..\"))\nif (exists(\".\")) { print(1) }",
            &[],
        );
        assert_eq!(numbers, vec![1]);
        assert_eq!(strings, vec!["true".to_string(), "true".to_string()]);
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
    fn nested_arrays_work() {
        let (numbers, strings) = run_with_argv(
            "let m = [[1, 2], [3, 4]]\nprint(m)\nprint(m[1][0])\nprint(len(m))\nfor row in m {\n  print(row)\n}\nm[0] = [9, 8]\nprint(m)\npush(m, [5])\nprint(m)\nprint(m[-1])\nfor i, row in m {\n  print(i)\n}\nlet total = 0\nfor row in m {\n  for v in row {\n    total = total + v\n  }\n}\nprint(total)\nfn sum_row(r) {\n  let s = 0\n  for v in r {\n    s = s + v\n  }\n  return s\n}\nprint(sum_row(m[0]))",
            &[],
        );
        assert_eq!(numbers, vec![3, 2, 0, 1, 2, 29, 17]);
        assert_eq!(
            strings,
            vec![
                "[[1,2],[3,4]]".to_string(),
                "[1,2]".to_string(),
                "[3,4]".to_string(),
                "[[9,8],[3,4]]".to_string(),
                "[[9,8],[3,4],[5]]".to_string(),
                "[5]".to_string(),
            ]
        );
    }

    #[test]
    fn containers_print_like_the_vm() {
        let strings = run_strings(
            "print([1, 2, 3])\nprint([\"a\", \"b\"])\nprint([])\nprint({a: 1, b: 2})\nprint({})\nprint([{a: 1}, {a: 2}])\nlet xs = [10, 20]\nprint(xs)\nprint(sort([3, 1], fn(a, b) { return a - b }))",
        );
        assert_eq!(
            strings,
            vec![
                "[1,2,3]".to_string(),
                "[\"a\",\"b\"]".to_string(),
                "[]".to_string(),
                "{\"a\":1,\"b\":2}".to_string(),
                "{}".to_string(),
                "[{\"a\":1},{\"a\":2}]".to_string(),
                "[10,20]".to_string(),
                "[1,3]".to_string(),
            ]
        );
    }

    #[test]
    fn push_pop_string_and_record_arrays_and_empty_objects() {
        let (numbers, strings) = run_with_argv(
            "let xs = [\"a\"]\npush(xs, \"b\")\nfor v in xs {\n  print(v)\n}\nprint(len(xs))\npop(xs)\nprint(len(xs))\nlet rs = [{a: 1}]\npush(rs, {a: 2})\nfor r in rs {\n  print(r.a)\n}\npop(rs)\nprint(len(rs))\nlet empty = {}\nprint(len(keys(empty)))\nlet m = merge(empty, {x: 5})\nprint(m.x)\nprint(type(empty))",
            &[],
        );
        assert_eq!(numbers, vec![2, 1, 1, 2, 1, 0, 5]);
        assert_eq!(
            strings,
            vec!["a".to_string(), "b".to_string(), "object".to_string()]
        );
    }

    #[test]
    fn float_arrays_work() {
        let (numbers, strings) = run_with_argv(
            "let a = [1.5, 2, 3.25]\nprint(a)\nprint(a[0])\nprint(a[1] + 1)\nprint(len(a))\nprint(sum(a))\nprint(min(a))\nprint(max(a))\nprint(index_of(a, 2))\nprint(a.contains(3.25))\nfor v in a {\n  print(v)\n}\npush(a, 4.5)\nprint(a)\na[0] = 9.5\nprint(a[0])\nprint(type(a))\nlet total = 0.0\nfor v in a {\n  total = total + v\n}\nprint(total)\nfor i, v in a {\n  print(i)\n}",
            &[],
        );
        assert_eq!(numbers, vec![3, 1, 0, 1, 2, 3]);
        assert_eq!(
            strings,
            vec![
                "[1.5,2,3.25]".to_string(),
                "1.5".to_string(),
                "3".to_string(),
                "6.75".to_string(),
                "1.5".to_string(),
                "3.25".to_string(),
                "true".to_string(),
                "1.5".to_string(),
                "2".to_string(),
                "3.25".to_string(),
                "[1.5,2,3.25,4.5]".to_string(),
                "9.5".to_string(),
                "array".to_string(),
                "19.25".to_string(),
            ]
        );
    }

    #[test]
    fn interpolation_with_record_fields_matches_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let o = {a: 1, b: \"x\"}\nprint(\"v=${o.a}\")\nprint(\"v=${o.b}\")\nprint(\"v=${o.a}-${o.b}\")\nprint(\"${o.b}+${o.a}\")\nprint(o.b + \"!\")\nprint(\"[\" + o.b + \"]\" + o.b)\nlet t = o.b + o.b\nprint(t)\nprint(len(t))",
            &[],
        );
        assert_eq!(numbers, vec![2]);
        assert_eq!(
            strings,
            vec![
                "v=1".to_string(),
                "v=x".to_string(),
                "v=1-x".to_string(),
                "x+1".to_string(),
                "x!".to_string(),
                "[x]x".to_string(),
                "xx".to_string(),
            ]
        );
    }

    #[test]
    fn attempt_catches_guarded_errors_and_matches_the_vm() {
        let (numbers, strings) = run_with_argv(
            "print(attempt(fn() { return 1 }).ok)\nprint(attempt(fn() { return 1 }).value)\nprint(attempt(fn() { return 1 / 0 }).ok)\nlet e = attempt(fn() { error(\"boom\") })\nprint(e.ok)\nprint(len(e.error) > 0)\nprint(attempt(fn() { return \"x\" }).value)\nprint(attempt(fn() { return 1 < 2 }).value)\nlet looped = 0\nlet i = 0\nwhile (i < 3) {\n  let r = attempt(fn() { return 10 / (i - 1) })\n  if (r.ok) { looped += 1 }\n  i += 1\n}\nprint(looped)",
            &[],
        );
        assert_eq!(numbers, vec![1, 2]);
        assert_eq!(
            strings,
            vec![
                "true".to_string(),
                "false".to_string(),
                "false".to_string(),
                "true".to_string(),
                "x".to_string(),
                "true".to_string(),
            ]
        );
    }

    #[test]
    fn attempt_catches_exec_spawn_failures_and_matches_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let r = attempt(fn() { return exec(\"definitely-not-a-command-xyz\").code })\nprint(r.ok)\nprint(len(r.error) > 0)\nlet q = attempt(fn() { return sh(\"definitely-not-a-command-xyz\").code })\nprint(q.ok)",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec!["false".to_string(), "true".to_string(), "true".to_string()]
        );
    }

    #[test]
    fn attempt_catches_file_io_failures_and_matches_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let r = attempt(fn() { return read(\"definitely-missing-file-xyz.txt\") })\nprint(r.ok)\nprint(len(r.error) > 0)\nlet w = attempt(fn() { return write(\"Z:/definitely/missing/dir/x.txt\", \"x\") })\nprint(w.ok)\nprint(len(w.error) > 0)",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec![
                "false".to_string(),
                "true".to_string(),
                "false".to_string(),
                "true".to_string()
            ]
        );
    }

    #[test]
    fn mkdir_expression_is_fatal_and_attempt_catches_it() {
        let (numbers, strings) = run_with_argv(
            "write(\"target/mkdir-attempt-probe.txt\", \"x\")\nlet ok = attempt(fn() { return mkdir(\"target\") })\nprint(ok.ok)\nlet bad = attempt(fn() { return mkdir(\"target/mkdir-attempt-probe.txt\") })\nprint(bad.ok)\nprint(len(bad.error) > 0)",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec!["true".to_string(), "false".to_string(), "true".to_string()]
        );
    }

    #[test]
    fn malformed_programs_fail_without_panicking() {
        let templates = [
            "print(1 + \"x\")",
            "print(\"x\" - 1)",
            "print(true * false)",
            "print([1, \"x\"])",
            "print({a: 1, b: \"x\"}.a + 1)",
            "print(1 .a)",
            "print([1][true])",
            "print(len(1))",
            "print(unknown())",
            "print(map([1], fn(x) { return x.a }))",
            "print(sort([1], fn(x, y) { return \"a\" }))",
            "print(sort({a: 1}, fn(x, y) { return 0 }))",
            "print(attempt(1))",
            "print(attempt(fn(x) { return x }))",
            "let f = 1 { f() }",
            "if (1) { print(1) }",
            "while (\"x\") { }",
            "print(exec(1))",
            "print(read(1))",
            "print({a: 1} + 1)",
            "print([1] == 1)",
            "for k in 1 { print(k) }",
            "print(1[0])",
            "print(true && 1)",
            "print(1 ? 2 : 3)",
            "fn f(a, b) { return a } print(f(1))",
            "print(str())",
            "print(int([]))",
            "print({}.a)",
            "print(mkdir(1))",
        ];
        let mut panics: Vec<String> = Vec::new();
        for template in templates {
            let outcome = std::panic::catch_unwind(|| compile_to_wasm(template));
            if outcome.is_err() {
                panics.push(template.to_string());
            }
        }
        assert!(panics.is_empty(), "compiler panicked on: {:?}", panics);
    }

    #[test]
    fn random_malformed_expressions_do_not_panic() {
        let atoms = [
            "0", "1", "-3", "2.5", "true", "false", "\"s\"", "\"\"", "[1, 2]", "[\"a\"]", "[]",
            "{a: 1}", "{}", "null", "x", "len", "print", "[]", "[[1]]",
        ];
        let ops = ["+", "-", "*", "/", "%", "==", "!=", "<", "&&", "||"];
        let calls = [
            "len", "str", "int", "float", "type", "abs", "sqrt", "sum", "sort", "map", "reduce",
            "get", "has", "keys", "values", "attempt", "exec", "read", "mkdir", "unknown",
        ];
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut next = |limit: usize| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as usize) % limit
        };
        let mut panics: Vec<String> = Vec::new();
        for _ in 0..300 {
            let left = atoms[next(atoms.len())];
            let right = atoms[next(atoms.len())];
            let op = ops[next(ops.len())];
            let call = calls[next(calls.len())];
            let depth = next(3);
            let program = match depth {
                0 => format!("print({} {} {})", left, op, right),
                1 => format!("print({}({}))", call, left),
                _ => format!("let v = {} {}\nprint({}(v, {}))", left, op, right, call),
            };
            let outcome = std::panic::catch_unwind(|| compile_to_wasm(&program));
            if outcome.is_err() {
                panics.push(program);
            }
        }
        assert!(
            panics.is_empty(),
            "compiler panicked on random inputs: {:?}",
            panics
        );
    }

    #[test]
    fn comparator_sort_is_stable_and_matches_the_vm() {
        let (numbers, strings) = run_with_argv(
            "print(sort([3, 1, 2], fn(x, y) { return x - y }))\nprint(sort([3, 1, 2], fn(x, y) { return y - x }))\nprint(sort([5], fn(x, y) { return x - y }))\nprint(sort([], fn(x, y) { return x - y }))\nprint(sort([2.5, 1.5, 3.5], fn(x, y) { if (x < y) { return -1 }\n  return 1 }))\nprint(sort([\"bb\", \"a\", \"ccc\"], fn(x, y) { return len(x) - len(y) }))\nlet rows = [[3], [1, 2], [2]]\nprint(sort(rows, fn(x, y) { return len(x) - len(y) }))\nlet rs = [{a: 3}, {a: 1}, {a: 2}, {a: 1}]\nprint(sort(rs, fn(x, y) { return x.a - y.a }))",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec![
                "[1,2,3]".to_string(),
                "[3,2,1]".to_string(),
                "[5]".to_string(),
                "[]".to_string(),
                "[1.5,2.5,3.5]".to_string(),
                "[\"a\",\"bb\",\"ccc\"]".to_string(),
                "[[3],[2],[1,2]]".to_string(),
                "[{\"a\":1},{\"a\":1},{\"a\":2},{\"a\":3}]".to_string(),
            ]
        );
    }

    #[test]
    fn sort_chain_into_let_keeps_record_types() {
        let (numbers, strings) = run_with_argv(
            "let rows = map(range(5), fn(i) { return {a: i} })\nlet ordered = sort(rows, fn(x, y) { return x.a - y.a })\nprint(reduce(ordered, fn(acc, r) { return acc + r.a }, 0))\nprint(map(ordered, fn(r) { return r.a }))\nlet reversed = reverse(ordered)\nprint(map(reversed, fn(r) { return r.a }))",
            &[],
        );
        assert_eq!(numbers, vec![10]);
        assert_eq!(
            strings,
            vec!["[0,1,2,3,4]".to_string(), "[4,3,2,1,0]".to_string(),]
        );
    }

    #[test]
    fn dynamic_record_get_matches_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let o = {a: 1, b: 2}\nlet k = \"b\"\nprint(get(o, k))\nprint(get(o, \"a\"))\nlet missing = \"z\"\nprint(get(o, missing, 9))\nlet s = {x: \"p\", y: \"q\"}\nlet sk = \"y\"\nprint(get(s, sk))\nlet f = {m: 1.5, n: 2.5}\nlet fk = \"m\"\nprint(get(f, fk))\nlet b = {u: true, v: false}\nlet bk = \"v\"\nprint(get(b, bk))",
            &[],
        );
        assert_eq!(numbers, vec![2, 1, 9]);
        assert_eq!(
            strings,
            vec!["q".to_string(), "1.5".to_string(), "false".to_string()]
        );
    }

    #[test]
    fn structural_equality_and_record_lookups_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let a = {x: 1, y: \"s\"}\nlet b = {x: 1, y: \"s\"}\nprint(a == b)\nprint(a == {x: 2, y: \"s\"})\nprint(a != b)\nprint(contains([a], b))\nprint(contains([a], {x: 2, y: \"s\"}))\nprint(index_of([{a: 1}, {a: 2}], {a: 2}))\nprint(get(a, \"y\"))\nprint(get(a, \"missing\", 7))\nprint(has(a, \"x\"))\nprint(has(a, \"z\"))\nprint([1, 2] == [1, 2])\nprint([1, 2] == [1, 3])\nprint([[1], [2, 3]] == [[1], [2, 3]])\nprint(index_of([[1]], [1]))\nprint(contains([[\"a\"]], [\"a\"]))",
            &[],
        );
        assert_eq!(numbers, vec![1, 7, 0]);
        assert_eq!(
            strings,
            vec![
                "true".to_string(),
                "false".to_string(),
                "false".to_string(),
                "true".to_string(),
                "false".to_string(),
                "s".to_string(),
                "true".to_string(),
                "false".to_string(),
                "true".to_string(),
                "false".to_string(),
                "true".to_string(),
                "true".to_string(),
            ]
        );
    }

    #[test]
    fn exec_and_sh_return_vm_shaped_records() {
        let bytes = compile_to_wasm(
            "let r = sh(\"echo hi\")\nprint(r.code)\nprint(r.ok)\nprint(r.timeout)\nprint(r.out.trim())\nprint(len(r.err))",
        )
        .unwrap();
        let printed = Arc::new(Mutex::new(Vec::new()));
        let numbers = Arc::new(Mutex::new(Vec::new()));
        let text_sink = printed.clone();
        let number_sink = numbers.clone();
        execute(
            &bytes,
            &[],
            0,
            0,
            move |value| number_sink.lock().expect("lock").push(value),
            move |text| text_sink.lock().expect("lock").push(text.to_string()),
        )
        .expect("execute");
        assert_eq!(*numbers.lock().expect("lock"), vec![0, 0]);
        assert_eq!(
            *printed.lock().expect("lock"),
            vec!["true".to_string(), "false".to_string(), "hi".to_string()]
        );

        let bytes = compile_to_wasm("exec([\"definitely-not-a-cmd-xyz\"])").unwrap();
        let error = execute(&bytes, &[], 0, 0, |_value| {}, |_text| {})
            .expect_err("missing program must abort");
        assert!(
            error.contains("cannot run 'definitely-not-a-cmd-xyz'"),
            "got {}",
            error
        );
    }

    #[test]
    fn typed_record_fields_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let o = {a: 1, b: \"x\", c: 1.5, d: true}\nprint(o)\nprint(o.a)\nprint(o.b)\nprint(o.c)\nprint(o.d)\no.a = 2\no.b = \"yy\"\no.c = 2.5\no.d = false\nprint(o)\nlet arr = [{a: 1, b: \"q\"}, {a: 2, b: \"r\"}]\nprint(arr)\nprint(map(arr, fn(r) { return r.b }))\nprint(sort(arr, fn(x, y) { return x.a - y.a }))",
            &[],
        );
        assert_eq!(numbers, vec![1]);
        assert_eq!(
            strings,
            vec![
                "{\"a\":1,\"b\":\"x\",\"c\":1.5,\"d\":true}".to_string(),
                "x".to_string(),
                "1.5".to_string(),
                "true".to_string(),
                "{\"a\":2,\"b\":\"yy\",\"c\":2.5,\"d\":false}".to_string(),
                "[{\"a\":1,\"b\":\"q\"},{\"a\":2,\"b\":\"r\"}]".to_string(),
                "[\"q\",\"r\"]".to_string(),
                "[{\"a\":1,\"b\":\"q\"},{\"a\":2,\"b\":\"r\"}]".to_string(),
            ]
        );
    }

    #[test]
    fn read_write_and_which_aliases_work() {
        let path = std::env::temp_dir().join("bx-wasm-alias-test.txt");
        let escaped = path.to_string_lossy().replace('\\', "/");
        let source = format!(
            "let p = \"{}\"\nprint(write(p, \"payload\"))\nprint(read(p))\nprint(len(which(\"definitely-not-a-command-xyz\")))\nprint(type(which(\"definitely-not-a-command-xyz\")))",
            escaped
        );
        let (numbers, strings) = run_with_argv(&source, &[]);
        let _ = std::fs::remove_file(&path);
        assert_eq!(numbers, vec![0, 0]);
        assert_eq!(strings, vec!["payload".to_string(), "str".to_string()]);
    }

    #[test]
    fn collection_builtins_and_numeric_conversions_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "print(float(2))\nprint(float(\"1.5\"))\nprint(int(true))\nlet a = [1, 2]\nprint(push(a, 3))\nprint(a)\nprint(pop(a))\nprint(a)\nprint(contains([1, 2], 2))\nprint(contains([1, 2], 9))\nprint(contains([\"a\", \"b\"], \"b\"))\nprint(contains([1.5, 2.5], 2.5))\nlet s = [\"x\", \"y\"]\nprint(pop(s))\nprint(s)\nlet f = [1.5, 2.5]\nprint(pop(f))\nprint(f)\nlet rows = [[1], [2, 3]]\nprint(pop(rows))\nprint(rows)",
            &[],
        );
        assert_eq!(numbers, vec![1, 3]);
        assert_eq!(
            strings,
            vec![
                "2".to_string(),
                "1.5".to_string(),
                "[1,2,3]".to_string(),
                "[1,2,3]".to_string(),
                "[1,2]".to_string(),
                "true".to_string(),
                "false".to_string(),
                "true".to_string(),
                "true".to_string(),
                "y".to_string(),
                "[\"x\"]".to_string(),
                "2.5".to_string(),
                "[1.5]".to_string(),
                "[2,3]".to_string(),
                "[[1]]".to_string(),
            ]
        );

        let bytes = compile_to_wasm("let a = []\nprint(pop(a))").unwrap();
        let error =
            execute(&bytes, &[], 0, 0, |_value| {}, |_text| {}).expect_err("empty pop must abort");
        assert!(error.contains("pop from empty array"), "got {}", error);

        let bytes = compile_to_wasm("print(float(\"abc\"))").unwrap();
        let error =
            execute(&bytes, &[], 0, 0, |_value| {}, |_text| {}).expect_err("bad float must abort");
        assert!(
            error.contains("cannot convert str to float"),
            "got {}",
            error
        );
    }

    #[test]
    fn exit_assert_and_error_report_through_the_host() {
        let bytes = compile_to_wasm("print(\"before\")\nexit(7)\nprint(\"after\")").unwrap();
        let printed = Arc::new(Mutex::new(Vec::new()));
        let sink = printed.clone();
        let code = execute(
            &bytes,
            &[],
            0,
            0,
            |_value| {},
            move |text| sink.lock().expect("lock").push(text.to_string()),
        )
        .expect("execute");
        assert_eq!(code, 7);
        assert_eq!(*printed.lock().expect("lock"), vec!["before".to_string()]);

        let bytes = compile_to_wasm("assert(false, \"boom\")").unwrap();
        let error =
            execute(&bytes, &[], 0, 0, |_value| {}, |_text| {}).expect_err("assert must fail");
        assert_eq!(error, "boom");

        let bytes = compile_to_wasm("error(\"kaput\")").unwrap();
        let error =
            execute(&bytes, &[], 0, 0, |_value| {}, |_text| {}).expect_err("error must fail");
        assert_eq!(error, "kaput");

        let bytes = compile_to_wasm("assert(true)\nprint(1)").unwrap();
        execute(&bytes, &[], 0, 0, |_value| {}, |_text| {}).expect("assert(true) passes");
    }

    #[test]
    fn break_continue_and_string_ternaries_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "for x in [1, 2, 3] {\n  if (x == 2) { break }\n  print(x)\n}\nfor y in [1, 2, 3] {\n  if (y == 2) { continue }\n  print(y)\n}\nfor i, z in [1, 2] {\n  print(i + z)\n}\nfor s in [\"a\", \"b\"] {\n  if (s == \"b\") { break }\n  print(s)\n}\nfor r in [[1], [2, 3]] {\n  if (len(r) > 1) { break }\n  print(len(r))\n}\nfor ch in \"abc\" {\n  if (ch == \"b\") { continue }\n  print(ch)\n}\nlet w = 0\nwhile (w < 5) {\n  w += 1\n  if (w == 2) { break }\n}\nprint(w)\nprint(1 < 2 ? \"yes\" : \"no\")\nprint(1 < 2 ? (2 < 3 ? \"a\" : \"b\") : \"c\")",
            &[],
        );
        assert_eq!(numbers, vec![1, 1, 3, 1, 3, 1, 2]);
        assert_eq!(
            strings,
            vec![
                "a".to_string(),
                "a".to_string(),
                "c".to_string(),
                "yes".to_string(),
                "a".to_string(),
            ]
        );
    }

    #[test]
    fn unicode_strings_and_character_iteration_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "print(len(\"héllo\"))\nprint(\"héllo\"[1])\nprint(slice(\"héllo\", 1, 4))\nprint(index_of(\"héllo\", \"llo\"))\nprint(upper(\"héllo\"))\nfor ch in \"hé\" {\n  print(ch)\n}\nfor i, ch in \"ab\" {\n  print(str(i) + \":\" + ch)\n}\nlet s = \"日本語\"\nprint(len(s))\nprint(s[0])\nprint(len(\"\"))",
            &[],
        );
        assert_eq!(numbers, vec![5, 2, 3, 0]);
        assert_eq!(
            strings,
            vec![
                "é".to_string(),
                "éll".to_string(),
                "HÉLLO".to_string(),
                "h".to_string(),
                "é".to_string(),
                "0:a".to_string(),
                "1:b".to_string(),
                "日".to_string(),
            ]
        );
    }

    #[test]
    fn string_array_rows_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let rows = [[\"a\", \"b\"], [\"c\"]]\nprint(rows)\nprint(len(rows))\nprint(rows[1][0])\nfor row in rows {\n  print(row)\n}\npush(rows, [\"z\"])\nprint(rows)\nrows[0] = [\"q\"]\nprint(rows)\nprint(map(rows, fn(r) { return len(r) }))\nprint(filter(rows, fn(r) { return len(r) > 1 }))\neach(rows, fn(r) { print(r[0]) })\nprint(sort([[\"a\"], [\"b\", \"c\"]], fn(x, y) { return len(x) - len(y) }))\nprint(reverse([[\"a\"], [\"b\"]]))\nprint(slice([[\"a\"], [\"b\"], [\"c\"]], 1, 2))\nlet f = fn() { return rows[0][0] }\nprint(f())",
            &[],
        );
        assert_eq!(numbers, vec![2]);
        assert_eq!(
            strings,
            vec![
                "[[\"a\",\"b\"],[\"c\"]]".to_string(),
                "c".to_string(),
                "[\"a\",\"b\"]".to_string(),
                "[\"c\"]".to_string(),
                "[[\"a\",\"b\"],[\"c\"],[\"z\"]]".to_string(),
                "[[\"q\"],[\"c\"],[\"z\"]]".to_string(),
                "[1,1,1]".to_string(),
                "[]".to_string(),
                "q".to_string(),
                "c".to_string(),
                "z".to_string(),
                "[[\"a\"],[\"b\",\"c\"]]".to_string(),
                "[[\"b\"],[\"a\"]]".to_string(),
                "[[\"b\"]]".to_string(),
                "q".to_string(),
            ]
        );
    }

    #[test]
    fn array_of_arrays_and_reduce_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "let rows = [[1, 2], [3], []]\nprint(map(rows, fn(r) { return sum(r) }))\nprint(filter(rows, fn(r) { return len(r) > 0 }))\neach(rows, fn(r) { print(len(r)) })\nprint(reduce(rows, fn(acc, r) { return acc + len(r) }, 0))\nprint(reduce([1.5, 2.5], fn(a, x) { return a + x }, 0.0))\nprint(reduce([\"a\", \"b\"], fn(a, x) { return a + x }, \"\"))\nprint(reduce([\"a\", \"b\"], fn(a, x) { return a + x }))\nprint(reduce([{a: 1}, {a: 2}], fn(acc, r) { return acc + r.a }, 0))\nprint(sort(rows, fn(a, b) { return len(a) - len(b) }))\nprint(reverse(rows))",
            &[],
        );
        assert_eq!(numbers, vec![2, 1, 0, 3, 3]);
        assert_eq!(
            strings,
            vec![
                "[3,3,0]".to_string(),
                "[[1,2],[3]]".to_string(),
                "4".to_string(),
                "ab".to_string(),
                "ab".to_string(),
                "[[],[3],[1,2]]".to_string(),
                "[[1,2],[3],[]]".to_string(),
            ]
        );
    }

    #[test]
    fn record_arrays_support_map_filter_each_and_comparator_sort() {
        let (numbers, strings) = run_with_argv(
            "let xs = [{a: 2}, {a: 1}]\nprint(map(xs, fn(r) { return r.a * 10 }))\nprint(map(xs, fn(r) { return \"v\" + str(r.a) }))\nprint(map(xs, fn(r) { return {b: r.a + 1} }))\nprint(filter(xs, fn(r) { return r.a > 1 }))\neach(xs, fn(r) { print(r.a) })\nprint(sort(xs, fn(x, y) { return x.a - y.a }))\nprint(reverse([{a: 1}, {a: 2}]))\nprint(slice([{a: 1}, {a: 2}, {a: 3}], 1, 2))\nprint(sort([2.5, 1.5], fn(x, y) { return int((x - y) * 10) }))",
            &[],
        );
        assert_eq!(numbers, vec![2, 1]);
        assert_eq!(
            strings,
            vec![
                "[20,10]".to_string(),
                "[\"v2\",\"v1\"]".to_string(),
                "[{\"b\":3},{\"b\":2}]".to_string(),
                "[{\"a\":2}]".to_string(),
                "[{\"a\":1},{\"a\":2}]".to_string(),
                "[{\"a\":2},{\"a\":1}]".to_string(),
                "[{\"a\":2}]".to_string(),
                "[1.5,2.5]".to_string(),
            ]
        );
    }

    #[test]
    fn numeric_and_string_array_helpers_match_the_vm() {
        let (numbers, strings) = run_with_argv(
            "print(sort([\"b\", \"c\", \"a\"]))\nprint(sort([2.5, 1.5]))\nlet words = [\"a\", \"b\"]\nlet reversed = reverse(words)\nprint(words)\nprint(reversed)\nlet floats = [1.5, 0.5]\nreverse(floats)\nprint(floats)\nprint(slice([\"a\", \"b\", \"c\"], 1, 2))\nprint(slice([1.5, 2.5, 3.5], 0, 2))\nprint(join([\"a\", \"b\"]))\nprint(join([1, 2], \"-\"))\nprint(join([1.5, 2], \"+\"))\nlet unsorted = [\"b\", \"a\"]\nsort(unsorted)\nprint(unsorted)",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec![
                "[\"a\",\"b\",\"c\"]".to_string(),
                "[1.5,2.5]".to_string(),
                "[\"b\",\"a\"]".to_string(),
                "[\"b\",\"a\"]".to_string(),
                "[0.5,1.5]".to_string(),
                "[\"b\"]".to_string(),
                "[1.5,2.5]".to_string(),
                "ab".to_string(),
                "1-2".to_string(),
                "1.5+2".to_string(),
                "[\"a\",\"b\"]".to_string(),
            ]
        );
    }

    #[test]
    fn array_search_and_minmax_builtins() {
        let (numbers, strings) = run_with_argv(
            "print(index_of([1, 2, 3], 2))\nprint(index_of([1, 2], 9))\nprint(index_of([\"a\", \"b\"], \"b\"))\nprint([\"a\", \"b\"].contains(\"b\"))\nprint([1, 2, 3].contains(4))\nprint([1, 2].index_of(2))\nprint(min([3, 1, 2]))\nprint(max([3, 1, 2]))\nprint(min([\"b\", \"a\", \"c\"]))\nprint(max([\"b\", \"a\"]))\nprint(index_of([], 1))\nprint(\"abc\".index_of(\"b\"))\nprint(\"abc\".contains(\"bc\"))",
            &[],
        );
        assert_eq!(numbers, vec![1, -1, 1, 1, 1, 3, -1, 1]);
        assert_eq!(
            strings,
            vec![
                "true".to_string(),
                "false".to_string(),
                "a".to_string(),
                "b".to_string(),
                "true".to_string(),
            ]
        );
    }

    #[test]
    fn maps_print_and_support_record_builtins() {
        let (numbers, strings) = run_with_argv(
            "let m = map_new()\nmap_set(m, \"one\", 1)\nmap_set(m, \"two\", 2)\nprint(m)\nprint(len(keys(m)))\nprint(join(keys(m), \"+\"))\nprint(values(m))\nprint(has(m, \"one\"))\nprint(has(m, \"zzz\"))\nprint(get(m, \"two\"))\nprint(get(m, \"zzz\", 9))\nprint(len(m))\nfor k, v in m {\n  print(k + \"=\" + str(v))\n}",
            &[],
        );
        assert_eq!(numbers, vec![2, 2, 9, 2]);
        assert_eq!(
            strings,
            vec![
                "{\"one\":1,\"two\":2}".to_string(),
                "one+two".to_string(),
                "[1,2]".to_string(),
                "true".to_string(),
                "false".to_string(),
                "one=1".to_string(),
                "two=2".to_string(),
            ]
        );
    }

    #[test]
    fn sort_strings_with_comparator() {
        let (numbers, strings) = run_with_argv(
            "let xs = [\"bbb\", \"a\", \"cc\"]\nfor v in sort(xs, fn(x, y) { return len(x) - len(y) }) {\n  print(v)\n}\nfn prefixed(base) {\n  return sort([\"bb\", \"a\", \"ccc\"], fn(x, y) { return (len(x) - len(y)) + base })\n}\nfor v in prefixed(0) {\n  print(v)\n}\nprint(sort([\"b\", \"a\"], fn(x, y) { return len(x) - len(y) }))",
            &[],
        );
        assert!(numbers.is_empty());
        assert_eq!(
            strings,
            vec![
                "a".to_string(),
                "cc".to_string(),
                "bbb".to_string(),
                "a".to_string(),
                "bb".to_string(),
                "ccc".to_string(),
                "[\"b\",\"a\"]".to_string(),
            ]
        );
    }

    #[test]
    fn sort_with_comparator() {
        let (numbers, strings) = run_with_argv(
            "let a = [3, 1, 2]\nlet b = sort(a, fn(x, y) { return x - y })\nprint(a[0])\nprint(b[0])\nfor v in sort([5, 9, 1, 4], fn(x, y) { return y - x }) {\n  print(v)\n}\nfn with_capture(offset) {\n  let limit = 0\n  return sort([3, 2, 1], fn(x, y) { return (x - y) + limit + offset })\n}\nfor v in with_capture(0) {\n  print(v)\n}\nprint(len(sort([], fn(x, y) { return x - y })))",
            &[],
        );
        assert!(strings.is_empty());
        assert_eq!(numbers, vec![1, 1, 9, 5, 4, 1, 1, 2, 3, 0]);
    }

    #[test]
    fn record_arrays_work() {
        let (numbers, strings) = run_with_argv(
            "let xs = [{a: 1}, {a: 2}]\nprint(xs[0].a)\nprint(len(xs))\nfor r in xs {\n  print(r.a)\n}\nxs[1] = {a: 9}\nprint(xs[-1].a)\nfor i, r in xs {\n  print(i + r.a)\n}\nfn total(rows) {\n  let s = 0\n  for r in rows {\n    s = s + r.a\n  }\n  return s\n}\nprint(total([{a: 1}, {a: 2}, {a: 3}]))\nprint(type(xs[0]))",
            &[],
        );
        assert_eq!(numbers, vec![1, 2, 1, 2, 9, 1, 10, 6]);
        assert_eq!(strings, vec!["object".to_string()]);
    }

    #[test]
    fn loop_variables_are_capturable_over_any_iterable() {
        let (numbers, strings) = run_with_argv(
            "for s in [\"a\", \"bb\"] {\n  print(fn() { return s + \"!\" }())\n}\nlet o = {x: 1, y: 2}\nfor k in o {\n  print(fn() { return k + \"?\" }())\n}\nfor k, v in o {\n  print(fn() { return k + \":\" + str(v) }())\n}\nfor i in range(2) {\n  print(fn() { return i * 5 }())\n}",
            &[],
        );
        assert_eq!(numbers, vec![0, 5]);
        assert_eq!(
            strings,
            vec![
                "a!".to_string(),
                "bb!".to_string(),
                "x?".to_string(),
                "y?".to_string(),
                "x:1".to_string(),
                "y:2".to_string(),
            ]
        );
    }

    #[test]
    fn capturing_function_literals_share_cells() {
        let (numbers, strings) = run_with_argv(
            "fn adder(n) {\n  return fn(x) { return x + n }\n}\nlet add2 = adder(2)\nprint(add2(5))\nfn counter() {\n  let n = 0\n  return fn() { n = n + 1; return n }\n}\nlet c = counter()\nprint(c())\nprint(c())\nlet d = counter()\nprint(d())\nprint(c())\nfn compose(f, x) { return f(x) }\nprint(compose(fn(y) { return y * 3 }, 4))",
            &[],
        );
        assert!(strings.is_empty());
        assert_eq!(numbers, vec![7, 1, 2, 1, 3, 12]);
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
        assert!(compile_to_wasm("let f = fn() { return 1 }\nprint(f)").is_err());
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
        assert!(compile_to_wasm("print(unknown_name)").is_err());
    }
}
