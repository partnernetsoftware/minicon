use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::ast::{BinOp, Expr, Stmt};
use crate::value::{str_val, FuncRef, Value};

pub mod op {
    pub const HALT: u8 = 0;
    pub const CONST: u8 = 1;
    pub const NULL: u8 = 2;
    pub const TRUE: u8 = 3;
    pub const FALSE: u8 = 4;
    pub const POP: u8 = 5;
    pub const DUP: u8 = 6;
    pub const GET_LOCAL: u8 = 7;
    pub const SET_LOCAL: u8 = 8;
    pub const GET_GLOBAL: u8 = 9;
    pub const SET_GLOBAL: u8 = 10;
    pub const ADD: u8 = 11;
    pub const SUB: u8 = 12;
    pub const MUL: u8 = 13;
    pub const DIV: u8 = 14;
    pub const MOD: u8 = 15;
    pub const NEG: u8 = 16;
    pub const NOT: u8 = 17;
    pub const EQ: u8 = 18;
    pub const NE: u8 = 19;
    pub const LT: u8 = 20;
    pub const LE: u8 = 21;
    pub const GT: u8 = 22;
    pub const GE: u8 = 23;
    pub const JUMP: u8 = 24;
    pub const JUMP_IF_FALSE: u8 = 25;
    pub const JUMP_IF_TRUE: u8 = 26;
    pub const JUMP_IF_FALSE_OR_POP: u8 = 27;
    pub const JUMP_IF_TRUE_OR_POP: u8 = 28;
    pub const CALL: u8 = 29;
    pub const RET: u8 = 30;
    pub const MAKE_ARRAY: u8 = 31;
    pub const MAKE_OBJECT: u8 = 32;
    pub const INDEX_GET: u8 = 33;
    pub const INDEX_SET: u8 = 34;
    pub const LEN: u8 = 35;
    pub const DUP2: u8 = 36;
    pub const INDEX_SET_KEEP: u8 = 37;
    /// Normalise any iterable into the array a `for … in` walks: an array stays
    /// itself, a string becomes its characters, an object becomes its keys.
    pub const ITER_PREP: u8 = 38;
}

pub fn has_operand(o: u8) -> bool {
    matches!(
        o,
        op::CONST
            | op::GET_LOCAL
            | op::SET_LOCAL
            | op::GET_GLOBAL
            | op::SET_GLOBAL
            | op::JUMP
            | op::JUMP_IF_FALSE
            | op::JUMP_IF_TRUE
            | op::JUMP_IF_FALSE_OR_POP
            | op::JUMP_IF_TRUE_OR_POP
            | op::CALL
            | op::MAKE_ARRAY
            | op::MAKE_OBJECT
    )
}

pub fn op_name(o: u8) -> &'static str {
    match o {
        op::HALT => "HALT",
        op::CONST => "CONST",
        op::NULL => "NULL",
        op::TRUE => "TRUE",
        op::FALSE => "FALSE",
        op::POP => "POP",
        op::DUP => "DUP",
        op::GET_LOCAL => "GET_LOCAL",
        op::SET_LOCAL => "SET_LOCAL",
        op::GET_GLOBAL => "GET_GLOBAL",
        op::SET_GLOBAL => "SET_GLOBAL",
        op::ADD => "ADD",
        op::SUB => "SUB",
        op::MUL => "MUL",
        op::DIV => "DIV",
        op::MOD => "MOD",
        op::NEG => "NEG",
        op::NOT => "NOT",
        op::EQ => "EQ",
        op::NE => "NE",
        op::LT => "LT",
        op::LE => "LE",
        op::GT => "GT",
        op::GE => "GE",
        op::JUMP => "JUMP",
        op::JUMP_IF_FALSE => "JUMP_IF_FALSE",
        op::JUMP_IF_TRUE => "JUMP_IF_TRUE",
        op::JUMP_IF_FALSE_OR_POP => "JUMP_IF_FALSE_OR_POP",
        op::JUMP_IF_TRUE_OR_POP => "JUMP_IF_TRUE_OR_POP",
        op::CALL => "CALL",
        op::RET => "RET",
        op::MAKE_ARRAY => "MAKE_ARRAY",
        op::MAKE_OBJECT => "MAKE_OBJECT",
        op::INDEX_GET => "INDEX_GET",
        op::INDEX_SET => "INDEX_SET",
        op::LEN => "LEN",
        op::DUP2 => "DUP2",
        op::INDEX_SET_KEEP => "INDEX_SET_KEEP",
        op::ITER_PREP => "ITER_PREP",
        _ => "?",
    }
}

