use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crate::json;
use crate::value::{array_val, object_val, str_val, Value};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::vm::{compare, len_of, Spawned, Vm};

pub struct Builtin {
    pub name: &'static str,
    pub sig: &'static str,
    pub doc: &'static str,
    pub f: fn(&mut Vm, &[Value]) -> Result<Value, String>,
}

pub fn call(vm: &mut Vm, name: &str, args: &[Value]) -> Result<Value, String> {
    for b in BUILTINS {
        if b.name == name {
            return (b.f)(vm, args);
        }
    }
    Err(format!("unknown builtin '{}'", name))
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn arg(args: &[Value], i: usize) -> Result<&Value, String> {
    args.get(i)
        .ok_or_else(|| format!("missing argument #{}", i + 1))
}

fn s_arg(args: &[Value], i: usize) -> Result<String, String> {
    match arg(args, i)? {
        Value::Str(s) => Ok((**s).clone()),
        other => Err(format!(
            "argument #{} must be a string, got {}",
            i + 1,
            other.type_name()
        )),
    }
}

fn i_arg(args: &[Value], i: usize) -> Result<i64, String> {
    match arg(args, i)? {
        Value::Int(v) => Ok(*v),
        Value::Float(f) => Ok(*f as i64),
        other => Err(format!(
            "argument #{} must be a number, got {}",
            i + 1,
            other.type_name()
        )),
    }
}

fn as_array(v: &Value, what: &str) -> Result<std::rc::Rc<std::cell::RefCell<Vec<Value>>>, String> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        other => Err(format!(
            "{} must be an array, got {}",
            what,
            other.type_name()
        )),
    }
}

fn as_object(
    v: &Value,
    what: &str,
) -> Result<std::rc::Rc<std::cell::RefCell<BTreeMap<String, Value>>>, String> {
    match v {
        Value::Object(o) => Ok(o.clone()),
        other => Err(format!(
            "{} must be an object, got {}",
            what,
            other.type_name()
        )),
    }
}

fn split_cmdline(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    has = true;
                } else if c.is_whitespace() {
                    if has || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        has = false;
                    }
                } else {
                    cur.push(c);
                    has = true;
                }
            }
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// core
// ---------------------------------------------------------------------------

fn b_print(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let mut line = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(&a.display());
    }
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", line);
    let _ = out.flush();
    Ok(Value::Null)
}

fn b_eprint(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let mut line = String::new();
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(&a.display());
    }
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{}", line);
    let _ = err.flush();
    Ok(Value::Null)
}

fn b_len(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(len_of(arg(args, 0)?)? as i64))
}

fn b_type(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(str_val(arg(args, 0)?.type_name()))
}

fn b_str(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(str_val(arg(args, 0)?.display()))
}

fn b_int(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let v = arg(args, 0)?;
    match v {
        Value::Int(i) => Ok(Value::Int(*i)),
        Value::Str(s) => {
            let t = s.trim();
            if let Ok(i) = t.parse::<i64>() {
                return Ok(Value::Int(i));
            }
            match t.parse::<f64>() {
                Ok(f) => Ok(Value::Int(f as i64)),
                Err(_) => Err(format!("cannot convert '{}' to int", s)),
            }
        }
        other => match other.to_num() {
            Some(n) => Ok(Value::Int(n as i64)),
            None => Err(format!("cannot convert {} to int", other.type_name())),
        },
    }
}

fn b_float(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let v = arg(args, 0)?;
    match v.to_num() {
        Some(n) => Ok(Value::Float(n)),
        None => Err(format!("cannot convert {} to float", v.type_name())),
    }
}

fn b_json(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(str_val(arg(args, 0)?.to_json()))
}

fn b_parse(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let s = s_arg(args, 0)?;
    json::parse(&s)
}

fn b_keys(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let o = as_object(arg(args, 0)?, "keys() argument")?;
    let keys: Vec<Value> = o.borrow().keys().map(|k| str_val(k.clone())).collect();
    Ok(array_val(keys))
}

fn b_values(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let o = as_object(arg(args, 0)?, "values() argument")?;
    let vals: Vec<Value> = o.borrow().values().cloned().collect();
    Ok(array_val(vals))
}

fn b_has(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let target = arg(args, 0)?;
    let needle = arg(args, 1)?;
    match target {
        Value::Object(o) => {
            let key = match needle {
                Value::Str(s) => (**s).clone(),
                other => other.display(),
            };
            Ok(Value::Bool(o.borrow().contains_key(&key)))
        }
        Value::Array(a) => Ok(Value::Bool(a.borrow().iter().any(|v| v == needle))),
        Value::Str(s) => Ok(Value::Bool(s.contains(&needle.display()))),
        other => Err(format!("has() does not apply to {}", other.type_name())),
    }
}

fn b_get(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let target = arg(args, 0)?;
    let key = arg(args, 1)?;
    let default = args.get(2).cloned().unwrap_or(Value::Null);
    match target {
        Value::Object(o) => {
            let k = match key {
                Value::Str(s) => (**s).clone(),
                other => other.display(),
            };
            Ok(o.borrow().get(&k).cloned().unwrap_or(default))
        }
        Value::Array(a) => {
            let i = match key {
                Value::Int(i) => *i,
                _ => return Err("array get() needs an integer index".to_string()),
            };
            let a = a.borrow();
            let n = a.len() as i64;
            let j = if i < 0 { n + i } else { i };
            if j < 0 || j >= n {
                Ok(default)
            } else {
                Ok(a[j as usize].clone())
            }
        }
        other => Err(format!("get() does not apply to {}", other.type_name())),
    }
}

fn b_push(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "push() target")?;
    {
        let mut a = a.borrow_mut();
        for v in &args[1..] {
            a.push(v.clone());
        }
    }
    Ok(args[0].clone())
}

fn b_pop(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "pop() target")?;
    let v = a.borrow_mut().pop();
    Ok(v.unwrap_or(Value::Null))
}

fn b_slice(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let target = arg(args, 0)?;
    let start = i_arg(args, 1)?;
    let end = args.get(2).map(|_| i_arg(args, 2)).transpose()?;
    match target {
        Value::Array(a) => {
            let a = a.borrow();
            let n = a.len() as i64;
            let (s, e) = clamp_range(n, start, end);
            Ok(array_val(a[s..e].to_vec()))
        }
        Value::Str(st) => {
            let chars: Vec<char> = st.chars().collect();
            let n = chars.len() as i64;
            let (s, e) = clamp_range(n, start, end);
            Ok(str_val(chars[s..e].iter().collect::<String>()))
        }
        other => Err(format!("slice() does not apply to {}", other.type_name())),
    }
}

fn clamp_range(n: i64, start: i64, end: Option<i64>) -> (usize, usize) {
    let fix = |i: i64| -> i64 {
        if i < 0 {
            (n + i).max(0)
        } else {
            i.min(n)
        }
    };
    let s = fix(start).max(0);
    let e = fix(end.unwrap_or(n)).max(s);
    (s as usize, e as usize)
}

