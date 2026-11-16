use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::rc::Rc;

#[derive(Debug)]
pub struct FuncRef {
    pub chunk: usize,
    pub name: String,
}

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<String>),
    Array(Rc<RefCell<Vec<Value>>>),
    Object(Rc<RefCell<BTreeMap<String, Value>>>),
    Func(Rc<FuncRef>),
    Builtin(&'static str),
}

pub fn str_val(s: impl Into<String>) -> Value {
    Value::Str(Rc::new(s.into()))
}

pub fn array_val(v: Vec<Value>) -> Value {
    Value::Array(Rc::new(RefCell::new(v)))
}

pub fn object_val(m: BTreeMap<String, Value>) -> Value {
    Value::Object(Rc::new(RefCell::new(m)))
}

pub fn fmt_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    format!("{}", f)
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0 && !f.is_nan(),
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Func(_) | Value::Builtin(_) => "fn",
        }
    }

    pub fn to_num(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Null => Some(0.0),
            Value::Str(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_str_val(&self) -> Option<String> {
        match self {
            Value::Str(s) => Some((**s).clone()),
            _ => None,
        }
    }

    /// Human/agent facing text form. Strings print raw, everything else as JSON.
    pub fn display(&self) -> String {
        match self {
            Value::Str(s) => (**s).clone(),
            _ => self.to_json(),
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write_json(&mut s, 0);
        s
    }

    fn write_json(&self, out: &mut String, depth: usize) {
        if depth > 96 {
            out.push_str("null");
            return;
        }
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(i) => {
                let _ = write!(out, "{}", i);
            }
            Value::Float(f) => {
                if f.is_finite() {
                    let s = fmt_float(*f);
                    out.push_str(&s);
                } else {
                    out.push_str("null");
                }
            }
            Value::Str(s) => write_json_str(out, s),
            Value::Array(a) => {
                out.push('[');
                for (i, v) in a.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_json(out, depth + 1);
                }
                out.push(']');
            }
            Value::Object(o) => {
                out.push('{');
                for (i, (k, v)) in o.borrow().iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_str(out, k);
                    out.push(':');
                    v.write_json(out, depth + 1);
                }
                out.push('}');
            }
            Value::Func(f) => {
                let _ = write!(out, "\"<fn {}>\"", f.name);
            }
            Value::Builtin(n) => {
                let _ = write!(out, "\"<builtin {}>\"", n);
            }
        }
    }
}

pub fn write_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn num_eq(a: &Value, b: &Value) -> Option<bool> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x == y),
        _ => match (a.to_num(), b.to_num()) {
            (Some(x), Some(y)) => Some(x == y),
            _ => None,
        },
    }
}

fn eq(a: &Value, b: &Value, depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y),
        (Value::Builtin(x), Value::Builtin(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            let xb = x.borrow();
            let yb = y.borrow();
            xb.len() == yb.len() && xb.iter().zip(yb.iter()).all(|(p, q)| eq(p, q, depth + 1))
        }
        (Value::Object(x), Value::Object(y)) => {
            let xb = x.borrow();
            let yb = y.borrow();
            xb.len() == yb.len()
                && xb
                    .iter()
                    .all(|(k, v)| yb.get(k).map(|w| eq(v, w, depth + 1)).unwrap_or(false))
        }
        _ => num_eq(a, b).unwrap_or(false),
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        eq(self, other, 0)
    }
}
