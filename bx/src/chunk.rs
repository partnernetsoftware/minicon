use std::collections::{HashMap, HashSet};
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
    pub const GET_UPVALUE: u8 = 39;
    pub const MAKE_CLOSURE: u8 = 40;
    pub const ITER_KEYS: u8 = 41;
    pub const GET_CELL: u8 = 42;
    pub const SET_CELL: u8 = 43;
    pub const WRAP_CELL: u8 = 44;
    pub const SET_UPVALUE: u8 = 45;
    pub const GET_UPVALUE_RAW: u8 = 46;
}

/// One bit per opcode, set when the instruction carries a 4-byte operand.
/// A 64-bit test replaces the growing match that ran on every instruction.
const OPERAND_BITS: u64 = (1 << op::CONST)
    | (1 << op::GET_LOCAL)
    | (1 << op::SET_LOCAL)
    | (1 << op::GET_GLOBAL)
    | (1 << op::SET_GLOBAL)
    | (1 << op::JUMP)
    | (1 << op::JUMP_IF_FALSE)
    | (1 << op::JUMP_IF_TRUE)
    | (1 << op::JUMP_IF_FALSE_OR_POP)
    | (1 << op::JUMP_IF_TRUE_OR_POP)
    | (1 << op::CALL)
    | (1 << op::MAKE_ARRAY)
    | (1 << op::MAKE_OBJECT)
    | (1 << op::GET_UPVALUE)
    | (1 << op::MAKE_CLOSURE)
    | (1 << op::GET_CELL)
    | (1 << op::SET_CELL)
    | (1 << op::SET_UPVALUE)
    | (1 << op::GET_UPVALUE_RAW);

pub fn has_operand(o: u8) -> bool {
    o < 64 && (OPERAND_BITS >> o) & 1 == 1
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
        op::GET_UPVALUE => "GET_UPVALUE",
        op::MAKE_CLOSURE => "MAKE_CLOSURE",
        op::ITER_KEYS => "ITER_KEYS",
        op::GET_CELL => "GET_CELL",
        op::SET_CELL => "SET_CELL",
        op::WRAP_CELL => "WRAP_CELL",
        op::SET_UPVALUE => "SET_UPVALUE",
        op::GET_UPVALUE_RAW => "GET_UPVALUE_RAW",
        _ => "?",
    }
}

#[derive(Clone, Default)]
pub struct Chunk {
    /// Shared with the VM so a call frame does not copy the name.
    pub name: Rc<String>,
    pub arity: u16,
    pub max_slots: u16,
    pub code: Vec<u8>,
    pub lines: Vec<u32>,
    pub consts: Vec<Value>,
    /// Source file this chunk came from, for runtime error attribution.
    pub file: Rc<String>,
}