fn b_join(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "join() argument")?;
    let sep = match args.get(1) {
        Some(Value::Str(s)) => (**s).clone(),
        Some(other) => other.display(),
        None => String::new(),
    };
    let parts: Vec<String> = a.borrow().iter().map(|v| v.display()).collect();
    Ok(str_val(parts.join(&sep)))
}

fn b_split(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let s = s_arg(args, 0)?;
    let parts: Vec<Value> = match args.get(1) {
        None | Some(Value::Null) => s
            .split_whitespace()
            .map(|p| str_val(p.to_string()))
            .collect(),
        Some(v) => {
            let sep = v.display();
            if sep.is_empty() {
                s.chars().map(|c| str_val(c.to_string())).collect()
            } else {
                s.split(&sep).map(|p| str_val(p.to_string())).collect()
            }
        }
    };
    Ok(array_val(parts))
}

fn b_trim(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(str_val(s_arg(args, 0)?.trim().to_string()))
}

fn b_upper(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(str_val(s_arg(args, 0)?.to_uppercase()))
}

fn b_lower(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(str_val(s_arg(args, 0)?.to_lowercase()))
}

fn b_replace(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let s = s_arg(args, 0)?;
    let from = s_arg(args, 1)?;
    let to = s_arg(args, 2)?;
    if from.is_empty() {
        return Ok(str_val(s));
    }
    Ok(str_val(s.replace(&from, &to)))
}

fn b_contains(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let target = arg(args, 0)?;
    let needle = arg(args, 1)?;
    match target {
        Value::Str(s) => Ok(Value::Bool(s.contains(&needle.display()))),
        Value::Array(a) => Ok(Value::Bool(a.borrow().iter().any(|v| v == needle))),
        Value::Object(o) => {
            let k = needle.display();
            Ok(Value::Bool(o.borrow().contains_key(&k)))
        }
        other => Err(format!(
            "contains() does not apply to {}",
            other.type_name()
        )),
    }
}

fn b_index_of(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let target = arg(args, 0)?;
    let needle = arg(args, 1)?;
    match target {
        Value::Str(s) => {
            let sub = needle.display();
            match s.find(&sub) {
                Some(byte_pos) => Ok(Value::Int(s[..byte_pos].chars().count() as i64)),
                None => Ok(Value::Int(-1)),
            }
        }
        Value::Array(a) => {
            let pos = a.borrow().iter().position(|v| v == needle);
            Ok(Value::Int(pos.map(|p| p as i64).unwrap_or(-1)))
        }
        other => Err(format!(
            "index_of() does not apply to {}",
            other.type_name()
        )),
    }
}

fn b_starts_with(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(Value::Bool(s_arg(args, 0)?.starts_with(&s_arg(args, 1)?)))
}

fn b_ends_with(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    Ok(Value::Bool(s_arg(args, 0)?.ends_with(&s_arg(args, 1)?)))
}

fn b_repeat(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let s = s_arg(args, 0)?;
    let n = i_arg(args, 1)?.max(0) as usize;
    Ok(str_val(s.repeat(n.min(1_000_000))))
}

fn b_merge(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let mut out = BTreeMap::new();
    for v in args {
        let o = as_object(v, "merge() argument")?;
        for (k, val) in o.borrow().iter() {
            out.insert(k.clone(), val.clone());
        }
    }
    Ok(object_val(out))
}

fn b_map(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "map() argument")?;
    let f = arg(args, 1)?.clone();
    let items = a.borrow().clone();
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        out.push(vm.call_value1(f.clone(), it)?);
    }
    Ok(array_val(out))
}

fn b_filter(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "filter() argument")?;
    let f = arg(args, 1)?.clone();
    let items = a.borrow().clone();
    let mut out = Vec::new();
    for it in items {
        let keep = vm.call_value1(f.clone(), it.clone())?;
        if keep.truthy() {
            out.push(it);
        }
    }
    Ok(array_val(out))
}

fn b_each(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "each() argument")?;
    let f = arg(args, 1)?.clone();
    let items = a.borrow().clone();
    for it in items {
        vm.call_value1(f.clone(), it)?;
    }
    Ok(Value::Null)
}

fn b_reduce(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "reduce() argument")?;
    let f = arg(args, 1)?.clone();
    let items = a.borrow().clone();
    let (mut acc, start) = if args.len() > 2 {
        (args[2].clone(), 0)
    } else {
        match items.first() {
            Some(v) => (v.clone(), 1),
            None => return Err("reduce() of an empty array needs an initial value".to_string()),
        }
    };
    for it in items.iter().skip(start) {
        acc = vm.call_value2(f.clone(), acc, it.clone())?;
    }
    Ok(acc)
}

fn b_sort(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "sort() argument")?;
    let mut items = a.borrow().clone();
    let cmp_fn = match args.get(1) {
        Some(Value::Func(_)) | Some(Value::Builtin(_)) => Some(args[1].clone()),
        Some(Value::Null) | None => None,
        Some(other) => {
            return Err(format!(
                "sort() comparator must be a function, got {}",
                other.type_name()
            ))
        }
    };
    let mut err: Option<String> = None;
    match cmp_fn {
        Some(f) => items.sort_by(|x, y| {
            if err.is_some() {
                return std::cmp::Ordering::Equal;
            }
            match vm.call_value2(f.clone(), x.clone(), y.clone()) {
                Ok(Value::Int(i)) => i.cmp(&0),
                Ok(Value::Float(v)) => v.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal),
                Ok(other) => {
                    err = Some(format!(
                        "comparator must return a number, got {}",
                        other.type_name()
                    ));
                    std::cmp::Ordering::Equal
                }
                Err(e) => {
                    err = Some(e);
                    std::cmp::Ordering::Equal
                }
            }
        }),
        None => items.sort_by(|x, y| compare(x, y).unwrap_or(std::cmp::Ordering::Equal)),
    }
    if let Some(e) = err {
        return Err(e);
    }
    *a.borrow_mut() = items;
    Ok(args[0].clone())
}

fn b_reverse(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "reverse() argument")?;
    a.borrow_mut().reverse();
    Ok(args[0].clone())
}

fn b_range(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let (start, end) = if args.len() >= 2 {
        (i_arg(args, 0)?, i_arg(args, 1)?)
    } else {
        (0, i_arg(args, 0)?)
    };
    let n = (end - start).clamp(0, 10_000_000);
    Ok(array_val((0..n).map(|i| Value::Int(start + i)).collect()))
}

fn b_sum(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "sum() argument")?;
    let mut total = 0.0f64;
    let mut all_int = true;
    for v in a.borrow().iter() {
        match v.to_num() {
            Some(n) => total += n,
            None => return Err(format!("sum() found a non-number: {}", v.type_name())),
        }
        if !matches!(v, Value::Int(_)) {
            all_int = false;
        }
    }
    if all_int && total.abs() < 9.0e15 {
        Ok(Value::Int(total as i64))
    } else {
        Ok(Value::Float(total))
    }
}

