use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::builtins;
use crate::chunk::{has_operand, op, Chunk};
use crate::value::{array_val, object_val, str_val, FuncRef, Value};
use std::rc::Rc;

const MAX_DEPTH: usize = 220;

pub struct Vm {
    pub chunks: Vec<Chunk>,
    pub globals: BTreeMap<String, Value>,
    pub stack: Vec<Value>,
    pub script_name: String,
    depth: usize,
    steps_left: u64,
    ticks: u64,
    /// Wall-clock budget; checked every few thousand instructions.
    deadline: Option<std::time::Instant>,
}

impl Vm {
    pub fn new(
        chunks: Vec<Chunk>,
        script_name: String,
        argv: Vec<Value>,
        script_dir: String,
    ) -> Vm {
        let mut globals: BTreeMap<String, Value> = BTreeMap::new();
        for b in builtins::BUILTINS {
            globals.insert(b.name.to_string(), Value::Builtin(b.name));
        }
        globals.insert("argv".to_string(), array_val(argv.clone()));
        globals.insert("script".to_string(), str_val(script_name.clone()));
        globals.insert("__dir__".to_string(), str_val(script_dir));
        let steps = std::env::var("BX_MAX_STEPS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(1_000_000_000);
        let timeout_ms = std::env::var("BX_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let deadline = if timeout_ms > 0 {
            Some(std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms))
        } else {
            None
        };
        Vm {
            chunks,
            globals,
            stack: Vec::with_capacity(256),
            script_name,
            depth: 0,
            steps_left: steps,
            ticks: 0,
            deadline,
        }
    }

    pub fn run(&mut self) -> Result<Value, String> {
        self.run_frame(0, Vec::new(), Vec::new())
    }

    pub fn call_value(
        &mut self,
        callee: Value,
        args: Vec<Value>,
        _line: u32,
    ) -> Result<Value, String> {
        match callee {
            Value::Func(f) => self.run_frame(f.chunk, f.upvalues.clone(), args),
            Value::Builtin(name) => builtins::call(self, name, &args),
            other => Err(format!("{} value is not callable", other.type_name())),
        }
    }

    #[allow(dead_code)]
    pub fn get_global(&self, name: &str) -> Option<Value> {
        self.globals.get(name).cloned()
    }

    fn run_frame(
        &mut self,
        ci: usize,
        mut upvalues: Vec<Value>,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        if ci >= self.chunks.len() {
            return Err("bytecode references a missing function chunk".to_string());
        }
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(format!("call depth exceeded ({} frames)", MAX_DEPTH));
        }
        let chunk_name = self.chunks[ci].name.clone();
        let chunk_file = self.chunks[ci].file.clone();
        let arity = self.chunks[ci].arity as usize;
        let nslots = (self.chunks[ci].max_slots as usize).max(arity).max(1);
        let mut slots = vec![Value::Null; nslots];
        for (i, a) in args.into_iter().enumerate() {
            if i < slots.len() {
                slots[i] = a;
            }
        }
        let base = self.stack.len();
        let mut ip: usize = 0;