impl Chunk {
    pub fn new(name: String, arity: u16) -> Chunk {
        Chunk {
            name: Rc::new(name),
            arity,
            max_slots: 0,
            code: Vec::new(),
            lines: Vec::new(),
            consts: Vec::new(),
            file: Rc::new(String::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// binary container
// ---------------------------------------------------------------------------

/// v2 adds the per-chunk source file; v1 files still decode (file unknown).
const MAGIC: &[u8; 4] = b"BXB\x02";
const MAGIC_V1: &[u8; 4] = b"BXB\x01";

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
        put_str(&mut out, c.name.as_str());
        put_str(&mut out, c.file.as_str());
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
    if data.len() < 9 || (&data[0..4] != MAGIC && &data[0..4] != MAGIC_V1) {
        return Err("not a bx bytecode file (bad magic)".to_string());
    }
    let format = data[3];
    let mut r = Reader { buf: data, pos: 4 };
    let want_result = r.u8()? != 0;
    let nchunks = r.u32()? as usize;
    if nchunks == 0 || nchunks > 100_000 {
        return Err("bytecode declares an implausible chunk count".to_string());
    }
    let mut chunks = Vec::with_capacity(nchunks);
    for _ in 0..nchunks {
        let name = r.string()?;
        let file = if format >= 2 {
            r.string()?
        } else {
            String::new()
        };
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
            name: Rc::new(name),
            arity,
            max_slots,
            code,
            lines,
            consts,
            file: Rc::new(file),
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
            Value::Func(Rc::new(FuncRef::new(chunk, name)))
        }
        t => return Err(format!("unknown constant tag {}", t)),
    })
}

pub fn disasm(chunks: &[Chunk]) -> String {
    let mut out = String::new();
    for (ci, c) in chunks.iter().enumerate() {
        let origin = if c.file.is_empty() {
            String::new()
        } else {
            format!(" [{}]", c.file)
        };
        out.push_str(&format!(
            "\n== chunk {} '{}' arity={} slots={}{} ==\n",
            ci, c.name, c.arity, c.max_slots, origin
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
                    op::CONST => format!(
                        "  ; {}",
                        c.consts
                            .get(a as usize)
                            .map(|v| v.to_json())
                            .unwrap_or_default()
                    ),
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

pub fn compile(
    src: &str,
    base_dir: PathBuf,
    file: String,
    keep_last: bool,
) -> Result<Vec<Chunk>, String> {
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
            captures: Vec::new(),
            cells: HashSet::new(),
            file,
        };
        c.cur.file = Rc::new(c.file.clone());
        c.compile_stmts(&stmts, keep_last)?;
        c.emit(op::HALT, 0, 0);
        let (fin, _captures) = c.finish();
        chunks[0] = fin;
    }
    Ok(chunks)
}

/// Where a closure fetches one captured value from, in its creator's frame.
#[derive(Clone, Copy)]
enum CaptureSource {
    /// The creator's local slot, which holds a cell.
    Local(u16),
    /// The creator's own upvalue, forwarded from a further level.
    Upvalue(u16),
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
    /// Names this function closed over, and where its creator keeps them.
    captures: Vec<(String, CaptureSource)>,
    /// Locals that a nested function uses; only these are boxed.
    cells: HashSet<String>,
    /// File currently being compiled, for error attribution across `use`.
    file: String,
}

impl<'a> Compiler<'a> {
    fn finish(mut self) -> (Chunk, Vec<(String, CaptureSource)>) {
        self.cur.max_slots = self.max_slots;
        (self.cur, self.captures)
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

    /// Only locals used by a nested function need a box.
    fn is_cell(&self, name: &str) -> bool {
        self.cells.contains(name)
    }

    /// Index of a captured name in this function's upvalue list.
    fn capture_position(&self, name: &str) -> Option<u16> {
        self.captures
            .iter()
            .position(|(n, _)| n == name)
            .map(|i| i as u16)
    }

    /// A for-in binding: top-level names are globals, function-level ones are
    /// fresh locals (matching the surrounding `let` rule).
    fn loop_binding(&mut self, name: &str) -> Target {
        if self.is_top {
            Target::Global(self.name_idx(name))
        } else {
            Target::Local(self.declare_local(name))
        }
    }

    fn emit_set(&mut self, target: &Target, name: &str, line: u32) {
        match target {
            Target::Local(slot) => {
                // A captured loop binding gets a fresh box per iteration, so a
                // closure made in iteration N keeps iteration N's value.
                if self.is_cell(name) {
                    self.emit(op::WRAP_CELL, 0, line);
                }
                self.emit(op::SET_LOCAL, *slot as i32, line);
            }
            Target::Global(k) => {
                self.emit(op::SET_GLOBAL, *k as i32, line);
            }
        }
    }

    /// Push the function constant, then its captured values, then close it.
    fn emit_closure(
        &mut self,
        idx: usize,
        name: &str,
        captures: &[(String, CaptureSource)],
        line: u32,
    ) {
        let k = self.const_idx(Value::Func(Rc::new(FuncRef::new(idx, name.to_string()))));
        self.emit(op::CONST, k as i32, line);
        if !captures.is_empty() {
            for (_, source) in captures {
                match source {
                    CaptureSource::Local(slot) => {
                        self.emit(op::GET_LOCAL, *slot as i32, line);
                    }
                    CaptureSource::Upvalue(i) => {
                        self.emit(op::GET_UPVALUE_RAW, *i as i32, line);
                    }
                }
            }
            self.emit(op::MAKE_CLOSURE, captures.len() as i32, line);
        }
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
                    if self.is_cell(name) {
                        self.emit(op::WRAP_CELL, 0, *line);
                    }
                    self.emit(op::SET_LOCAL, slot as i32, *line);
                }
            }
            Stmt::Fn(name, params, body, line) => {
                let (idx, captures) = self.compile_function(Some(name.clone()), params, body)?;
                self.emit_closure(idx, name, &captures, *line);
                if self.is_top {
                    let nk = self.name_idx(name);
                    self.emit(op::SET_GLOBAL, nk as i32, *line);
                } else {
                    let slot = self.declare_local(name);
                    if self.is_cell(name) {
                        self.emit(op::WRAP_CELL, 0, *line);
                    }
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
            Stmt::ForIn(name, value, iter, body, line) => {
                self.begin_scope();
                let src_slot = self.declare_local("$src");
                let seq_slot = self.declare_local("$seq");
                let idx_slot = self.declare_local("$i");
                let key_slot = if value.is_some() {
                    Some(self.declare_local("$k"))
                } else {
                    None
                };
                self.compile_expr(iter)?;
                self.emit(op::SET_LOCAL, src_slot as i32, *line);
                self.emit(op::GET_LOCAL, src_slot as i32, *line);
                // Key/value form walks keys (objects) or indices (arrays and
                // strings), so `src[k]` is the value in every case.
                let prepare = if value.is_some() {
                    op::ITER_KEYS
                } else {
                    op::ITER_PREP
                };
                self.emit(prepare, 0, *line);
                self.emit(op::SET_LOCAL, seq_slot as i32, *line);
                let zero = self.const_idx(Value::Int(0));
                self.emit(op::CONST, zero as i32, *line);
                self.emit(op::SET_LOCAL, idx_slot as i32, *line);

                let target_key = self.loop_binding(name);
                let target_value = value.as_ref().map(|binding| self.loop_binding(binding));

                let start = self.here();
                self.emit(op::GET_LOCAL, idx_slot as i32, *line);
                self.emit(op::GET_LOCAL, seq_slot as i32, *line);
                self.emit(op::LEN, 0, *line);
                self.emit(op::LT, 0, *line);
                let jf = self.emit(op::JUMP_IF_FALSE, 0, *line);

                match (&target_value, key_slot) {
                    (Some(_), Some(kslot)) => {
                        // key once, then bind the name pair
                        self.emit(op::GET_LOCAL, seq_slot as i32, *line);
                        self.emit(op::GET_LOCAL, idx_slot as i32, *line);
                        self.emit(op::INDEX_GET, 0, *line);
                        self.emit(op::SET_LOCAL, kslot as i32, *line);
                        self.emit(op::GET_LOCAL, kslot as i32, *line);
                        self.emit_set(&target_key, name, *line);
                        self.emit(op::GET_LOCAL, src_slot as i32, *line);
                        self.emit(op::GET_LOCAL, kslot as i32, *line);
                        self.emit(op::INDEX_GET, 0, *line);
                        if let Some(value_target) = &target_value {
                            self.emit_set(value_target, value.as_deref().unwrap_or(""), *line);
                        }
                    }
                    _ => {
                        self.emit(op::GET_LOCAL, seq_slot as i32, *line);
                        self.emit(op::GET_LOCAL, idx_slot as i32, *line);
                        self.emit(op::INDEX_GET, 0, *line);
                        self.emit_set(&target_key, name, *line);
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
                    return Err(format!(
                        "{}: line {}: circular use of '{}'",
                        self.file, line, path
                    ));
                }
                let src = fs::read_to_string(&canon).map_err(|e| {
                    format!(
                        "{}: line {}: cannot read '{}': {}",
                        self.file,
                        line,
                        full.display(),
                        e
                    )
                })?;
                let stmts =
                    crate::parser::parse(&src).map_err(|e| format!("{}: {}", full.display(), e))?;
                let saved_dir = self.base_dir.clone();
                let saved_file = std::mem::take(&mut self.file);
                self.file = full.display().to_string();
                if let Some(parent) = full.parent() {
                    self.base_dir = parent.to_path_buf();
                }
                self.include_stack.push(canon);
                let r = self.compile_stmts(&stmts, false);
                self.include_stack.pop();
                self.base_dir = saved_dir;
                self.file = saved_file;
                if let Err(e) = r {
                    return Err(if error_is_located(&e) {
                        e
                    } else {
                        format!("{}: {}", full.display(), e)
                    });
                }
            }
        }
        Ok(())
    }

    fn compile_function(
        &mut self,
        name: Option<String>,
        params: &[String],
        body: &[Stmt],
    ) -> Result<(usize, Vec<(String, CaptureSource)>), String> {
        let chunk_name = match &name {
            Some(n) => n.clone(),
            None => "<lambda>".to_string(),
        };
        let inherited = inherited_captures(self);
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
            captures: inherited,
            cells: captured_names(body, params),
            file: self.file.clone(),
        };
        sub.cur.file = Rc::new(sub.file.clone());
        for p in params {
            let slot = sub.declare_local(p);
            if sub.is_cell(p) {
                sub.emit(op::GET_LOCAL, slot as i32, 0);
                sub.emit(op::WRAP_CELL, 0, 0);
                sub.emit(op::SET_LOCAL, slot as i32, 0);
            }
        }
        sub.compile_stmts(body, false)?;
        sub.emit(op::NULL, 0, 0);
        sub.emit(op::RET, 0, 0);
        let (fin, captures) = sub.finish();
        let idx = self.chunks.len();
        self.chunks.push(fin);
        Ok((idx, captures))
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
                    let opcode = if self.is_cell(name) {
                        op::GET_CELL
                    } else {
                        op::GET_LOCAL
                    };
                    self.emit(opcode, slot as i32, self.line);
                } else if let Some(i) = self.capture_position(name) {
                    self.emit(op::GET_UPVALUE, i as i32, self.line);
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
            Expr::Ternary(cond, then_branch, else_branch, line) => {
                self.compile_expr(cond)?;
                let jf = self.emit(op::JUMP_IF_FALSE, 0, *line);
                self.compile_expr(then_branch)?;
                let jend = self.emit(op::JUMP, 0, *line);
                let else_pos = self.here();
                self.patch(jf, else_pos);
                self.compile_expr(else_branch)?;
                let end = self.here();
                self.patch(jend, end);
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
            Expr::Assign(target, value, line) => match &**target {
                Expr::Ident(name) => {
                    self.compile_expr(value)?;
                    self.emit(op::DUP, 0, *line);
                    if let Some(slot) = self.resolve_local(name) {
                        let opcode = if self.is_cell(name) {
                            op::SET_CELL
                        } else {
                            op::SET_LOCAL
                        };
                        self.emit(opcode, slot as i32, *line);
                    } else if let Some(i) = self.capture_position(name) {
                        self.emit(op::SET_UPVALUE, i as i32, *line);
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
            },
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
                            let opcode = if self.is_cell(name) {
                                op::GET_CELL
                            } else {
                                op::GET_LOCAL
                            };
                            self.emit(opcode, slot as i32, *line);
                        } else if let Some(i) = self.capture_position(name) {
                            self.emit(op::GET_UPVALUE, i as i32, *line);
                        } else {
                            let k = self.name_idx(name);
                            self.emit(op::GET_GLOBAL, k as i32, *line);
                        }
                        self.compile_expr(value)?;
                        self.emit(o, 0, *line);
                        self.emit(op::DUP, 0, *line);
                        if let Some(slot) = self.resolve_local(name) {
                            let opcode = if self.is_cell(name) {
                                op::SET_CELL
                            } else {
                                op::SET_LOCAL
                            };
                            self.emit(opcode, slot as i32, *line);
                        } else if let Some(i) = self.capture_position(name) {
                            self.emit(op::SET_UPVALUE, i as i32, *line);
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
                    _ => return Err(format!("line {}: invalid compound assignment target", line)),
                }
            }
            Expr::Func(params, body, line) => {
                let (idx, captures) = self.compile_function(None, params, body)?;
                self.emit_closure(idx, "<lambda>", &captures, *line);
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
        | Stmt::ForIn(_, _, _, _, l)
        | Stmt::Return(_, l)
        | Stmt::Break(l)
        | Stmt::Continue(l)
        | Stmt::Use(_, l) => *l,
    }
}

/// Parse a source file into a compiled program.
pub fn compile_file(path: &Path, keep_last: bool) -> Result<Vec<Chunk>, String> {
    let src =
        fs::read_to_string(path).map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
    let base = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    compile(&src, base, path.display().to_string(), keep_last)
}

/// Everything a nested function may see: the creator's locals (as cells) plus
/// the creator's own captures (forwarded), so nesting depth does not limit
/// access to enclosing locals.
fn inherited_captures(parent: &Compiler) -> Vec<(String, CaptureSource)> {
    let mut out: Vec<(String, CaptureSource)> = Vec::new();
    for scope in &parent.scopes {
        for (name, slot) in scope {
            if name.starts_with('$') || !parent.cells.contains(name) {
                continue;
            }
            match out.iter().position(|(n, _)| n == name) {
                Some(pos) => out[pos] = (name.clone(), CaptureSource::Local(*slot)),
                None => out.push((name.clone(), CaptureSource::Local(*slot))),
            }
        }
    }
    for (i, (name, _)) in parent.captures.iter().enumerate() {
        if out.iter().any(|(n, _)| n == name) {
            continue;
        }
        out.push((name.clone(), CaptureSource::Upvalue(i as u16)));
    }
    out
}

/// Locals of one function that a nested function references: `declared here`
/// intersected with identifiers used anywhere inside a nested function. Only
/// these become cells; every other local stays a plain slot.
fn captured_names(body: &[Stmt], params: &[String]) -> HashSet<String> {
    let mut declared: HashSet<String> = params.iter().cloned().collect();
    collect_declared(body, &mut declared);
    let mut used = HashSet::new();
    collect_nested_idents(body, &mut used);
    declared.intersection(&used).cloned().collect()
}

fn collect_declared(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            Stmt::Let(name, _, _) => {
                out.insert(name.clone());
            }
            Stmt::Fn(name, _, _, _) => {
                out.insert(name.clone());
            }
            Stmt::Block(inner) => collect_declared(inner, out),
            Stmt::If(_, then, els, _) => {
                collect_declared(then, out);
                if let Some(els) = els {
                    collect_declared(els, out);
                }
            }
            Stmt::While(_, body, _) => collect_declared(body, out),
            Stmt::ForC(init, _, _, body, _) => {
                if let Some(init) = init {
                    if let Stmt::Let(name, _, _) = &**init {
                        out.insert(name.clone());
                    }
                }
                collect_declared(body, out);
            }
            Stmt::ForIn(key, value, _, body, _) => {
                out.insert(key.clone());
                if let Some(value) = value {
                    out.insert(value.clone());
                }
                collect_declared(body, out);
            }
            _ => {}
        }
    }
}

/// Identifiers used inside nested function bodies (walked whole; the caller
/// intersects with its own declarations, so over-collecting is harmless).
fn collect_nested_idents(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        match s {
            Stmt::Expr(e, _) => collect_nested_expr(e, out),
            Stmt::Let(_, Some(e), _) => collect_nested_expr(e, out),
            Stmt::Fn(_, _, body, _) => collect_all_idents_stmts(body, out),
            Stmt::Block(inner) => collect_nested_idents(inner, out),
            Stmt::If(cond, then, els, _) => {
                collect_nested_expr(cond, out);
                collect_nested_idents(then, out);
                if let Some(els) = els {
                    collect_nested_idents(els, out);
                }
            }
            Stmt::While(cond, body, _) => {
                collect_nested_expr(cond, out);
                collect_nested_idents(body, out);
            }
            Stmt::ForC(init, cond, step, body, _) => {
                if let Some(init) = init {
                    collect_nested_idents(std::slice::from_ref(init), out);
                }
                if let Some(cond) = cond {
                    collect_nested_expr(cond, out);
                }
                if let Some(step) = step {
                    collect_nested_expr(step, out);
                }
                collect_nested_idents(body, out);
            }
            Stmt::ForIn(_, _, iter, body, _) => {
                collect_nested_expr(iter, out);
                collect_nested_idents(body, out);
            }
            Stmt::Return(Some(e), _) => collect_nested_expr(e, out),
            _ => {}
        }
    }
}

fn collect_nested_expr(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Func(_, body, _) => collect_all_idents_stmts(body, out),
        Expr::Array(items) => {
            for item in items {
                collect_nested_expr(item, out);
            }
        }
        Expr::Object(fields) => {
            for (_, value) in fields {
                collect_nested_expr(value, out);
            }
        }
        Expr::Neg(x) | Expr::Not(x) => collect_nested_expr(x, out),
        Expr::Bin(_, a, b) => {
            collect_nested_expr(a, out);
            collect_nested_expr(b, out);
        }
        Expr::Ternary(a, b, c, _) => {
            collect_nested_expr(a, out);
            collect_nested_expr(b, out);
            collect_nested_expr(c, out);
        }
        Expr::And(a, b) | Expr::Or(a, b) => {
            collect_nested_expr(a, out);
            collect_nested_expr(b, out);
        }
        Expr::Call(callee, args, _) => {
            collect_nested_expr(callee, out);
            for arg in args {
                collect_nested_expr(arg, out);
            }
        }
        Expr::MethodCall(obj, _, args, _) => {
            collect_nested_expr(obj, out);
            for arg in args {
                collect_nested_expr(arg, out);
            }
        }
        Expr::Index(obj, idx, _) => {
            collect_nested_expr(obj, out);
            collect_nested_expr(idx, out);
        }
        Expr::Member(obj, _, _) => collect_nested_expr(obj, out),
        Expr::Assign(target, value, _) | Expr::Compound(target, _, value, _) => {
            collect_nested_expr(target, out);
            collect_nested_expr(value, out);
        }
        _ => {}
    }
}

fn collect_all_idents_stmts(stmts: &[Stmt], out: &mut HashSet<String>) {
    for s in stmts {
        collect_all_idents_stmt(s, out);
    }
}

fn collect_all_idents_stmt(s: &Stmt, out: &mut HashSet<String>) {
    match s {
        Stmt::Expr(e, _) => collect_all_idents_expr(e, out),
        Stmt::Let(_, Some(e), _) => collect_all_idents_expr(e, out),
        Stmt::Fn(_, params, body, _) => {
            for p in params {
                out.insert(p.clone());
            }
            collect_all_idents_stmts(body, out);
        }
        Stmt::Block(inner) => collect_all_idents_stmts(inner, out),
        Stmt::If(cond, then, els, _) => {
            collect_all_idents_expr(cond, out);
            collect_all_idents_stmts(then, out);
            if let Some(els) = els {
                collect_all_idents_stmts(els, out);
            }
        }
        Stmt::While(cond, body, _) => {
            collect_all_idents_expr(cond, out);
            collect_all_idents_stmts(body, out);
        }
        Stmt::ForC(init, cond, step, body, _) => {
            if let Some(init) = init {
                collect_all_idents_stmt(init, out);
            }
            if let Some(cond) = cond {
                collect_all_idents_expr(cond, out);
            }
            if let Some(step) = step {
                collect_all_idents_expr(step, out);
            }
            collect_all_idents_stmts(body, out);
        }
        Stmt::ForIn(key, value, iter, body, _) => {
            out.insert(key.clone());
            if let Some(value) = value {
                out.insert(value.clone());
            }
            collect_all_idents_expr(iter, out);
            collect_all_idents_stmts(body, out);
        }
        Stmt::Return(Some(e), _) => collect_all_idents_expr(e, out),
        _ => {}
    }
}

fn collect_all_idents_expr(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Ident(name) => {
            out.insert(name.clone());
        }
        Expr::Array(items) => {
            for item in items {
                collect_all_idents_expr(item, out);
            }
        }
        Expr::Object(fields) => {
            for (_, value) in fields {
                collect_all_idents_expr(value, out);
            }
        }
        Expr::Neg(x) | Expr::Not(x) => collect_all_idents_expr(x, out),
        Expr::Bin(_, a, b) => {
            collect_all_idents_expr(a, out);
            collect_all_idents_expr(b, out);
        }
        Expr::Ternary(a, b, c, _) => {
            collect_all_idents_expr(a, out);
            collect_all_idents_expr(b, out);
            collect_all_idents_expr(c, out);
        }
        Expr::And(a, b) | Expr::Or(a, b) => {
            collect_all_idents_expr(a, out);
            collect_all_idents_expr(b, out);
        }
        Expr::Call(callee, args, _) => {
            collect_all_idents_expr(callee, out);
            for arg in args {
                collect_all_idents_expr(arg, out);
            }
        }
        Expr::MethodCall(obj, _, args, _) => {
            collect_all_idents_expr(obj, out);
            for arg in args {
                collect_all_idents_expr(arg, out);
            }
        }
        Expr::Index(obj, idx, _) => {
            collect_all_idents_expr(obj, out);
            collect_all_idents_expr(idx, out);
        }
        Expr::Member(obj, _, _) => collect_all_idents_expr(obj, out),
        Expr::Assign(target, value, _) | Expr::Compound(target, _, value, _) => {
            collect_all_idents_expr(target, out);
            collect_all_idents_expr(value, out);
        }
        Expr::Func(params, body, _) => {
            for p in params {
                out.insert(p.clone());
            }
            collect_all_idents_stmts(body, out);
        }
        _ => {}
    }
}
/// True when an error already starts with a file location, so a nested `use`
/// does not stack `outer.bxs: inner.bxs: line N` prefixes.
fn error_is_located(e: &str) -> bool {
    match e.split_once(": ") {
        Some((head, _)) => {
            head.ends_with(".bxs")
                || head.ends_with(".bxb")
                || head.contains('/')
                || head.contains('\\')
        }
        None => false,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    fn sample_chunk() -> Chunk {
        let mut chunk = Chunk::new("sample".to_string(), 1);
        chunk.file = Rc::new("lib.bxs".to_string());
        chunk.max_slots = 2;
        chunk.code = vec![op::GET_LOCAL, 0, 0, 0, 0, op::RET];
        chunk.lines = vec![3, 3, 3, 3, 3, 3];
        chunk.consts = vec![Value::Int(7), Value::Str(Rc::new("name".to_string()))];
        chunk
    }

    #[test]
    fn container_roundtrip_keeps_name_file_and_code() {
        let bytes = encode(&[sample_chunk()], true);
        let (chunks, want) = decode(&bytes).expect("decode");
        assert!(want);
        assert_eq!(chunks.len(), 1);
        let decoded = &chunks[0];
        assert_eq!(decoded.name.as_str(), "sample");
        assert_eq!(decoded.file.as_str(), "lib.bxs");
        assert_eq!(decoded.arity, 1);
        assert_eq!(decoded.max_slots, 2);
        assert_eq!(decoded.code, sample_chunk().code);
        assert_eq!(decoded.lines, sample_chunk().lines);
        assert_eq!(decoded.consts.len(), 2);
        assert_eq!(decoded.consts[0].to_json(), "7");
    }

    #[test]
    fn decode_rejects_a_bad_magic() {
        assert!(decode(b"BXB").is_err());
        assert!(decode(b"nope!").is_err());
    }

    #[test]
    fn container_survives_random_chunks_and_any_truncation() {
        // Deterministic xorshift so a failure is reproducible.
        let mut seed: u64 = 0x1234_5678_9abc_def0;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        for _round in 0..20 {
            let mut chunks = Vec::new();
            let chunk_count = 1 + (next() % 3) as usize;
            for index in 0..chunk_count {
                let mut chunk = Chunk::new(format!("f{index}"), (next() % 4) as u16);
                chunk.file = Rc::new(if index % 2 == 0 {
                    "lib.bxs".to_string()
                } else {
                    "üñïçødé.bxs".to_string()
                });
                chunk.max_slots = (next() % 8) as u16;

                let mut code = Vec::new();
                let mut lines = Vec::new();
                let ops = 1 + (next() % 20);
                for _ in 0..ops {
                    let mut opcode = (next() % 47) as u8;
                    if opcode == 36 {
                        // gap between INDEX_SET and INDEX_SET_KEEP
                        opcode = 0;
                    }
                    code.push(opcode);
                    let line = (next() % 50) as u32;
                    lines.push(line);
                    if has_operand(opcode) {
                        let operand = next() as i32;
                        code.extend_from_slice(&operand.to_le_bytes());
                        for _ in 0..4 {
                            lines.push(line);
                        }
                    }
                }
                chunk.code = code;
                chunk.lines = lines;

                let const_count = (next() % 5) as usize;
                let mut consts = Vec::new();
                for _ in 0..const_count {
                    consts.push(match next() % 5 {
                        0 => Value::Null,
                        1 => Value::Bool(next() % 2 == 0),
                        2 => Value::Int(next() as i64),
                        3 => Value::Float((next() % 1000) as f64 / 8.0),
                        _ => Value::Str(Rc::new(format!("s{}", next() % 100))),
                    });
                }
                chunk.consts = consts;
                chunks.push(chunk);
            }

            let want_result = next() % 2 == 0;
            let bytes = encode(&chunks, want_result);
            let (decoded, decoded_want) = decode(&bytes).expect("decode random container");
            assert_eq!(decoded_want, want_result);
            assert_eq!(decoded.len(), chunks.len());
            assert_eq!(
                encode(&decoded, decoded_want),
                bytes,
                "encode(decode(x)) must be byte-identical"
            );

            // Every strict prefix must be rejected, and never panic.
            for cut in 0..bytes.len() {
                assert!(
                    decode(&bytes[..cut]).is_err(),
                    "truncated container of {cut} bytes was accepted"
                );
            }
        }
    }

    #[test]
    fn operand_bits_match_encodings() {
        assert!(has_operand(op::GET_GLOBAL));
        assert!(has_operand(op::SET_LOCAL));
        assert!(!has_operand(op::ADD));
        assert!(!has_operand(op::RET));
        assert!(!has_operand(200));
    }
}