fn b_abs(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    match arg(args, 0)? {
        Value::Int(i) => Ok(Value::Int(i.abs())),
        other => Ok(Value::Float(other.to_num().unwrap_or(f64::NAN).abs())),
    }
}

fn float_fn(f: fn(f64) -> f64) -> impl Fn(&mut Vm, &[Value]) -> Result<Value, String> {
    move |_vm: &mut Vm, args: &[Value]| {
        let v = arg(args, 0)?;
        let n = v
            .to_num()
            .ok_or_else(|| format!("cannot convert {} to a number", v.type_name()))?;
        Ok(Value::Float(f(n)))
    }
}

fn b_floor(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    float_fn(f64::floor)(vm, args)
}
fn b_ceil(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    float_fn(f64::ceil)(vm, args)
}
fn b_round(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    float_fn(f64::round)(vm, args)
}
fn b_sqrt(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    float_fn(f64::sqrt)(vm, args)
}

fn b_min(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "min() argument")?;
    let items = a.borrow().clone();
    let mut best: Option<Value> = None;
    for v in items {
        best = Some(match best {
            None => v,
            Some(b) => {
                if compare(&v, &b)
                    .map(|o| o == std::cmp::Ordering::Less)
                    .unwrap_or(false)
                {
                    v
                } else {
                    b
                }
            }
        });
    }
    Ok(best.unwrap_or(Value::Null))
}

fn b_max(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let a = as_array(arg(args, 0)?, "max() argument")?;
    let items = a.borrow().clone();
    let mut best: Option<Value> = None;
    for v in items {
        best = Some(match best {
            None => v,
            Some(b) => {
                if compare(&v, &b)
                    .map(|o| o == std::cmp::Ordering::Greater)
                    .unwrap_or(false)
                {
                    v
                } else {
                    b
                }
            }
        });
    }
    Ok(best.unwrap_or(Value::Null))
}

// ---------------------------------------------------------------------------
// processes: the binary primitive
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ExecOpts {
    timeout_ms: u64,
    cwd: Option<String>,
    env: Vec<(String, String)>,
    stdin: Option<Vec<u8>>,
    shell: bool,
    max_bytes: usize,
    stdout_file: Option<String>,
    stderr_file: Option<String>,
}

fn exec_opts(args: &[Value], shell_default: bool) -> Result<ExecOpts, String> {
    let mut o = ExecOpts {
        timeout_ms: 120_000,
        cwd: None,
        env: Vec::new(),
        stdin: None,
        shell: shell_default,
        max_bytes: 8 * 1024 * 1024,
        stdout_file: None,
        stderr_file: None,
    };
    if let Some(v) = args.get(1) {
        if matches!(v, Value::Null) {
            return Ok(o);
        }
        let obj = as_object(v, "exec() options")?;
        let obj = obj.borrow();
        if let Some(t) = obj.get("timeout") {
            o.timeout_ms = t
                .to_num()
                .ok_or_else(|| "options.timeout must be a number".to_string())?
                .max(1.0) as u64;
        }
        if let Some(c) = obj.get("cwd") {
            o.cwd = Some(c.display());
        }
        if let Some(Value::Object(e)) = obj.get("env") {
            for (k, v) in e.borrow().iter() {
                o.env.push((k.clone(), v.display()));
            }
        }
        if let Some(Value::Str(s)) = obj.get("stdin") {
            o.stdin = Some(s.as_bytes().to_vec());
        }
        if let Some(Value::Bool(b)) = obj.get("shell") {
            o.shell = *b;
        }
        if let Some(Value::Str(path)) = obj.get("stdout") {
            o.stdout_file = Some(path.to_string());
        }
        if let Some(Value::Str(path)) = obj.get("stderr") {
            o.stderr_file = Some(path.to_string());
        }
        if let Some(m) = obj.get("max") {
            if let Some(n) = m.to_num() {
                o.max_bytes = n.max(1024.0) as usize;
            }
        }
    }
    Ok(o)
}

fn drain(mut r: impl Read, cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.len() < cap {
                    let take = (cap - out.len()).min(n);
                    out.extend_from_slice(&buf[..take]);
                }
            }
            Err(_) => break,
        }
    }
    out
}

fn command_for(program: &str, argv: &[String], opts: &ExecOpts) -> Command {
    let mut cmd = if opts.shell {
        let mut line = program.to_string();
        for a in argv {
            line.push(' ');
            if a.contains(' ') {
                line.push('"');
                line.push_str(a);
                line.push('"');
            } else {
                line.push_str(a);
            }
        }
        #[cfg(windows)]
        let c = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(line);
            c
        };
        #[cfg(not(windows))]
        let c = {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(line);
            c
        };
        c
    } else {
        let mut c = Command::new(program);
        c.args(argv);
        c
    };

    if let Some(d) = &opts.cwd {
        cmd.current_dir(d);
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    cmd
}

fn run_process(program: &str, argv: &[String], opts: &ExecOpts) -> Result<Value, String> {
    let mut cmd = command_for(program, argv, opts);

    // A redirected stream goes straight to a file, so large output never has to
    // fit in the 8 MB capture buffer.
    match &opts.stdout_file {
        Some(path) => {
            let file =
                fs::File::create(path).map_err(|e| format!("cannot write '{}': {}", path, e))?;
            cmd.stdout(Stdio::from(file));
        }
        None => {
            cmd.stdout(Stdio::piped());
        }
    }
    match &opts.stderr_file {
        Some(path) => {
            let file =
                fs::File::create(path).map_err(|e| format!("cannot write '{}': {}", path, e))?;
            cmd.stderr(Stdio::from(file));
        }
        None => {
            cmd.stderr(Stdio::piped());
        }
    }
    cmd.stdin(if opts.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot run '{}': {}", program, e))?;

    let cap = opts.max_bytes;
    let h_out = child
        .stdout
        .take()
        .map(|s| std::thread::spawn(move || drain(s, cap)));
    let h_err = child
        .stderr
        .take()
        .map(|s| std::thread::spawn(move || drain(s, cap)));
    if let Some(data) = opts.stdin.clone() {
        if let Some(mut si) = child.stdin.take() {
            std::thread::spawn(move || {
                let _ = si.write_all(&data);
            });
        }
    }

    let mut timed_out = false;
    let mut status_code: Option<i32> = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                status_code = status.code();
                if status_code.is_none() && cfg!(windows) {
                    status_code = Some(1);
                }
                break;
            }
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                return Err(format!("waiting for '{}' failed: {}", program, e));
            }
        }
        if start.elapsed() >= Duration::from_millis(opts.timeout_ms) {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let out = h_out.and_then(|h| h.join().ok()).unwrap_or_default();
    let err = h_err.and_then(|h| h.join().ok()).unwrap_or_default();
    let ms = start.elapsed().as_millis() as i64;
    let code = if timed_out {
        -1
    } else {
        status_code.unwrap_or(-1)
    };

    let mut res = BTreeMap::new();
    res.insert("code".to_string(), Value::Int(code as i64));
    res.insert("ok".to_string(), Value::Bool(code == 0 && !timed_out));
    res.insert(
        "out".to_string(),
        str_val(String::from_utf8_lossy(&out).to_string()),
    );
    res.insert(
        "err".to_string(),
        str_val(String::from_utf8_lossy(&err).to_string()),
    );
    res.insert("ms".to_string(), Value::Int(ms));
    res.insert("timeout".to_string(), Value::Bool(timed_out));
    Ok(object_val(res))
}