        let outcome: Result<Value, String> = loop {
            let (o, arg, next_ip, line) = {
                let ch = &self.chunks[ci];
                if ip >= ch.code.len() {
                    break Err(format!(
                        "{}: bytecode ran past the end of chunk '{}'",
                        self.script_name, chunk_name
                    ));
                }
                let o = ch.code[ip];
                if has_operand(o) {
                    if ip + 5 > ch.code.len() {
                        break Err(format!("{}: truncated instruction", self.script_name));
                    }
                    let a = i32::from_le_bytes([
                        ch.code[ip + 1],
                        ch.code[ip + 2],
                        ch.code[ip + 3],
                        ch.code[ip + 4],
                    ]);
                    (o, a, ip + 5, ch.lines.get(ip).copied().unwrap_or(0))
                } else {
                    (o, 0, ip + 1, ch.lines.get(ip).copied().unwrap_or(0))
                }
            };
            ip = next_ip;
            self.steps_left = self.steps_left.saturating_sub(1);
            if self.steps_left == 0 {
                break Err(format!(
                    "{}: execution step limit exceeded (raise BX_MAX_STEPS or pass --max-steps)",
                    self.script_name
                ));
            }
            self.ticks = self.ticks.wrapping_add(1);
            if let Some(deadline) = self.deadline {
                if self.ticks & 0x0FFF == 0 && std::time::Instant::now() >= deadline {
                    break Err(format!(
                        "{}: execution timeout exceeded (raise --timeout)",
                        self.script_name
                    ));
                }
            }

            macro_rules! fail {
                ($($t:tt)*) => {{
                    let context = if chunk_name == "<main>" {
                        String::new()
                    } else {
                        format!(" in {}()", chunk_name)
                    };
                    // Source file of the chunk, when it is not the main script.
                    let origin = if chunk_name != "<main>"
                        && !chunk_file.is_empty()
                        && chunk_file != self.script_name
                    {
                        format!(" [{}]", chunk_file)
                    } else {
                        String::new()
                    };
                    break Err(format!(
                        "{}({}){}{}: {}",
                        self.script_name,
                        line,
                        context,
                        origin,
                        format!($($t)*)
                    ))
                }};
            }
            macro_rules! popv {
                () => {
                    match self.stack.pop() {
                        Some(v) => v,
                        None => fail!("internal stack underflow"),
                    }
                };
            }
            macro_rules! const_at {
                ($i:expr) => {
                    match self.chunks[ci].consts.get($i as usize) {
                        Some(v) => v.clone(),
                        None => fail!("bad constant index {}", $i),
                    }
                };
            }

            match o {
                op::HALT => break Ok(self.stack.pop().unwrap_or(Value::Null)),
                op::CONST => self.stack.push(const_at!(arg)),
                op::NULL => self.stack.push(Value::Null),
                op::TRUE => self.stack.push(Value::Bool(true)),
                op::FALSE => self.stack.push(Value::Bool(false)),
                op::POP => {
                    popv!();
                }
                op::DUP => match self.stack.last() {
                    Some(v) => {
                        let v = v.clone();
                        self.stack.push(v)
                    }
                    None => fail!("internal stack underflow"),
                },
                op::GET_LOCAL => {
                    let i = arg as usize;
                    match slots.get(i) {
                        Some(v) => self.stack.push(v.clone()),
                        None => fail!("bad local slot {}", i),
                    }
                }
                op::SET_LOCAL => {
                    let i = arg as usize;
                    let v = popv!();
                    match slots.get_mut(i) {
                        Some(s) => *s = v,
                        None => fail!("bad local slot {}", i),
                    }
                }
                op::GET_GLOBAL => {
                    let name = match const_at!(arg) {
                        Value::Str(s) => (*s).clone(),
                        _ => fail!("bad global name constant"),
                    };
                    match self.globals.get(&name) {
                        Some(v) => self.stack.push(v.clone()),
                        None => fail!("undefined variable '{}'", name),
                    }
                }
                op::SET_GLOBAL => {
                    let name = match const_at!(arg) {
                        Value::Str(s) => (*s).clone(),
                        _ => fail!("bad global name constant"),
                    };
                    let v = popv!();
                    self.globals.insert(name, v);
                }
                op::ADD => {
                    let b = popv!();
                    let a = popv!();
                    match add(a, b) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::SUB => {
                    let b = popv!();
                    let a = popv!();
                    match arith(a, b, |x, y| x - y) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::MUL => {
                    let b = popv!();
                    let a = popv!();
                    match arith(a, b, |x, y| x * y) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::DIV => {
                    let b = popv!();
                    let a = popv!();
                    match div(a, b) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::MOD => {
                    let b = popv!();
                    let a = popv!();
                    match arith(a, b, |x, y| x % y) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::NEG => {
                    let a = popv!();
                    match a {
                        Value::Int(i) => self.stack.push(Value::Int(-i)),
                        Value::Float(f) => self.stack.push(Value::Float(-f)),
                        other => match other.to_num() {
                            Some(n) => self.stack.push(Value::Float(-n)),
                            None => fail!("cannot negate {}", other.type_name()),
                        },
                    }
                }
                op::NOT => {
                    let a = popv!();
                    self.stack.push(Value::Bool(!a.truthy()));
                }
                op::EQ => {
                    let b = popv!();
                    let a = popv!();
                    self.stack.push(Value::Bool(a == b));
                }
                op::NE => {
                    let b = popv!();
                    let a = popv!();
                    self.stack.push(Value::Bool(!(a == b)));
                }
                op::LT => {
                    let b = popv!();
                    let a = popv!();
                    match compare(&a, &b) {
                        Ok(o) => self.stack.push(Value::Bool(o == Ordering::Less)),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::LE => {
                    let b = popv!();
                    let a = popv!();
                    match compare(&a, &b) {
                        Ok(o) => self.stack.push(Value::Bool(o != Ordering::Greater)),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::GT => {
                    let b = popv!();
                    let a = popv!();
                    match compare(&a, &b) {
                        Ok(o) => self.stack.push(Value::Bool(o == Ordering::Greater)),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::GE => {
                    let b = popv!();
                    let a = popv!();
                    match compare(&a, &b) {
                        Ok(o) => self.stack.push(Value::Bool(o != Ordering::Less)),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::JUMP => ip = arg as usize,
                op::JUMP_IF_FALSE => {
                    let c = popv!();
                    if !c.truthy() {
                        ip = arg as usize;
                    }
                }
                op::JUMP_IF_TRUE => {
                    let c = popv!();
                    if c.truthy() {
                        ip = arg as usize;
                    }
                }
                op::JUMP_IF_FALSE_OR_POP => {
                    let keep = match self.stack.last() {
                        Some(v) => v.truthy(),
                        None => fail!("internal stack underflow"),
                    };
                    if keep {
                        popv!();
                    } else {
                        ip = arg as usize;
                    }
                }
                op::JUMP_IF_TRUE_OR_POP => {
                    let keep = match self.stack.last() {
                        Some(v) => v.truthy(),
                        None => fail!("internal stack underflow"),
                    };
                    if !keep {
                        popv!();
                    } else {
                        ip = arg as usize;
                    }
                }
                op::CALL => {
                    let argc = arg.max(0) as usize;
                    if self.stack.len() < argc + 1 {
                        fail!("internal stack underflow at call");
                    }
                    let call_args: Vec<Value> = self.stack.split_off(self.stack.len() - argc);
                    let callee = popv!();
                    match self.call_value(callee, call_args, line) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => {
                            // The callee already reports its own location and
                            // function; do not stack a second prefix on it.
                            if e.starts_with(self.script_name.as_str()) {
                                break Err(e);
                            }
                            break Err(format!("{}({}): {}", self.script_name, line, e));
                        }
                    }
                }
                op::RET => {
                    let v = self.stack.pop().unwrap_or(Value::Null);
                    break Ok(v);
                }
                op::MAKE_ARRAY => {
                    let n = arg.max(0) as usize;
                    if self.stack.len() < n {
                        fail!("internal stack underflow building array");
                    }
                    let items = self.stack.split_off(self.stack.len() - n);
                    self.stack.push(array_val(items));
                }
                op::MAKE_OBJECT => {
                    let n = arg.max(0) as usize;
                    if self.stack.len() < n * 2 {
                        fail!("internal stack underflow building object");
                    }
                    let flat = self.stack.split_off(self.stack.len() - n * 2);
                    let mut map = BTreeMap::new();
                    let mut it = flat.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        let key = match k {
                            Value::Str(s) => (*s).clone(),
                            other => other.display(),
                        };
                        map.insert(key, v);
                    }
                    self.stack.push(object_val(map));
                }
                op::INDEX_GET => {
                    let idx = popv!();
                    let obj = popv!();
                    match index_get(&obj, &idx) {
                        Ok(v) => self.stack.push(v),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::INDEX_SET => {
                    let val = popv!();
                    let idx = popv!();
                    let obj = popv!();
                    match index_set(&obj, &idx, val) {
                        Ok(()) => {}
                        Err(e) => fail!("{}", e),
                    }
                }
                op::LEN => {
                    let v = popv!();
                    match len_of(&v) {
                        Ok(n) => self.stack.push(Value::Int(n as i64)),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::ITER_PREP => {
                    let v = popv!();
                    match iterable(v) {
                        Ok(a) => self.stack.push(a),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::ITER_KEYS => {
                    let v = popv!();
                    match iterable_keys(v) {
                        Ok(a) => self.stack.push(a),
                        Err(e) => fail!("{}", e),
                    }
                }
                op::GET_UPVALUE => {
                    let i = arg as usize;
                    match upvalues.get(i) {
                        Some(Value::Cell(cell)) => {
                            let value = cell.borrow().clone();
                            self.stack.push(value);
                        }
                        Some(v) => self.stack.push(v.clone()),
                        None => fail!("bad upvalue index {}", i),
                    }
                }
                op::GET_UPVALUE_RAW => {
                    let i = arg as usize;
                    match upvalues.get(i) {
                        Some(v) => self.stack.push(v.clone()),
                        None => fail!("bad upvalue index {}", i),
                    }
                }
                op::SET_UPVALUE => {
                    let value = popv!();
                    let i = arg as usize;
                    match upvalues.get(i) {
                        Some(Value::Cell(cell)) => {
                            *cell.borrow_mut() = value;
                        }
                        Some(_) => {
                            upvalues[i] = value;
                        }
                        None => fail!("bad upvalue index {}", i),
                    }
                }
                op::WRAP_CELL => {
                    let value = popv!();
                    self.stack
                        .push(Value::Cell(Rc::new(std::cell::RefCell::new(value))));
                }
                op::GET_CELL => {
                    let slot = arg as usize;
                    match slots.get(slot) {
                        Some(Value::Cell(cell)) => {
                            let value = cell.borrow().clone();
                            self.stack.push(value);
                        }
                        _ => fail!("local {} is not a cell", slot),
                    }
                }
                op::SET_CELL => {
                    let value = popv!();
                    let slot = arg as usize;
                    match slots.get_mut(slot) {
                        Some(Value::Cell(cell)) => {
                            *cell.borrow_mut() = value;
                        }
                        _ => fail!("local {} is not a cell", slot),
                    }
                }
                op::MAKE_CLOSURE => {
                    let n = arg.max(0) as usize;
                    if self.stack.len() < n + 1 {
                        fail!("internal stack underflow creating a closure");
                    }
                    let captured = self.stack.split_off(self.stack.len() - n);
                    let callee = popv!();
                    match callee {
                        Value::Func(f) => {
                            let mut closure = FuncRef::new(f.chunk, f.name.clone());
                            closure.upvalues = captured;
                            self.stack.push(Value::Func(Rc::new(closure)));
                        }
                        other => fail!("cannot close over {}", other.type_name()),
                    }
                }
                op::DUP2 => {
                    if self.stack.len() < 2 {
                        fail!("internal stack underflow");
                    }
                    let n = self.stack.len();
                    let a = self.stack[n - 2].clone();
                    let b = self.stack[n - 1].clone();
                    self.stack.push(a);
                    self.stack.push(b);
                }
                op::INDEX_SET_KEEP => {
                    let val = popv!();
                    let idx = popv!();
                    let obj = popv!();
                    match index_set(&obj, &idx, val.clone()) {
                        Ok(()) => self.stack.push(val),
                        Err(e) => fail!("{}", e),
                    }
                }
                other => fail!("unknown opcode {}", other),
            }
        };

        self.stack.truncate(base);
        self.depth -= 1;
        outcome
    }
}

/// The value a `for … in` walks: arrays are shared as-is, strings become their
/// characters, objects become their keys. Anything else is a clear error rather
/// than the silent index-into-a-map that used to yield `null`.
pub fn iterable(v: Value) -> Result<Value, String> {
    match v {
        Value::Array(_) => Ok(v),
        Value::Str(s) => Ok(array_val(
            s.chars().map(|c| str_val(c.to_string())).collect(),
        )),
        Value::Object(o) => Ok(array_val(
            o.borrow().keys().map(|k| str_val(k.clone())).collect(),
        )),
        other => Err(format!("cannot iterate over {}", other.type_name())),
    }
}

/// The iteration keys a `for k, v in` walks: an object's keys, or the indices of
/// an array/string. `v` is then always `source[k]`, which works for all three.
pub fn iterable_keys(v: Value) -> Result<Value, String> {
    match v {
        Value::Object(o) => Ok(array_val(
            o.borrow().keys().map(|k| str_val(k.clone())).collect(),
        )),
        Value::Array(a) => Ok(array_val(
            (0..a.borrow().len())
                .map(|i| Value::Int(i as i64))
                .collect(),
        )),
        Value::Str(s) => Ok(array_val(
            (0..s.chars().count())
                .map(|i| Value::Int(i as i64))
                .collect(),
        )),
        other => Err(format!("cannot iterate over {}", other.type_name())),
    }
}

pub fn len_of(v: &Value) -> Result<usize, String> {
    match v {
        Value::Str(s) => Ok(s.chars().count()),
        Value::Array(a) => Ok(a.borrow().len()),
        Value::Object(o) => Ok(o.borrow().len()),
        other => Err(format!("len() does not apply to {}", other.type_name())),
    }
}

fn num_pair(a: &Value, b: &Value) -> Result<(f64, f64), String> {
    match (a.to_num(), b.to_num()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(format!(
            "cannot apply arithmetic to {} and {}",
            a.type_name(),
            b.type_name()
        )),
    }
}

fn add(a: Value, b: Value) -> Result<Value, String> {
    if let (Value::Array(x), Value::Array(y)) = (&a, &b) {
        let mut out = x.borrow().clone();
        out.extend(y.borrow().iter().cloned());
        return Ok(array_val(out));
    }
    if matches!(a, Value::Str(_)) || matches!(b, Value::Str(_)) {
        return Ok(str_val(format!("{}{}", a.display(), b.display())));
    }
    if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
        if let Some(v) = x.checked_add(*y) {
            return Ok(Value::Int(v));
        }
    }
    let (x, y) = num_pair(&a, &b)?;
    Ok(Value::Float(x + y))
}

fn arith(a: Value, b: Value, f: fn(f64, f64) -> f64) -> Result<Value, String> {
    if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
        let r = f(*x as f64, *y as f64);
        if r.fract() == 0.0 && r.abs() < 9.0e15 {
            return Ok(Value::Int(r as i64));
        }
        return Ok(Value::Float(r));
    }
    let (x, y) = num_pair(&a, &b)?;
    Ok(Value::Float(f(x, y)))
}

fn div(a: Value, b: Value) -> Result<Value, String> {
    let (x, y) = num_pair(&a, &b)?;
    if y == 0.0 {
        return Err("division by zero".to_string());
    }
    Ok(Value::Float(x / y))
}

pub fn compare(a: &Value, b: &Value) -> Result<Ordering, String> {
    if let (Value::Str(x), Value::Str(y)) = (a, b) {
        return Ok(x.cmp(y));
    }
    if let (Some(x), Some(y)) = (a.to_num(), b.to_num()) {
        return x
            .partial_cmp(&y)
            .ok_or_else(|| "cannot compare NaN".to_string());
    }
    Err(format!(
        "cannot compare {} with {}",
        a.type_name(),
        b.type_name()
    ))
}

fn as_index(v: &Value) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i64),
        Value::Str(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("index '{}' is not a number", s)),
        other => Err(format!("{} cannot be used as an index", other.type_name())),
    }
}

pub fn index_get(obj: &Value, idx: &Value) -> Result<Value, String> {
    match obj {
        Value::Array(a) => {
            let i = as_index(idx)?;
            let a = a.borrow();
            let n = a.len() as i64;
            let j = if i < 0 { n + i } else { i };
            if j < 0 || j >= n {
                return Ok(Value::Null);
            }
            Ok(a[j as usize].clone())
        }
        Value::Str(s) => {
            let i = as_index(idx)?;
            let chars: Vec<char> = s.chars().collect();
            let n = chars.len() as i64;
            let j = if i < 0 { n + i } else { i };
            if j < 0 || j >= n {
                return Ok(Value::Null);
            }
            Ok(str_val(chars[j as usize].to_string()))
        }
        Value::Object(o) => {
            let key = match idx {
                Value::Str(s) => (**s).clone(),
                other => other.display(),
            };
            Ok(o.borrow().get(&key).cloned().unwrap_or(Value::Null))
        }
        Value::Null => Err("cannot index null (missing value?)".to_string()),
        other => Err(format!("cannot index {}", other.type_name())),
    }
}

pub fn index_set(obj: &Value, idx: &Value, val: Value) -> Result<(), String> {
    match obj {
        Value::Array(a) => {
            let i = as_index(idx)?;
            let mut a = a.borrow_mut();
            let n = a.len() as i64;
            let j = if i < 0 { n + i } else { i };
            if j < 0 {
                return Err(format!("array index {} out of range", i));
            }
            let j = j as usize;
            if j < a.len() {
                a[j] = val;
            } else if j == a.len() {
                a.push(val);
            } else {
                return Err(format!(
                    "array index {} out of range (length {})",
                    j,
                    a.len()
                ));
            }
            Ok(())
        }
        Value::Object(o) => {
            let key = match idx {
                Value::Str(s) => (**s).clone(),
                other => other.display(),
            };
            o.borrow_mut().insert(key, val);
            Ok(())
        }
        Value::Str(_) => Err("strings are immutable".to_string()),
        other => Err(format!("cannot assign into {}", other.type_name())),
    }
}