#[derive(Clone, Default)]
pub struct Chunk {
    pub name: String,
    pub arity: u16,
    pub max_slots: u16,
    pub code: Vec<u8>,
    pub lines: Vec<u32>,
    pub consts: Vec<Value>,
}

impl Chunk {
    pub fn new(name: String, arity: u16) -> Chunk {
        Chunk {
            name,
            arity,
            max_slots: 0,
            code: Vec::new(),
            lines: Vec::new(),
            consts: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// binary container
// ---------------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"BXB\x01";

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

pub fn encode(chunks: &[Chunk], want_result: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(if want_result { 1 } else { 0 });
    put_u32(&mut out, chunks.len() as u32);
    for c in chunks {
        put_str(&mut out, &c.name);
        put_u16(&mut out, c.arity);
        put_u16(&mut out, c.max_slots);
        put_u32(&mut out, c.code.len() as u32);
        out.extend_from_slice(&c.code);
        // line table, run-length encoded so bytecode stays close to source size
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for l in &c.lines {
            match runs.last_mut() {
                Some((line, n)) if *line == *l => *n += 1,
                _ => runs.push((*l, 1)),
            }
        }
        put_u32(&mut out, runs.len() as u32);
        for (line, n) in runs {
            put_u32(&mut out, line);
            put_u32(&mut out, n);
        }
        put_u32(&mut out, c.consts.len() as u32);
        for v in &c.consts {
            encode_const(&mut out, v);
        }
    }
    out
}

fn encode_const(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => out.push(0),
        Value::Bool(b) => {
            out.push(1);
            out.push(if *b { 1 } else { 0 });
        }
        Value::Int(i) => {
            out.push(2);
            put_i64(out, *i);
        }
        Value::Float(f) => {
            out.push(3);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Str(s) => {
            out.push(4);
            put_str(out, s);
        }
        Value::Func(f) => {
            out.push(5);
            put_u32(out, f.chunk as u32);
            put_str(out, &f.name);
        }
        _ => out.push(0),
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn need(&self, n: usize) -> Result<(), String> {
        if self.pos + n > self.buf.len() {
            Err("truncated bytecode".to_string())
        } else {
            Ok(())
        }
    }
    fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, String> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        self.need(4)?;
        let v = u32::from_le_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }
    fn i64(&mut self) -> Result<i64, String> {
        self.need(8)?;
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.buf[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(i64::from_le_bytes(b))
    }
    fn f64(&mut self) -> Result<f64, String> {
        self.need(8)?;
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.buf[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(f64::from_le_bytes(b))
    }
    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, String> {
        self.need(n)?;
        let v = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        let b = self.bytes(n)?;
        String::from_utf8(b).map_err(|_| "invalid utf-8 in bytecode".to_string())
    }
}

pub fn decode(data: &[u8]) -> Result<(Vec<Chunk>, bool), String> {
    if data.len() < 9 || &data[0..4] != MAGIC {
        return Err("not a bx bytecode file (bad magic)".to_string());
    }
    let mut r = Reader { buf: data, pos: 4 };
    let want_result = r.u8()? != 0;
    let nchunks = r.u32()? as usize;
    if nchunks == 0 || nchunks > 100_000 {
        return Err("bytecode declares an implausible chunk count".to_string());
    }
    let mut chunks = Vec::with_capacity(nchunks);
    for _ in 0..nchunks {
        let name = r.string()?;
        let arity = r.u16()?;
        let max_slots = r.u16()?;
        let clen = r.u32()? as usize;
        let code = r.bytes(clen)?;
        let nruns = r.u32()? as usize;
        if nruns > clen + 1 {
            return Err("bytecode line table is malformed".to_string());
        }
        let mut lines: Vec<u32> = Vec::with_capacity(clen);
        for _ in 0..nruns {
            let line = r.u32()?;
            let n = r.u32()? as usize;
            if lines.len() + n > clen {
                return Err("bytecode line table overruns code size".to_string());
            }
            for _ in 0..n {
                lines.push(line);
            }
        }
        if lines.len() != clen {
            return Err("bytecode line table does not match code size".to_string());
        }
        let nconsts = r.u32()? as usize;
        if nconsts > 1_000_000 {
            return Err("bytecode declares too many constants".to_string());
        }
        let mut consts = Vec::with_capacity(nconsts);
        for _ in 0..nconsts {
            consts.push(decode_const(&mut r)?);
        }
        // validate every instruction is known and well formed
        let mut ip = 0usize;
        while ip < code.len() {
            let o = code[ip];
            if op_name(o) == "?" {
                return Err(format!("bytecode contains unknown opcode {}", o));
            }
            let step = if has_operand(o) { 5 } else { 1 };
            if ip + step > code.len() {
                return Err("bytecode instruction truncated".to_string());
            }
            ip += step;
        }
        chunks.push(Chunk {
            name,
            arity,
            max_slots,
            code,
            lines,
            consts,
        });
    }
    Ok((chunks, want_result))
}

fn decode_const(r: &mut Reader) -> Result<Value, String> {
    Ok(match r.u8()? {
        0 => Value::Null,
        1 => Value::Bool(r.u8()? != 0),
        2 => Value::Int(r.i64()?),
        3 => Value::Float(r.f64()?),
        4 => Value::Str(Rc::new(r.string()?)),
        5 => {
            let chunk = r.u32()? as usize;
            let name = r.string()?;
            Value::Func(Rc::new(FuncRef { chunk, name }))
        }
        t => return Err(format!("unknown constant tag {}", t)),
    })
}

pub fn disasm(chunks: &[Chunk]) -> String {
    let mut out = String::new();
    for (ci, c) in chunks.iter().enumerate() {
        out.push_str(&format!(
            "\n== chunk {} '{}' arity={} slots={} ==\n",
            ci, c.name, c.arity, c.max_slots
        ));
        for (i, v) in c.consts.iter().enumerate() {
            out.push_str(&format!("  const[{}] = {}\n", i, v.to_json()));
        }
        let mut ip = 0usize;
        while ip < c.code.len() {
            let o = c.code[ip];
            let step = if has_operand(o) { 5 } else { 1 };
            let line = c.lines.get(ip).copied().unwrap_or(0);
            if has_operand(o) {
                let a = i32::from_le_bytes([
                    c.code[ip + 1],
                    c.code[ip + 2],
                    c.code[ip + 3],
                    c.code[ip + 4],
                ]);
                let hint = match o {
                    op::CONST => format!("  ; {}", c.consts.get(a as usize).map(|v| v.to_json()).unwrap_or_default()),
                    op::GET_GLOBAL | op::SET_GLOBAL => format!(
                        "  ; {}",
                        c.consts
                            .get(a as usize)
                            .map(|v| v.display())
                            .unwrap_or_default()
                    ),
                    _ => String::new(),
                };
                out.push_str(&format!(
                    "  {:04}  L{:<4} {:<20} {}{}\n",
                    ip,
                    line,
                    op_name(o),
                    a,
                    hint
                ));
            } else {
                out.push_str(&format!("  {:04}  L{:<4} {}\n", ip, line, op_name(o)));
            }
            ip += step;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// compiler
// ---------------------------------------------------------------------------

enum Target {
    Local(u16),
    Global(u32),
}

struct LoopCtx {
    breaks: Vec<usize>,
    conts: Vec<usize>,
}

pub fn compile(src: &str, base_dir: PathBuf, keep_last: bool) -> Result<Vec<Chunk>, String> {
    let stmts = crate::parser::parse(src)?;
    let mut chunks: Vec<Chunk> = vec![Chunk::default()];
    {
        let mut c = Compiler {
            chunks: &mut chunks,
            cur: Chunk::new("<main>".to_string(), 0),
            scopes: vec![Vec::new()],
            scope_starts: vec![0],
            nslots: 0,
            max_slots: 0,
            is_top: true,
            loops: Vec::new(),
            base_dir,
            line: 0,
            const_map: HashMap::new(),
            include_stack: Vec::new(),
        };
        c.compile_stmts(&stmts, keep_last)?;
        c.emit(op::HALT, 0, 0);
        let fin = c.finish();
        chunks[0] = fin;
    }
    Ok(chunks)
}

struct Compiler<'a> {
    chunks: &'a mut Vec<Chunk>,
    cur: Chunk,
    scopes: Vec<Vec<(String, u16)>>,
    scope_starts: Vec<u16>,
    nslots: u16,
    max_slots: u16,
    is_top: bool,
    loops: Vec<LoopCtx>,
    base_dir: PathBuf,
    line: u32,
    const_map: HashMap<String, u32>,
    include_stack: Vec<PathBuf>,
}

impl<'a> Compiler<'a> {
    fn finish(mut self) -> Chunk {
        self.cur.max_slots = self.max_slots;
        self.cur
    }

    fn here(&self) -> usize {
        self.cur.code.len()
    }

    fn emit(&mut self, opcode: u8, arg: i32, line: u32) -> usize {
        let pos = self.cur.code.len();
        self.cur.code.push(opcode);
        self.cur.lines.push(line);
        if has_operand(opcode) {
            self.cur.code.extend_from_slice(&arg.to_le_bytes());
            for _ in 0..4 {
                self.cur.lines.push(line);
            }
        }
        pos
    }

    fn patch(&mut self, pos: usize, target: usize) {
        let bytes = (target as i32).to_le_bytes();
        self.cur.code[pos + 1] = bytes[0];
        self.cur.code[pos + 2] = bytes[1];
        self.cur.code[pos + 3] = bytes[2];
        self.cur.code[pos + 4] = bytes[3];
    }

    fn const_key(v: &Value) -> String {
        match v {
            Value::Null => "n".to_string(),
            Value::Bool(b) => format!("b{}", b),
            Value::Int(i) => format!("i{}", i),
            Value::Float(f) => format!("f{}", f.to_bits()),
            Value::Str(s) => format!("s{}", s),
            Value::Func(f) => format!("F{}:{}", f.chunk, f.name),
            _ => "?".to_string(),
        }
    }

    fn const_idx(&mut self, v: Value) -> u32 {
        let key = Self::const_key(&v);
        if let Some(i) = self.const_map.get(&key) {
            return *i;
        }
        let i = self.cur.consts.len() as u32;
        self.cur.consts.push(v);
        self.const_map.insert(key, i);
        i
    }

    fn name_idx(&mut self, name: &str) -> u32 {
        self.const_idx(str_val(name))
    }

    fn begin_scope(&mut self) {
        self.scopes.push(Vec::new());
        self.scope_starts.push(self.nslots);
    }

    fn end_scope(&mut self) {
        self.scopes.pop();
        if let Some(start) = self.scope_starts.pop() {
            self.nslots = start;
        }
    }

    fn declare_local(&mut self, name: &str) -> u16 {
        let slot = self.nslots;
        self.nslots += 1;
        if self.nslots > self.max_slots {
            self.max_slots = self.nslots;
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.to_string(), slot));
        }
        slot
    }

    fn resolve_local(&self, name: &str) -> Option<u16> {
        for scope in self.scopes.iter().rev() {
            for (n, s) in scope.iter().rev() {
                if n == name {
                    return Some(*s);
                }
            }
        }
        None
    }

    fn compile_stmts(&mut self, stmts: &[Stmt], keep_last: bool) -> Result<(), String> {
        let n = stmts.len();
        for (i, s) in stmts.iter().enumerate() {
            let keep = keep_last && i + 1 == n;
            self.compile_stmt(s, keep)?;
        }
        Ok(())
    }

    fn compile_stmt(&mut self, stmt: &Stmt, keep: bool) -> Result<(), String> {
        self.line = stmt_line(stmt);
        match stmt {
            Stmt::Expr(e, _) => {
                self.compile_expr(e)?;
                if !keep {
                    let line = self.line;
                    self.emit(op::POP, 0, line);
                }
            }
            Stmt::Let(name, init, line) => {
                match init {
                    Some(e) => self.compile_expr(e)?,
                    None => {
                        self.emit(op::NULL, 0, *line);
                    }
                }
                if self.is_top {
                    let k = self.name_idx(name);
                    self.emit(op::SET_GLOBAL, k as i32, *line);
                } else {
                    let slot = self.declare_local(name);
                    self.emit(op::SET_LOCAL, slot as i32, *line);
                }
            }
            Stmt::Fn(name, params, body, line) => {
                let idx = self.compile_function(Some(name.clone()), params, body)?;
                let k = self.const_idx(Value::Func(Rc::new(FuncRef {
                    chunk: idx,
                    name: name.clone(),
                })));
                self.emit(op::CONST, k as i32, *line);
                if self.is_top {
                    let nk = self.name_idx(name);
                    self.emit(op::SET_GLOBAL, nk as i32, *line);
                } else {
                    let slot = self.declare_local(name);
                    self.emit(op::SET_LOCAL, slot as i32, *line);
                }
            }
            Stmt::Block(stmts) => {
                self.begin_scope();
                self.compile_stmts(stmts, false)?;
                self.end_scope();
            }
            Stmt::If(cond, then, els, line) => {
                self.compile_expr(cond)?;
                let jf = self.emit(op::JUMP_IF_FALSE, 0, *line);
                self.begin_scope();
                self.compile_stmts(then, false)?;
                self.end_scope();
                let jend = self.emit(op::JUMP, 0, *line);
                let else_pos = self.here();
                self.patch(jf, else_pos);
                if let Some(els) = els {
                    self.begin_scope();
                    self.compile_stmts(els, false)?;
                    self.end_scope();
                }
                let end = self.here();
                self.patch(jend, end);
            }
            Stmt::While(cond, body, line) => {
                let start = self.here();
                self.compile_expr(cond)?;
                let jf = self.emit(op::JUMP_IF_FALSE, 0, *line);
                self.loops.push(LoopCtx {
                    breaks: Vec::new(),
                    conts: Vec::new(),
                });
                self.begin_scope();
                self.compile_stmts(body, false)?;
                self.end_scope();
                let ctx = self.loops.pop().unwrap();
                self.emit(op::JUMP, start as i32, *line);
                let end = self.here();
                self.patch(jf, end);
                for b in ctx.breaks {
                    self.patch(b, end);
                }
                for c in ctx.conts {
                    self.patch(c, start);
                }
            }
            Stmt::ForC(init, cond, step, body, line) => {
                self.begin_scope();
                if let Some(init) = init {
                    self.compile_stmt(init, false)?;
                }
                let start = self.here();
                let jf = if let Some(cond) = cond {
                    self.compile_expr(cond)?;
                    Some(self.emit(op::JUMP_IF_FALSE, 0, *line))
                } else {
                    None
                };
                self.loops.push(LoopCtx {
                    breaks: Vec::new(),
                    conts: Vec::new(),
                });
                self.begin_scope();
                self.compile_stmts(body, false)?;
                self.end_scope();
                let ctx = self.loops.pop().unwrap();
                let cont = self.here();
                if let Some(step) = step {
                    self.compile_expr(step)?;
                    self.emit(op::POP, 0, *line);
                }
                self.emit(op::JUMP, start as i32, *line);
                let end = self.here();
                if let Some(jf) = jf {
                    self.patch(jf, end);
                }
                for b in ctx.breaks {
                    self.patch(b, end);
                }
                for c in ctx.conts {
                    self.patch(c, cont);
                }
                self.end_scope();
            }
            Stmt::ForIn(name, iter, body, line) => {
                self.begin_scope();
                let seq_slot = self.declare_local("$seq");
                let idx_slot = self.declare_local("$i");
                self.compile_expr(iter)?;
                self.emit(op::ITER_PREP, 0, *line);
                self.emit(op::SET_LOCAL, seq_slot as i32, *line);
                let one = self.const_idx(Value::Int(0));
                self.emit(op::CONST, one as i32, *line);
                self.emit(op::SET_LOCAL, idx_slot as i32, *line);

                let target = if self.is_top {
                    Target::Global(self.name_idx(name))
                } else {
                    Target::Local(self.declare_local(name))
                };

                let start = self.here();
                self.emit(op::GET_LOCAL, idx_slot as i32, *line);
                self.emit(op::GET_LOCAL, seq_slot as i32, *line);
                self.emit(op::LEN, 0, *line);
                self.emit(op::LT, 0, *line);
                let jf = self.emit(op::JUMP_IF_FALSE, 0, *line);

                self.emit(op::GET_LOCAL, seq_slot as i32, *line);
                self.emit(op::GET_LOCAL, idx_slot as i32, *line);
                self.emit(op::INDEX_GET, 0, *line);
                match target {
                    Target::Local(slot) => {
                        self.emit(op::SET_LOCAL, slot as i32, *line);
                    }
                    Target::Global(k) => {
                        self.emit(op::SET_GLOBAL, k as i32, *line);
                    }
                }

                self.loops.push(LoopCtx {
                    breaks: Vec::new(),
                    conts: Vec::new(),
                });
                self.begin_scope();
                self.compile_stmts(body, false)?;
                self.end_scope();
                let ctx = self.loops.pop().unwrap();

                let cont = self.here();
                let c1 = self.const_idx(Value::Int(1));
                self.emit(op::GET_LOCAL, idx_slot as i32, *line);
                self.emit(op::CONST, c1 as i32, *line);
                self.emit(op::ADD, 0, *line);
                self.emit(op::SET_LOCAL, idx_slot as i32, *line);
                self.emit(op::JUMP, start as i32, *line);

                let end = self.here();
                self.patch(jf, end);
                for b in ctx.breaks {
                    self.patch(b, end);
                }
                for c in ctx.conts {
                    self.patch(c, cont);
                }
                self.end_scope();
            }
            Stmt::Return(value, line) => {
                match value {
                    Some(e) => self.compile_expr(e)?,
                    None => {
                        self.emit(op::NULL, 0, *line);
                    }
                }
                self.emit(op::RET, 0, *line);
            }
            Stmt::Break(line) => {
                if self.loops.is_empty() {
                    return Err(format!("line {}: 'break' outside of a loop", line));
                }
                let j = self.emit(op::JUMP, 0, *line);
                self.loops.last_mut().unwrap().breaks.push(j);
            }
            Stmt::Continue(line) => {
                if self.loops.is_empty() {
                    return Err(format!("line {}: 'continue' outside of a loop", line));
                }
                let j = self.emit(op::JUMP, 0, *line);
                self.loops.last_mut().unwrap().conts.push(j);
            }
            Stmt::Use(path, line) => {
                let p = Path::new(path);
                let full = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    self.base_dir.join(p)
                };
                let canon = fs::canonicalize(&full).unwrap_or_else(|_| full.clone());
                if self.include_stack.contains(&canon) {
                    return Err(format!("line {}: circular use of '{}'", line, path));
                }
                let src = fs::read_to_string(&canon).map_err(|e| {
                    format!("line {}: cannot read '{}': {}", line, full.display(), e)
                })?;
                let stmts = crate::parser::parse(&src)
                    .map_err(|e| format!("{}: {}", full.display(), e))?;
                let saved_dir = self.base_dir.clone();
                if let Some(parent) = canon.parent() {
                    self.base_dir = parent.to_path_buf();
                }
                self.include_stack.push(canon);
                let r = self.compile_stmts(&stmts, false);
                self.include_stack.pop();
                self.base_dir = saved_dir;
                r?;
            }
        }
        Ok(())
    }

    fn compile_function(
        &mut self,
        name: Option<String>,
        params: &[String],
        body: &[Stmt],
    ) -> Result<usize, String> {
        let chunk_name = match &name {
            Some(n) => n.clone(),
            None => "<lambda>".to_string(),
        };
        let mut sub = Compiler {
            chunks: &mut *self.chunks,
            cur: Chunk::new(chunk_name, params.len() as u16),
            scopes: vec![Vec::new()],
            scope_starts: vec![0],
            nslots: 0,
            max_slots: 0,
            is_top: false,
            loops: Vec::new(),
            base_dir: self.base_dir.clone(),
            line: self.line,
            const_map: HashMap::new(),
            include_stack: self.include_stack.clone(),
        };
        for p in params {
            sub.declare_local(p);
        }
        sub.compile_stmts(body, false)?;
        sub.emit(op::NULL, 0, 0);
        sub.emit(op::RET, 0, 0);
        let fin = sub.finish();
        let idx = self.chunks.len();
        self.chunks.push(fin);
        Ok(idx)
    }

    fn compile_expr(&mut self, e: &Expr) -> Result<(), String> {
        match e {
            Expr::Null => {
                self.emit(op::NULL, 0, self.line);
            }
            Expr::Bool(b) => {
                self.emit(if *b { op::TRUE } else { op::FALSE }, 0, self.line);
            }
            Expr::Int(i) => {
                let k = self.const_idx(Value::Int(*i));
                self.emit(op::CONST, k as i32, self.line);
            }
            Expr::Float(f) => {
                let k = self.const_idx(Value::Float(*f));
                self.emit(op::CONST, k as i32, self.line);
            }
            Expr::Str(s) => {
                let k = self.const_idx(str_val(s.clone()));
                self.emit(op::CONST, k as i32, self.line);
            }
            Expr::Ident(name) => {
                if let Some(slot) = self.resolve_local(name) {
                    self.emit(op::GET_LOCAL, slot as i32, self.line);
                } else {
                    let k = self.name_idx(name);
                    self.emit(op::GET_GLOBAL, k as i32, self.line);
                }
            }
            Expr::Array(items) => {
                for it in items {
                    self.compile_expr(it)?;
                }
                self.emit(op::MAKE_ARRAY, items.len() as i32, self.line);
            }
            Expr::Object(entries) => {
                for (k, v) in entries {
                    let ki = self.const_idx(str_val(k.clone()));
                    self.emit(op::CONST, ki as i32, self.line);
                    self.compile_expr(v)?;
                }
                self.emit(op::MAKE_OBJECT, entries.len() as i32, self.line);
            }
            Expr::Neg(inner) => {
                self.compile_expr(inner)?;
                self.emit(op::NEG, 0, self.line);
            }
            Expr::Not(inner) => {
                self.compile_expr(inner)?;
                self.emit(op::NOT, 0, self.line);
            }
            Expr::Bin(binop, a, b) => {
                self.compile_expr(a)?;
                self.compile_expr(b)?;
                let o = match binop {
                    BinOp::Add => op::ADD,
                    BinOp::Sub => op::SUB,
                    BinOp::Mul => op::MUL,
                    BinOp::Div => op::DIV,
                    BinOp::Mod => op::MOD,
                    BinOp::Eq => op::EQ,
                    BinOp::Ne => op::NE,
                    BinOp::Lt => op::LT,
                    BinOp::Le => op::LE,
                    BinOp::Gt => op::GT,
                    BinOp::Ge => op::GE,
                };
                self.emit(o, 0, self.line);
            }
            Expr::And(a, b) => {
                self.compile_expr(a)?;
                let j = self.emit(op::JUMP_IF_FALSE_OR_POP, 0, self.line);
                self.compile_expr(b)?;
                let end = self.here();
                self.patch(j, end);
            }
            Expr::Or(a, b) => {
                self.compile_expr(a)?;
                let j = self.emit(op::JUMP_IF_TRUE_OR_POP, 0, self.line);
                self.compile_expr(b)?;
                let end = self.here();
                self.patch(j, end);
            }
            Expr::Call(callee, args, line) => {
                self.compile_expr(callee)?;
                for a in args {
                    self.compile_expr(a)?;
                }
                self.emit(op::CALL, args.len() as i32, *line);
            }
            Expr::MethodCall(base, name, args, line) => {
                let k = self.name_idx(name);
                self.emit(op::GET_GLOBAL, k as i32, *line);
                self.compile_expr(base)?;
                for a in args {
                    self.compile_expr(a)?;
                }
                self.emit(op::CALL, (args.len() + 1) as i32, *line);
            }
            Expr::Index(obj, idx, line) => {
                self.compile_expr(obj)?;
                self.compile_expr(idx)?;
                self.emit(op::INDEX_GET, 0, *line);
            }
            Expr::Member(obj, name, line) => {
                self.compile_expr(obj)?;
                let k = self.const_idx(str_val(name.clone()));
                self.emit(op::CONST, k as i32, *line);
                self.emit(op::INDEX_GET, 0, *line);
            }
            Expr::Assign(target, value, line) => {
                match &**target {
                    Expr::Ident(name) => {
                        self.compile_expr(value)?;
                        self.emit(op::DUP, 0, *line);
                        if let Some(slot) = self.resolve_local(name) {
                            self.emit(op::SET_LOCAL, slot as i32, *line);
                        } else {
                            let k = self.name_idx(name);
                            self.emit(op::SET_GLOBAL, k as i32, *line);
                        }
                    }
                    Expr::Member(obj, name, _) => {
                        self.compile_expr(obj)?;
                        let k = self.const_idx(str_val(name.clone()));
                        self.emit(op::CONST, k as i32, *line);
                        self.compile_expr(value)?;
                        self.emit(op::INDEX_SET_KEEP, 0, *line);
                    }
                    Expr::Index(obj, idx, _) => {
                        self.compile_expr(obj)?;
                        self.compile_expr(idx)?;
                        self.compile_expr(value)?;
                        self.emit(op::INDEX_SET_KEEP, 0, *line);
                    }
                    _ => return Err(format!("line {}: invalid assignment target", line)),
                }
            }
            Expr::Compound(target, binop, value, line) => {
                let o = match binop {
                    BinOp::Add => op::ADD,
                    BinOp::Sub => op::SUB,
                    BinOp::Mul => op::MUL,
                    BinOp::Div => op::DIV,
                    BinOp::Mod => op::MOD,
                    _ => op::ADD,
                };
                match &**target {
                    Expr::Ident(name) => {
                        if let Some(slot) = self.resolve_local(name) {
                            self.emit(op::GET_LOCAL, slot as i32, *line);
                        } else {
                            let k = self.name_idx(name);
                            self.emit(op::GET_GLOBAL, k as i32, *line);
                        }
                        self.compile_expr(value)?;
                        self.emit(o, 0, *line);
                        self.emit(op::DUP, 0, *line);
                        if let Some(slot) = self.resolve_local(name) {
                            self.emit(op::SET_LOCAL, slot as i32, *line);
                        } else {
                            let k = self.name_idx(name);
                            self.emit(op::SET_GLOBAL, k as i32, *line);
                        }
                    }
                    Expr::Member(obj, name, _) => {
                        self.compile_expr(obj)?;
                        let k = self.const_idx(str_val(name.clone()));
                        self.emit(op::CONST, k as i32, *line);
                        self.emit(op::DUP2, 0, *line);
                        self.emit(op::INDEX_GET, 0, *line);
                        self.compile_expr(value)?;
                        self.emit(o, 0, *line);
                        self.emit(op::INDEX_SET_KEEP, 0, *line);
                    }
                    Expr::Index(obj, idx, _) => {
                        self.compile_expr(obj)?;
                        self.compile_expr(idx)?;
                        self.emit(op::DUP2, 0, *line);
                        self.emit(op::INDEX_GET, 0, *line);
                        self.compile_expr(value)?;
                        self.emit(o, 0, *line);
                        self.emit(op::INDEX_SET_KEEP, 0, *line);
                    }
                    _ => {
                        return Err(format!("line {}: invalid compound assignment target", line))
                    }
                }
            }
            Expr::Func(params, body, line) => {
                let idx = self.compile_function(None, params, body)?;
                let k = self.const_idx(Value::Func(Rc::new(FuncRef {
                    chunk: idx,
                    name: "<lambda>".to_string(),
                })));
                self.emit(op::CONST, k as i32, *line);
            }
        }
        Ok(())
    }
}

fn stmt_line(s: &Stmt) -> u32 {
    match s {
        Stmt::Expr(_, l) => *l,
        Stmt::Block(_) => 0,
        Stmt::Let(_, _, l)
        | Stmt::Fn(_, _, _, l)
        | Stmt::If(_, _, _, l)
        | Stmt::While(_, _, l)
        | Stmt::ForC(_, _, _, _, l)
        | Stmt::ForIn(_, _, _, l)
        | Stmt::Return(_, l)
        | Stmt::Break(l)
        | Stmt::Continue(l)
        | Stmt::Use(_, l) => *l,
    }
}

/// Parse a source file into a compiled program.
pub fn compile_file(path: &Path, keep_last: bool) -> Result<Vec<Chunk>, String> {
    let src = fs::read_to_string(path)
        .map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
    let base = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    compile(&src, base, keep_last)
}