fn exec_one(item: &Value, opts: &ExecOpts, label: &str) -> Result<Value, String> {
    match item {
        Value::Str(s) => {
            if opts.shell {
                run_process(s.as_str(), &[], opts)
            } else {
                let mut parts = split_cmdline(s);
                if parts.is_empty() {
                    return Err(format!("{} got an empty command", label));
                }
                let program = parts.remove(0);
                run_process(&program, &parts, opts)
            }
        }
        Value::Array(a) => {
            let items = a.borrow().clone();
            if items.is_empty() {
                return Err(format!("{} got an empty argv array", label));
            }
            let program = items[0].display();
            let rest: Vec<String> = items[1..].iter().map(|v| v.display()).collect();
            run_process(&program, &rest, opts)
        }
        other => Err(format!(
            "{} needs a command string or argv array, got {}",
            label,
            other.type_name()
        )),
    }
}

fn exec_failure(err: String) -> Value {
    let mut res = BTreeMap::new();
    res.insert("code".to_string(), Value::Int(-1));
    res.insert("ok".to_string(), Value::Bool(false));
    res.insert("out".to_string(), str_val(String::new()));
    res.insert("err".to_string(), str_val(err));
    res.insert("ms".to_string(), Value::Int(0));
    res.insert("timeout".to_string(), Value::Bool(false));
    object_val(res)
}

fn b_exec(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let opts = exec_opts(args, false)?;
    exec_one(arg(args, 0)?, &opts, "exec()")
}

/// A plain-data copy of one exec result, so it can cross a worker boundary
/// (`Value` holds `Rc`s and is not `Send`).
struct ExecOutcome {
    code: i64,
    ok: bool,
    out: String,
    err: String,
    ms: i64,
    timeout: bool,
}

fn extract_outcome(value: Value) -> ExecOutcome {
    let mut outcome = ExecOutcome {
        code: -1,
        ok: false,
        out: String::new(),
        err: String::new(),
        ms: 0,
        timeout: false,
    };
    if let Value::Object(map) = &value {
        let map = map.borrow();
        if let Some(Value::Int(code)) = map.get("code") {
            outcome.code = *code;
        }
        if let Some(Value::Bool(ok)) = map.get("ok") {
            outcome.ok = *ok;
        }
        if let Some(Value::Str(out)) = map.get("out") {
            outcome.out = out.to_string();
        }
        if let Some(Value::Str(err)) = map.get("err") {
            outcome.err = err.to_string();
        }
        if let Some(Value::Int(ms)) = map.get("ms") {
            outcome.ms = *ms;
        }
        if let Some(Value::Bool(timeout)) = map.get("timeout") {
            outcome.timeout = *timeout;
        }
    }
    outcome
}

fn outcome_value(outcome: ExecOutcome) -> Value {
    let mut res = BTreeMap::new();
    res.insert("code".to_string(), Value::Int(outcome.code));
    res.insert("ok".to_string(), Value::Bool(outcome.ok));
    res.insert("out".to_string(), str_val(outcome.out));
    res.insert("err".to_string(), str_val(outcome.err));
    res.insert("ms".to_string(), Value::Int(outcome.ms));
    res.insert("timeout".to_string(), Value::Bool(outcome.timeout));
    object_val(res)
}

/// Run several commands at once (batches of eight) and keep the input order in
/// the result array. Each command keeps its own timeout; a spawn failure becomes
/// a failed result entry instead of aborting the whole batch.
fn b_exec_all(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let opts = exec_opts(args, false)?;
    let list = as_array(arg(args, 0)?, "exec_all() argument")?;
    let items: Vec<Value> = list.borrow().clone();
    if items.is_empty() {
        return Ok(array_val(Vec::new()));
    }
    // Resolve each command on the main thread; workers only carry plain strings.
    let mut commands: Vec<Result<(String, Vec<String>), String>> = Vec::with_capacity(items.len());
    for item in &items {
        commands.push(match item {
            Value::Str(s) => {
                if opts.shell {
                    Ok((s.to_string(), Vec::new()))
                } else {
                    let mut parts = split_cmdline(s);
                    if parts.is_empty() {
                        Err("exec_all() got an empty command".to_string())
                    } else {
                        let program = parts.remove(0);
                        Ok((program, parts))
                    }
                }
            }
            Value::Array(a) => {
                let values = a.borrow().clone();
                if values.is_empty() {
                    Err("exec_all() got an empty argv array".to_string())
                } else {
                    let program = values[0].display();
                    let rest: Vec<String> = values[1..].iter().map(|v| v.display()).collect();
                    Ok((program, rest))
                }
            }
            other => Err(format!(
                "exec_all() needs a command string or argv array, got {}",
                other.type_name()
            )),
        });
    }

    let mut results: Vec<Value> = Vec::with_capacity(commands.len());
    const BATCH: usize = 8;
    for start in (0..commands.len()).step_by(BATCH) {
        let end = (start + BATCH).min(commands.len());
        let batch = &commands[start..end];
        let mut part: Vec<Value> = Vec::with_capacity(batch.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|command| {
                    let options = opts.clone();
                    match command {
                        Ok((program, argv)) => {
                            let program = program.clone();
                            let argv = argv.clone();
                            scope.spawn(move || {
                                run_process(&program, &argv, &options).map(extract_outcome)
                            })
                        }
                        Err(message) => {
                            let message = message.clone();
                            scope.spawn(move || Err(message))
                        }
                    }
                })
                .collect();
            for handle in handles {
                part.push(match handle.join() {
                    Ok(Ok(outcome)) => outcome_value(outcome),
                    Ok(Err(message)) => exec_failure(message),
                    Err(_) => exec_failure("exec_all() worker thread panicked".to_string()),
                });
            }
        });
        results.extend(part);
    }
    Ok(array_val(results))
}

fn b_sh(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let opts = exec_opts(args, true)?;
    let line = match arg(args, 0)? {
        Value::Str(s) => (**s).clone(),
        Value::Array(a) => a
            .borrow()
            .iter()
            .map(|v| v.display())
            .collect::<Vec<_>>()
            .join(" "),
        other => {
            return Err(format!(
                "sh() needs a command string, got {}",
                other.type_name()
            ))
        }
    };
    run_process(&line, &[], &opts)
}

fn b_which(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let name = s_arg(args, 0)?;
    if let Some(p) = find_on_path(&name) {
        return Ok(str_val(p.to_string_lossy().to_string()));
    }
    Ok(Value::Null)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 || p.is_absolute() {
        return if p.is_file() {
            Some(p.to_path_buf())
        } else {
            None
        };
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let mut cand = dir.join(name);
            if !ext.is_empty() && !name.to_lowercase().ends_with(ext.as_str()) {
                cand = dir.join(format!("{}{}", name, ext));
            }
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// filesystem / environment
// ---------------------------------------------------------------------------

fn b_read(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    // `read("-")` drains stdin, so a script can be a pipe filter:
    //   git log | bx run summarize.bxs
    if p == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| format!("cannot read stdin: {}", e))?;
        return Ok(str_val(String::from_utf8_lossy(&buf).to_string()));
    }
    let bytes = fs::read(&p).map_err(|e| format!("cannot read '{}': {}", p, e))?;
    Ok(str_val(String::from_utf8_lossy(&bytes).to_string()))
}

fn b_write(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    let data = arg(args, 1)?.display();
    if let Some(parent) = Path::new(&p).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    fs::write(&p, data).map_err(|e| format!("cannot write '{}': {}", p, e))?;
    Ok(Value::Null)
}

fn b_append(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    use std::fs::OpenOptions;
    let p = s_arg(args, 0)?;
    let data = arg(args, 1)?.display();
    if let Some(parent) = Path::new(&p).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .map_err(|e| format!("cannot append to '{}': {}", p, e))?;
    f.write_all(data.as_bytes())
        .map_err(|e| format!("cannot append to '{}': {}", p, e))?;
    Ok(Value::Null)
}

fn b_exists(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    Ok(Value::Bool(Path::new(&p).exists()))
}

fn b_list(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    let mut names: Vec<String> = Vec::new();
    let rd = fs::read_dir(&p).map_err(|e| format!("cannot list '{}': {}", p, e))?;
    for entry in rd.flatten() {
        names.push(entry.file_name().to_string_lossy().to_string());
    }
    names.sort();
    Ok(array_val(names.into_iter().map(str_val).collect()))
}

fn b_mkdir(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    fs::create_dir_all(&p).map_err(|e| format!("cannot create '{}': {}", p, e))?;
    Ok(Value::Null)
}

fn b_remove(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    let path = Path::new(&p);
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| format!("cannot remove '{}': {}", p, e))?;
    } else if path.exists() {
        fs::remove_file(path).map_err(|e| format!("cannot remove '{}': {}", p, e))?;
    }
    Ok(Value::Null)
}

fn b_cwd(_vm: &mut Vm, _args: &[Value]) -> Result<Value, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cannot read cwd: {}", e))?;
    Ok(str_val(cwd.to_string_lossy().to_string()))
}

fn b_env(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() {
        let mut m = BTreeMap::new();
        for (k, v) in std::env::vars() {
            m.insert(k, str_val(v));
        }
        return Ok(object_val(m));
    }
    let key = s_arg(args, 0)?;
    match std::env::var(&key) {
        Ok(v) => Ok(str_val(v)),
        Err(_) => Ok(args.get(1).cloned().unwrap_or(Value::Null)),
    }
}

fn b_now(_vm: &mut Vm, _args: &[Value]) -> Result<Value, String> {
    let ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::Int(ms))
}

fn b_sleep(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let ms = i_arg(args, 0)?.max(0) as u64;
    std::thread::sleep(Duration::from_millis(ms));
    Ok(Value::Null)
}

fn b_exit(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let code = args.first().and_then(|v| v.to_num()).unwrap_or(0.0) as i32;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}

fn b_error(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    match args.first() {
        Some(v) => Err(v.display()),
        None => Err("error()".to_string()),
    }
}

/// Run a zero-argument function and turn a runtime failure into a value. The
/// VM already unwinds a failed frame back to its caller, so this is the whole
/// exception mechanism a script needs: no syntax, no handler stack.
fn b_attempt(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let f = arg(args, 0)?.clone();
    let mut out = BTreeMap::new();
    match vm.call_value(f, Vec::new(), 0) {
        Ok(v) => {
            out.insert("ok".to_string(), Value::Bool(true));
            out.insert("value".to_string(), v);
            out.insert("error".to_string(), Value::Null);
        }
        Err(e) => {
            out.insert("ok".to_string(), Value::Bool(false));
            out.insert("value".to_string(), Value::Null);
            out.insert("error".to_string(), str_val(e));
        }
    }
    Ok(object_val(out))
}

fn b_assert(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let cond = arg(args, 0)?;
    if cond.truthy() {
        return Ok(Value::Null);
    }
    let msg = match args.get(1) {
        Some(v) => v.display(),
        None => "assertion failed".to_string(),
    };
    Err(msg)
}

fn b_basename(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    let name = Path::new(&p)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(str_val(name))
}

fn b_dirname(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let p = s_arg(args, 0)?;
    let d = Path::new(&p)
        .parent()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(str_val(d))
}

pub static BUILTINS: &[Builtin] = &[
    Builtin {
        name: "print",
        sig: "print(...values)",
        doc: "write a line to stdout",
        f: b_print,
    },
    Builtin {
        name: "eprint",
        sig: "eprint(...values)",
        doc: "write a line to stderr",
        f: b_eprint,
    },
    Builtin {
        name: "len",
        sig: "len(x)",
        doc: "length of string, array or object",
        f: b_len,
    },
    Builtin {
        name: "type",
        sig: "type(x)",
        doc: "value type name",
        f: b_type,
    },
    Builtin {
        name: "str",
        sig: "str(x)",
        doc: "convert to string",
        f: b_str,
    },
    Builtin {
        name: "int",
        sig: "int(x)",
        doc: "convert to integer",
        f: b_int,
    },
    Builtin {
        name: "float",
        sig: "float(x)",
        doc: "convert to float",
        f: b_float,
    },
    Builtin {
        name: "json",
        sig: "json(x)",
        doc: "serialize a value to compact JSON",
        f: b_json,
    },
    Builtin {
        name: "parse",
        sig: "parse(s)",
        doc: "parse JSON text into a value",
        f: b_parse,
    },
    Builtin {
        name: "keys",
        sig: "keys(o)",
        doc: "object keys as an array",
        f: b_keys,
    },
    Builtin {
        name: "values",
        sig: "values(o)",
        doc: "object values as an array",
        f: b_values,
    },
    Builtin {
        name: "has",
        sig: "has(x, key)",
        doc: "membership test for object/array/string",
        f: b_has,
    },
    Builtin {
        name: "get",
        sig: "get(x, key, default?)",
        doc: "safe index lookup",
        f: b_get,
    },
    Builtin {
        name: "push",
        sig: "push(a, ...values)",
        doc: "append to an array in place",
        f: b_push,
    },
    Builtin {
        name: "pop",
        sig: "pop(a)",
        doc: "remove and return the last array element",
        f: b_pop,
    },
    Builtin {
        name: "slice",
        sig: "slice(x, start, end?)",
        doc: "sub array or substring (negative ok)",
        f: b_slice,
    },
    Builtin {
        name: "join",
        sig: "join(a, sep?)",
        doc: "join array elements into a string",
        f: b_join,
    },
    Builtin {
        name: "split",
        sig: "split(s, sep?)",
        doc: "split a string; no sep means whitespace",
        f: b_split,
    },
    Builtin {
        name: "trim",
        sig: "trim(s)",
        doc: "strip leading and trailing whitespace",
        f: b_trim,
    },
    Builtin {
        name: "upper",
        sig: "upper(s)",
        doc: "uppercase",
        f: b_upper,
    },
    Builtin {
        name: "lower",
        sig: "lower(s)",
        doc: "lowercase",
        f: b_lower,
    },
    Builtin {
        name: "replace",
        sig: "replace(s, from, to)",
        doc: "replace all occurrences",
        f: b_replace,
    },
    Builtin {
        name: "contains",
        sig: "contains(x, needle)",
        doc: "substring / element / key test",
        f: b_contains,
    },
    Builtin {
        name: "index_of",
        sig: "index_of(x, needle)",
        doc: "position of a substring or element, -1 if absent",
        f: b_index_of,
    },
    Builtin {
        name: "starts_with",
        sig: "starts_with(s, prefix)",
        doc: "prefix test",
        f: b_starts_with,
    },
    Builtin {
        name: "ends_with",
        sig: "ends_with(s, suffix)",
        doc: "suffix test",
        f: b_ends_with,
    },
    Builtin {
        name: "repeat",
        sig: "repeat(s, n)",
        doc: "repeat a string",
        f: b_repeat,
    },
    Builtin {
        name: "merge",
        sig: "merge(...objects)",
        doc: "merge objects into a new object",
        f: b_merge,
    },
    Builtin {
        name: "map",
        sig: "map(a, f)",
        doc: "transform every element",
        f: b_map,
    },
    Builtin {
        name: "filter",
        sig: "filter(a, f)",
        doc: "keep elements where f is truthy",
        f: b_filter,
    },
    Builtin {
        name: "each",
        sig: "each(a, f)",
        doc: "call f for every element",
        f: b_each,
    },
    Builtin {
        name: "reduce",
        sig: "reduce(a, f, init?)",
        doc: "fold an array into one value",
        f: b_reduce,
    },
    Builtin {
        name: "sort",
        sig: "sort(a, cmp?)",
        doc: "sort an array in place",
        f: b_sort,
    },
    Builtin {
        name: "reverse",
        sig: "reverse(a)",
        doc: "reverse an array in place",
        f: b_reverse,
    },
    Builtin {
        name: "range",
        sig: "range(n) | range(start, end)",
        doc: "array of integers",
        f: b_range,
    },
    Builtin {
        name: "sum",
        sig: "sum(a)",
        doc: "sum an array of numbers",
        f: b_sum,
    },
    Builtin {
        name: "abs",
        sig: "abs(n)",
        doc: "absolute value",
        f: b_abs,
    },
    Builtin {
        name: "floor",
        sig: "floor(n)",
        doc: "round down",
        f: b_floor,
    },
    Builtin {
        name: "ceil",
        sig: "ceil(n)",
        doc: "round up",
        f: b_ceil,
    },
    Builtin {
        name: "round",
        sig: "round(n)",
        doc: "round to nearest",
        f: b_round,
    },
    Builtin {
        name: "sqrt",
        sig: "sqrt(n)",
        doc: "square root",
        f: b_sqrt,
    },
    Builtin {
        name: "min",
        sig: "min(a)",
        doc: "smallest element",
        f: b_min,
    },
    Builtin {
        name: "max",
        sig: "max(a)",
        doc: "largest element",
        f: b_max,
    },
    Builtin {
        name: "exec",
        sig: "exec(cmd, opts?)",
        doc: "run a binary; returns {code,ok,out,err,ms,timeout}",
        f: b_exec,
    },
    Builtin {
        name: "sh",
        sig: "sh(cmd, opts?)",
        doc: "run a command through the system shell",
        f: b_sh,
    },
    Builtin {
        name: "exec_all",
        sig: "exec_all([...], options?)",
        doc: "run commands concurrently (up to eight at a time); results keep the input order",
        f: b_exec_all,
    },
    Builtin {
        name: "spawn",
        sig: "spawn(command, options?)",
        doc: "start a background process; returns {id, pid} for spawn_poll/spawn_wait",
        f: b_spawn,
    },
    Builtin {
        name: "spawn_poll",
        sig: "spawn_poll(id)",
        doc: "non-blocking check: {running, code, out, err}; drains new output",
        f: b_spawn_poll,
    },
    Builtin {
        name: "spawn_wait",
        sig: "spawn_wait(id, timeout_ms?)",
        doc: "wait for a spawned process, returning {running, timeout, code, out, err}",
        f: b_spawn_wait,
    },
    Builtin {
        name: "spawn_write",
        sig: "spawn_write(id, text)",
        doc: "write text to a spawned process stdin; false when it has no pipe",
        f: b_spawn_write,
    },
    Builtin {
        name: "spawn_kill",
        sig: "spawn_kill(id)",
        doc: "kill a spawned process and collect the rest of its output",
        f: b_spawn_kill,
    },
    Builtin {
        name: "spawn_close",
        sig: "spawn_close(id)",
        doc: "close a spawned process stdin (EOF); false when it had no pipe",
        f: b_spawn_close,
    },
    Builtin {
        name: "which",
        sig: "which(name)",
        doc: "resolve a program on PATH",
        f: b_which,
    },
    Builtin {
        name: "read",
        sig: "read(path)",
        doc: "read a file as text; read(\"-\") drains stdin to EOF",
        f: b_read,
    },
    Builtin {
        name: "write",
        sig: "write(path, data)",
        doc: "write text to a file, creating parent dirs",
        f: b_write,
    },
    Builtin {
        name: "append",
        sig: "append(path, data)",
        doc: "append text to a file",
        f: b_append,
    },
    Builtin {
        name: "patch",
        sig: "patch(path, diff)",
        doc: "apply a unified diff to a text file; nothing is written when a hunk does not match",
        f: b_patch,
    },
    Builtin {
        name: "exists",
        sig: "exists(path)",
        doc: "path existence test",
        f: b_exists,
    },
    Builtin {
        name: "list",
        sig: "list(dir)",
        doc: "sorted entry names of a directory",
        f: b_list,
    },
    Builtin {
        name: "mkdir",
        sig: "mkdir(path)",
        doc: "create a directory and parents",
        f: b_mkdir,
    },
    Builtin {
        name: "remove",
        sig: "remove(path)",
        doc: "delete a file or directory tree",
        f: b_remove,
    },
    Builtin {
        name: "cwd",
        sig: "cwd()",
        doc: "current working directory",
        f: b_cwd,
    },
    Builtin {
        name: "env",
        sig: "env(name, default?) | env()",
        doc: "read an environment variable or all of them",
        f: b_env,
    },
    Builtin {
        name: "now",
        sig: "now()",
        doc: "unix epoch milliseconds",
        f: b_now,
    },
    Builtin {
        name: "sleep",
        sig: "sleep(ms)",
        doc: "block for a number of milliseconds",
        f: b_sleep,
    },
    Builtin {
        name: "exit",
        sig: "exit(code?)",
        doc: "exit the process immediately",
        f: b_exit,
    },
    Builtin {
        name: "error",
        sig: "error(message)",
        doc: "raise a runtime error",
        f: b_error,
    },
    Builtin {
        name: "attempt",
        sig: "attempt(f)",
        doc: "run f(); return {ok, value, error} instead of failing the script",
        f: b_attempt,
    },
    Builtin {
        name: "test",
        sig: "test(name, fn)",
        doc: "run one named test; `bx test <file>` reports the summary and exit code",
        f: b_test,
    },
    Builtin {
        name: "assert",
        sig: "assert(cond, message?)",
        doc: "raise unless cond is truthy",
        f: b_assert,
    },
    Builtin {
        name: "basename",
        sig: "basename(path)",
        doc: "final path component",
        f: b_basename,
    },
    Builtin {
        name: "dirname",
        sig: "dirname(path)",
        doc: "parent directory of a path",
        f: b_dirname,
    },
];
// ---------------------------------------------------------------------------
// unified diff application: the structured edit primitive for self-iteration
// ---------------------------------------------------------------------------

struct PatchHunk {
    old_start: usize,
    ops: Vec<(char, String)>,
}

fn parse_patch(diff: &str) -> Result<Vec<PatchHunk>, String> {
    let mut hunks: Vec<PatchHunk> = Vec::new();
    let mut current: Option<PatchHunk> = None;
    let raw_lines: Vec<&str> = diff.split('\n').collect();
    for (raw_index, raw) in raw_lines.iter().enumerate() {
        let line = raw.trim_end_matches('\r');
        // The final newline of the diff file leaves an empty tail element.
        if line.is_empty() && raw_index + 1 == raw_lines.len() {
            continue;
        }
        if line.starts_with("@@") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            let old_start =
                parse_hunk_old_start(line).ok_or_else(|| format!("bad hunk header '{}'", line))?;
            current = Some(PatchHunk {
                old_start,
                ops: Vec::new(),
            });
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("diff ") {
            continue;
        }
        if line.starts_with("\\ No newline") {
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            continue;
        };
        let (op, text) = if let Some(rest) = line.strip_prefix('+') {
            ('+', rest.to_string())
        } else if let Some(rest) = line.strip_prefix('-') {
            ('-', rest.to_string())
        } else if let Some(rest) = line.strip_prefix(' ') {
            (' ', rest.to_string())
        } else if line.is_empty() {
            (' ', String::new())
        } else {
            return Err(format!("unsupported diff line '{}'", line));
        };
        hunk.ops.push((op, text));
    }
    if let Some(hunk) = current.take() {
        hunks.push(hunk);
    }
    Ok(hunks)
}

fn parse_hunk_old_start(header: &str) -> Option<usize> {
    let rest = header.strip_prefix("@@ -")?;
    rest.split([',', ' ']).next()?.parse::<usize>().ok()
}

fn apply_patch(original: &str, hunks: &[PatchHunk]) -> Result<String, String> {
    let eol = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let had_trailing = original.ends_with('\n');
    let mut lines: Vec<String> = original
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    if had_trailing {
        lines.pop();
    }

    let mut out: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    for (index, hunk) in hunks.iter().enumerate() {
        let start = hunk.old_start.saturating_sub(1);
        if start < cursor {
            return Err(format!("hunk {} overlaps the previous hunk", index + 1));
        }
        if start > lines.len() {
            return Err(format!(
                "hunk {} starts past the end of the file (line {})",
                index + 1,
                start + 1
            ));
        }
        out.extend_from_slice(&lines[cursor..start]);
        cursor = start;
        for (op, text) in &hunk.ops {
            match op {
                '+' => out.push(text.clone()),
                ' ' | '-' => {
                    let actual = lines.get(cursor);
                    if actual != Some(text) {
                        return Err(format!(
                            "hunk {}: context mismatch at line {} (expected {:?})",
                            index + 1,
                            cursor + 1,
                            text
                        ));
                    }
                    if *op == ' ' {
                        out.push(text.clone());
                    }
                    cursor += 1;
                }
                _ => {}
            }
        }
    }
    out.extend_from_slice(&lines[cursor..]);
    let mut result = out.join(eol);
    if had_trailing {
        result.push_str(eol);
    }
    Ok(result)
}

fn b_patch(_vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let path = s_arg(args, 0)?;
    let diff = match arg(args, 1)? {
        Value::Str(text) => text.to_string(),
        other => other.display(),
    };
    let original =
        fs::read_to_string(&path).map_err(|e| format!("cannot read '{}': {}", path, e))?;
    let hunks = parse_patch(&diff)?;
    if hunks.is_empty() {
        return Err("diff contains no hunks".to_string());
    }
    let updated = apply_patch(&original, &hunks)?;
    // Publish only a fully applied patch; a rejected hunk leaves the file alone.
    let tmp = format!("{}.bxpatch-{}", path, std::process::id());
    fs::write(&tmp, updated.as_bytes()).map_err(|e| format!("cannot write '{}': {}", tmp, e))?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("cannot replace '{}': {}", path, e));
    }
    let mut out = BTreeMap::new();
    out.insert("ok".to_string(), Value::Bool(true));
    out.insert("path".to_string(), str_val(path));
    out.insert("hunks".to_string(), Value::Int(hunks.len() as i64));
    out.insert("bytes".to_string(), Value::Int(updated.len() as i64));
    Ok(object_val(out))
}
// ---------------------------------------------------------------------------
// spawn: a background process whose streams are read incrementally
// ---------------------------------------------------------------------------

fn command_parts(item: &Value, label: &str) -> Result<(String, Vec<String>), String> {
    match item {
        Value::Str(command) => {
            let mut parts = split_cmdline(command);
            if parts.is_empty() {
                return Err(format!("{} got an empty command", label));
            }
            let program = parts.remove(0);
            Ok((program, parts))
        }
        Value::Array(list) => {
            let items = list.borrow().clone();
            if items.is_empty() {
                return Err(format!("{} got an empty argv array", label));
            }
            Ok((
                items[0].display(),
                items[1..].iter().map(|v| v.display()).collect(),
            ))
        }
        other => Err(format!(
            "{} needs a command string or argv array, got {}",
            label,
            other.type_name()
        )),
    }
}

fn spawn_reader(
    mut stream: impl Read + Send + 'static,
    buffer: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut guard) = buffer.lock() {
                        guard.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
    })
}

fn take_buffer(buffer: &Arc<Mutex<Vec<u8>>>) -> String {
    match buffer.lock() {
        Ok(mut guard) => String::from_utf8_lossy(&std::mem::take(&mut *guard)).to_string(),
        Err(_) => String::new(),
    }
}

fn spawn_result(
    code: Option<i32>,
    out: String,
    err: String,
    running: bool,
    timeout: bool,
) -> Value {
    let mut res = BTreeMap::new();
    res.insert("code".to_string(), Value::Int(code.unwrap_or(-1) as i64));
    res.insert(
        "ok".to_string(),
        Value::Bool(code == Some(0) && !timeout && !running),
    );
    res.insert("out".to_string(), str_val(out));
    res.insert("err".to_string(), str_val(err));
    res.insert("running".to_string(), Value::Bool(running));
    res.insert("timeout".to_string(), Value::Bool(timeout));
    object_val(res)
}

fn spawn_id(args: &[Value], label: &str) -> Result<u64, String> {
    arg(args, 0)?
        .to_num()
        .map(|n| n as u64)
        .ok_or_else(|| format!("{} needs a handle id", label))
}

fn b_spawn(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let opts = exec_opts(args, false)?;
    let (program, argv) = if opts.shell {
        match arg(args, 0)? {
            Value::Str(command) => (command.to_string(), Vec::new()),
            other => {
                return Err(format!(
                    "spawn() with shell needs a command string, got {}",
                    other.type_name()
                ))
            }
        }
    } else {
        command_parts(arg(args, 0)?, "spawn()")?
    };
    let mut cmd = command_for(&program, &argv, &opts);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot spawn '{}': {}", program, e))?;
    let pid = child.id() as i64;
    let out = Arc::new(Mutex::new(Vec::new()));
    let err = Arc::new(Mutex::new(Vec::new()));
    let mut readers = Vec::new();
    if let Some(stream) = child.stdout.take() {
        readers.push(spawn_reader(stream, out.clone()));
    }
    if let Some(stream) = child.stderr.take() {
        readers.push(spawn_reader(stream, err.clone()));
    }
    let mut stdin = child.stdin.take();
    if let (Some(pipe), Some(data)) = (stdin.as_mut(), opts.stdin.clone()) {
        let _ = pipe.write_all(&data);
    }
    let id = vm.spawn_register(Spawned {
        child,
        out,
        err,
        readers,
        stdin,
    });
    let mut res = BTreeMap::new();
    res.insert("id".to_string(), Value::Int(id as i64));
    res.insert("pid".to_string(), Value::Int(pid));
    Ok(object_val(res))
}

fn b_spawn_poll(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let id = spawn_id(args, "spawn_poll()")?;
    let exited = {
        let spawned = vm
            .spawn_child(id)
            .ok_or_else(|| format!("unknown spawn handle {}", id))?;
        match spawned.child.try_wait() {
            Ok(Some(status)) => Some(status),
            Ok(None) => None,
            Err(e) => return Err(format!("spawn_poll() failed: {}", e)),
        }
    };
    match exited {
        Some(status) => {
            let mut spawned = vm
                .children
                .remove(&id)
                .ok_or_else(|| format!("unknown spawn handle {}", id))?;
            let out = take_buffer(&spawned.out);
            let err = take_buffer(&spawned.err);
            for reader in spawned.readers.drain(..) {
                let _ = reader.join();
            }
            let out = format!("{}{}", out, take_buffer(&spawned.out));
            let err = format!("{}{}", err, take_buffer(&spawned.err));
            Ok(spawn_result(status.code(), out, err, false, false))
        }
        None => {
            let spawned = vm
                .spawn_child(id)
                .ok_or_else(|| format!("unknown spawn handle {}", id))?;
            let out = take_buffer(&spawned.out);
            let err = take_buffer(&spawned.err);
            Ok(spawn_result(None, out, err, true, false))
        }
    }
}

fn b_spawn_wait(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let id = spawn_id(args, "spawn_wait()")?;
    let timeout_ms = if args.len() > 1 {
        args[1].to_num().unwrap_or(30_000.0).max(1.0) as u64
    } else {
        30_000
    };
    let start = Instant::now();
    let status = loop {
        let exited = {
            let spawned = vm
                .spawn_child(id)
                .ok_or_else(|| format!("unknown spawn handle {}", id))?;
            match spawned.child.try_wait() {
                Ok(Some(status)) => Some(status),
                Ok(None) => None,
                Err(e) => return Err(format!("spawn_wait() failed: {}", e)),
            }
        };
        if let Some(status) = exited {
            break status;
        }
        if start.elapsed() >= Duration::from_millis(timeout_ms) {
            let spawned = vm
                .spawn_child(id)
                .ok_or_else(|| format!("unknown spawn handle {}", id))?;
            let out = take_buffer(&spawned.out);
            let err = take_buffer(&spawned.err);
            return Ok(spawn_result(None, out, err, true, true));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut spawned = vm
        .children
        .remove(&id)
        .ok_or_else(|| format!("unknown spawn handle {}", id))?;
    for reader in spawned.readers.drain(..) {
        let _ = reader.join();
    }
    let out = take_buffer(&spawned.out);
    let err = take_buffer(&spawned.err);
    Ok(spawn_result(status.code(), out, err, false, false))
}

fn b_spawn_write(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let id = spawn_id(args, "spawn_write()")?;
    let data = arg(args, 1)?.display();
    let spawned = vm
        .spawn_child(id)
        .ok_or_else(|| format!("unknown spawn handle {}", id))?;
    match spawned.stdin.as_mut() {
        Some(pipe) => {
            pipe.write_all(data.as_bytes())
                .map_err(|e| format!("spawn_write() failed: {}", e))?;
            let _ = pipe.flush();
            Ok(Value::Bool(true))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn b_spawn_kill(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let id = spawn_id(args, "spawn_kill()")?;
    let mut spawned = vm
        .children
        .remove(&id)
        .ok_or_else(|| format!("unknown spawn handle {}", id))?;
    let _ = spawned.child.kill();
    let status = spawned.child.wait().ok();
    for reader in spawned.readers.drain(..) {
        let _ = reader.join();
    }
    let out = take_buffer(&spawned.out);
    let err = take_buffer(&spawned.err);
    Ok(spawn_result(
        status.and_then(|s| s.code()),
        out,
        err,
        false,
        false,
    ))
}
fn b_spawn_close(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let id = spawn_id(args, "spawn_close()")?;
    let spawned = vm
        .spawn_child(id)
        .ok_or_else(|| format!("unknown spawn handle {}", id))?;
    // Dropping the pipe sends EOF, which a child reading stdin waits for.
    let had_pipe = spawned.stdin.take().is_some();
    Ok(Value::Bool(had_pipe))
}
fn b_test(vm: &mut Vm, args: &[Value]) -> Result<Value, String> {
    let name = s_arg(args, 0)?;
    let body = arg(args, 1)?.clone();
    match vm.call_value(body, Vec::new(), 0) {
        Ok(_) => {
            vm.tests_passed += 1;
        }
        Err(e) => {
            vm.tests_failed += 1;
            vm.test_failures.push(format!("{}: {}", name, e));
        }
    }
    Ok(Value::Null)
}
