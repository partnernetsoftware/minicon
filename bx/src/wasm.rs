//! Self-contained WebAssembly support for bx: a small encoder for the modules
//! bx emits, a decoder for standard core modules, and an interpreter for the
//! MVP numeric/control subset. No third-party wasm crates.

// ---------------------------------------------------------------------------
// values and types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
}

impl ValType {
    fn code(self) -> u8 {
        match self {
            ValType::I32 => 0x7f,
            ValType::I64 => 0x7e,
            ValType::F32 => 0x7d,
            ValType::F64 => 0x7c,
        }
    }

    fn from_code(code: u8) -> Option<ValType> {
        match code {
            0x7f => Some(ValType::I32),
            0x7e => Some(ValType::I64),
            0x7d => Some(ValType::F32),
            0x7c => Some(ValType::F64),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            ValType::I32 => "i32",
            ValType::I64 => "i64",
            ValType::F32 => "f32",
            ValType::F64 => "f64",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WVal {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

impl WVal {
    fn ty(&self) -> ValType {
        match self {
            WVal::I32(_) => ValType::I32,
            WVal::I64(_) => ValType::I64,
            WVal::F32(_) => ValType::F32,
            WVal::F64(_) => ValType::F64,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

fn zero_value(ty: ValType) -> WVal {
    match ty {
        ValType::I32 => WVal::I32(0),
        ValType::I64 => WVal::I64(0),
        ValType::F32 => WVal::F32(0.0),
        ValType::F64 => WVal::F64(0.0),
    }
}

// ---------------------------------------------------------------------------
// module model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub type_index: u32,
}

#[derive(Clone, Debug)]
pub struct GlobalDef {
    pub ty: ValType,
    pub mutable: bool,
    pub init: WVal,
}

#[derive(Clone, Debug)]
pub enum Instr {
    Op(u8),
    Const(u8, WVal),
    Call(u32),
    Local(u8, u32),
    Global(u8, u32),
    Block {
        opcode: u8,
        end: usize,
        else_at: usize,
        start: usize,
    },
    Else {
        end: usize,
    },
    /// Memory access: opcode, alignment hint, static offset.
    Mem(u8, u32, u32),
    /// `memory.copy` (bulk memory): dst, src, len on the stack.
    MemoryCopy,
    Br(u32),
    BrIf(u32),
    BrTable(Box<[u32]>, u32),
}

#[derive(Clone, Debug)]
pub struct Func {
    pub type_index: u32,
    pub locals: Vec<ValType>,
    pub code: Vec<Instr>,
}

#[derive(Clone, Debug)]
pub struct DataSegment {
    pub offset: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub types: Vec<FuncType>,
    pub imports: Vec<Import>,
    pub funcs: Vec<Func>,
    pub globals: Vec<GlobalDef>,
    pub exports: Vec<(String, u32)>,
    pub memory_min_pages: u32,
    pub exports_memory: bool,
    pub data: Vec<DataSegment>,
}

// ---------------------------------------------------------------------------
// encoder
// ---------------------------------------------------------------------------

fn uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn sleb(out: &mut Vec<u8>, mut value: i64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        if done {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn encode_instr(out: &mut Vec<u8>, instr: &Instr) {
    match instr {
        Instr::Op(op) => out.push(*op),
        Instr::Const(op, value) => {
            out.push(*op);
            match value {
                WVal::I32(v) => sleb(out, *v as i64),
                WVal::I64(v) => sleb(out, *v),
                WVal::F32(v) => out.extend_from_slice(&v.to_le_bytes()),
                WVal::F64(v) => out.extend_from_slice(&v.to_le_bytes()),
            }
        }
        Instr::Call(index) => {
            out.push(0x10);
            uleb(out, *index as u64);
        }
        Instr::Local(op, index) | Instr::Global(op, index) => {
            out.push(*op);
            uleb(out, *index as u64);
        }
        Instr::Block { opcode, .. } => {
            out.push(*opcode);
            out.push(0x40);
        }
        Instr::Else { .. } => out.push(0x05),
        Instr::Mem(op, align, offset) => {
            out.push(*op);
            uleb(out, *align as u64);
            uleb(out, *offset as u64);
        }
        Instr::MemoryCopy => {
            out.push(0xfc);
            uleb(out, 0x0a);
            out.push(0x00);
            out.push(0x00);
        }
        Instr::Br(depth) => {
            out.push(0x0c);
            uleb(out, *depth as u64);
        }
        Instr::BrIf(depth) => {
            out.push(0x0d);
            uleb(out, *depth as u64);
        }
        Instr::BrTable(targets, default) => {
            out.push(0x0e);
            uleb(out, targets.len() as u64);
            for target in targets.iter() {
                uleb(out, *target as u64);
            }
            uleb(out, *default as u64);
        }
    }
}

#[derive(Clone, Debug)]
pub struct FuncDef {
    pub type_index: u32,
    pub locals: Vec<ValType>,
    pub body: Vec<Instr>,
}

/// Minimal module builder: types, imported functions, globals, a start function
/// plus zero or more user functions, and the `_start` export.
pub struct Builder {
    types: Vec<FuncType>,
    imports: Vec<Import>,
    globals: Vec<GlobalDef>,
    body: Vec<Instr>,
    body_type: Option<u32>,
    params: usize,
    functions: Vec<FuncDef>,
    locals: Vec<ValType>,
    memory_min_pages: u32,
    export_memory: bool,
    data: Vec<DataSegment>,
    next_data_offset: u32,
}

impl Builder {
    pub fn new() -> Builder {
        Builder {
            types: Vec::new(),
            imports: Vec::new(),
            globals: Vec::new(),
            body: Vec::new(),
            body_type: None,
            params: 0,
            functions: Vec::new(),
            locals: Vec::new(),
            memory_min_pages: 0,
            export_memory: false,
            data: Vec::new(),
            next_data_offset: 1024,
        }
    }

    /// Declare one linear memory of `pages` 64 KiB pages and export it.
    pub fn enable_memory(&mut self, pages: u32) {
        self.memory_min_pages = pages.max(1);
        self.export_memory = true;
    }

    /// Append a byte string to the data section and return its address in
    /// linear memory. Addresses start at 1024 so the zero page stays untouchable.
    pub fn add_data(&mut self, bytes: &[u8]) -> u32 {
        let offset = self.next_data_offset;
        self.data.push(DataSegment {
            offset,
            bytes: bytes.to_vec(),
        });
        self.next_data_offset += bytes.len() as u32;
        self.next_data_offset = (self.next_data_offset + 7) & !7;
        offset
    }

    pub fn add_type(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        self.types.push(FuncType { params, results });
        (self.types.len() - 1) as u32
    }

    pub fn import_func(&mut self, module: &str, name: &str, type_index: u32) -> u32 {
        self.imports.push(Import {
            module: module.to_string(),
            name: name.to_string(),
            type_index,
        });
        (self.imports.len() - 1) as u32
    }

    pub fn add_global(&mut self, ty: ValType, mutable: bool, init: WVal) {
        self.globals.push(GlobalDef { ty, mutable, init });
    }

    fn flush_pending(&mut self) {
        if let Some(type_index) = self.body_type.take() {
            self.functions.push(FuncDef {
                type_index,
                locals: std::mem::take(&mut self.locals),
                body: std::mem::take(&mut self.body),
            });
        }
    }

    /// Begin a new defined function; returns its index in the function index
    /// space (imports come first, so index = imports.len() + functions.len()).
    pub fn begin_function(&mut self, type_index: u32) -> usize {
        self.flush_pending();
        let param_count = self
            .types
            .get(type_index as usize)
            .map(|ty| ty.params.len())
            .unwrap_or(0);
        self.params = param_count;
        self.body_type = Some(type_index);
        self.locals.clear();
        self.body.clear();
        self.functions.len()
    }

    /// Append one typed local to the current function; returns its index after
    /// the parameters.
    pub fn add_local(&mut self, ty: ValType) -> u32 {
        self.locals.push(ty);
        (self.params + self.locals.len() - 1) as u32
    }

    pub fn push(&mut self, instr: Instr) {
        self.body.push(instr);
    }

    pub fn finish(&mut self, export_name: &str) -> Result<Vec<u8>, String> {
        self.flush_pending();
        if self.functions.is_empty() {
            return Err("no start function was defined".to_string());
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"\0asm");
        out.extend_from_slice(&1u32.to_le_bytes());

        let mut section = Vec::new();
        uleb(&mut section, self.types.len() as u64);
        for ty in &self.types {
            section.push(0x60);
            uleb(&mut section, ty.params.len() as u64);
            for param in &ty.params {
                section.push(param.code());
            }
            uleb(&mut section, ty.results.len() as u64);
            for result in &ty.results {
                section.push(result.code());
            }
        }
        push_section(&mut out, 1, &section);

        let mut section = Vec::new();
        uleb(&mut section, self.imports.len() as u64);
        for import in &self.imports {
            uleb(&mut section, import.module.len() as u64);
            section.extend_from_slice(import.module.as_bytes());
            uleb(&mut section, import.name.len() as u64);
            section.extend_from_slice(import.name.as_bytes());
            section.push(0x00);
            uleb(&mut section, import.type_index as u64);
        }
        push_section(&mut out, 2, &section);

        let mut section = Vec::new();
        uleb(&mut section, self.functions.len() as u64);
        for function in &self.functions {
            uleb(&mut section, function.type_index as u64);
        }
        push_section(&mut out, 3, &section);

        // memory section (id 5): one memory, min pages only
        if self.memory_min_pages > 0 {
            let mut section = Vec::new();
            uleb(&mut section, 1);
            section.push(0x00);
            uleb(&mut section, self.memory_min_pages as u64);
            push_section(&mut out, 5, &section);
        }

        let mut section = Vec::new();
        uleb(&mut section, self.globals.len() as u64);
        for global in &self.globals {
            section.push(global.ty.code());
            section.push(if global.mutable { 1 } else { 0 });
            match global.init {
                WVal::I32(v) => {
                    section.push(0x41);
                    sleb(&mut section, v as i64);
                }
                WVal::I64(v) => {
                    section.push(0x42);
                    sleb(&mut section, v);
                }
                WVal::F32(v) => {
                    section.push(0x43);
                    section.extend_from_slice(&v.to_le_bytes());
                }
                WVal::F64(v) => {
                    section.push(0x44);
                    section.extend_from_slice(&v.to_le_bytes());
                }
            }
            section.push(0x0b);
        }
        push_section(&mut out, 6, &section);

        let mut section = Vec::new();
        uleb(&mut section, if self.export_memory { 2 } else { 1 });
        uleb(&mut section, export_name.len() as u64);
        section.extend_from_slice(export_name.as_bytes());
        section.push(0x00);
        uleb(&mut section, self.imports.len() as u64);
        if self.export_memory {
            uleb(&mut section, 6);
            section.extend_from_slice(b"memory");
            section.push(0x02);
            uleb(&mut section, 0);
        }
        push_section(&mut out, 7, &section);

        let mut section = Vec::new();
        uleb(&mut section, self.functions.len() as u64);
        for function in &self.functions {
            let mut body = Vec::new();
            uleb(&mut body, function.locals.len() as u64);
            for local in &function.locals {
                uleb(&mut body, 1);
                body.push(local.code());
            }
            for instr in &function.body {
                encode_instr(&mut body, instr);
            }
            body.push(0x0b);
            uleb(&mut section, body.len() as u64);
            section.extend_from_slice(&body);
        }
        push_section(&mut out, 10, &section);

        // data section (id 11): active segments into memory 0
        if !self.data.is_empty() {
            let mut section = Vec::new();
            uleb(&mut section, self.data.len() as u64);
            for segment in &self.data {
                section.push(0x00);
                section.push(0x41);
                sleb(&mut section, segment.offset as i64);
                section.push(0x0b);
                uleb(&mut section, segment.bytes.len() as u64);
                section.extend_from_slice(&segment.bytes);
            }
            push_section(&mut out, 11, &section);
        }

        Ok(out)
    }
}

fn push_section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
    out.push(id);
    uleb(out, body.len() as u64);
    out.extend_from_slice(body);
}

// ---------------------------------------------------------------------------
// decoder
// ---------------------------------------------------------------------------

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    fn eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn u8(&mut self) -> Result<u8, String> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| "wasm module truncated".to_string())?;
        self.pos += 1;
        Ok(byte)
    }

    fn uleb(&mut self) -> Result<u64, String> {
        let mut value: u64 = 0;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            value |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift > 63 {
                return Err("wasm LEB128 integer is too long".to_string());
            }
        }
    }

    fn sleb(&mut self) -> Result<i64, String> {
        let mut value: i64 = 0;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            value |= ((byte & 0x7f) as i64) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    value |= -1 << shift;
                }
                return Ok(value);
            }
            if shift > 63 {
                return Err("wasm LEB128 integer is too long".to_string());
            }
        }
    }

    fn name(&mut self) -> Result<String, String> {
        let len = self.uleb()? as usize;
        let end = self.pos.checked_add(len).ok_or("wasm name overflow")?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| "wasm name truncated".to_string())?;
        self.pos = end;
        String::from_utf8(slice.to_vec()).map_err(|_| "wasm name is not UTF-8".to_string())
    }

    fn bytes(&mut self, len: usize) -> Result<Vec<u8>, String> {
        let end = self.pos.checked_add(len).ok_or("wasm bytes overflow")?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| "wasm bytes truncated".to_string())?;
        self.pos = end;
        Ok(slice.to_vec())
    }

    fn sub(&mut self, len: usize) -> Result<Reader<'a>, String> {
        let end = self.pos.checked_add(len).ok_or("wasm section overflow")?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| "wasm section truncated".to_string())?;
        self.pos = end;
        Ok(Reader::new(slice))
    }
}

fn decode_valtype(code: u8) -> Result<ValType, String> {
    ValType::from_code(code).ok_or_else(|| format!("unsupported wasm value type 0x{code:02x}"))
}

fn decode_const_expr(r: &mut Reader) -> Result<(ValType, WVal), String> {
    let opcode = r.u8()?;
    let value = match opcode {
        0x41 => WVal::I32(r.sleb()? as i32),
        0x42 => WVal::I64(r.sleb()?),
        0x43 => {
            let mut bytes = [0u8; 4];
            for byte in bytes.iter_mut() {
                *byte = r.u8()?;
            }
            WVal::F32(f32::from_le_bytes(bytes))
        }
        0x44 => {
            let mut bytes = [0u8; 8];
            for byte in bytes.iter_mut() {
                *byte = r.u8()?;
            }
            WVal::F64(f64::from_le_bytes(bytes))
        }
        other => return Err(format!("unsupported constant opcode 0x{other:02x}")),
    };
    if r.u8()? != 0x0b {
        return Err("constant expression is not terminated by end".to_string());
    }
    Ok((value.ty(), value))
}

fn decode_code_body(r: &mut Reader) -> Result<(Vec<ValType>, Vec<Instr>), String> {
    let local_groups = r.uleb()? as usize;
    let mut locals = Vec::new();
    for _ in 0..local_groups {
        let count = r.uleb()? as usize;
        let ty = decode_valtype(r.u8()?)?;
        if locals.len() + count > 100_000 {
            return Err("wasm function declares too many locals".to_string());
        }
        for _ in 0..count {
            locals.push(ty);
        }
    }
    let mut instrs: Vec<Instr> = Vec::new();
    // Indices of open Block/Else instructions, innermost last.
    let mut open: Vec<usize> = Vec::new();
    loop {
        if r.eof() {
            return Err("wasm function body is missing its end".to_string());
        }
        let opcode = r.u8()?;
        match opcode {
            0x00 | 0x01 | 0x0f | 0x1a | 0x1b => instrs.push(Instr::Op(opcode)),
            0x0b => {
                let end_index = instrs.len();
                instrs.push(Instr::Op(0x0b));
                match open.pop() {
                    Some(index) => {
                        let is_else = matches!(instrs[index], Instr::Else { .. });
                        match &mut instrs[index] {
                            Instr::Block { end, .. } => *end = end_index,
                            Instr::Else { end } => *end = end_index,
                            _ => return Err("bad open block".to_string()),
                        }
                        // An end that closes an `else` also closes its `if`.
                        if is_else {
                            if let Some(parent) = open.pop() {
                                if let Instr::Block { end, .. } = &mut instrs[parent] {
                                    *end = end_index;
                                }
                            }
                        }
                    }
                    None => break, // function body end
                }
            }
            0x02..=0x04 => {
                let block_type = r.u8()?;
                if block_type != 0x40 {
                    return Err(format!(
                        "wasm block type 0x{block_type:02x} is not supported (empty blocks only)"
                    ));
                }
                let start = instrs.len() + 1;
                open.push(instrs.len());
                instrs.push(Instr::Block {
                    opcode,
                    end: 0,
                    else_at: usize::MAX,
                    start,
                });
            }
            0x05 => {
                let block = *open
                    .last()
                    .ok_or_else(|| "wasm else without a block".to_string())?;
                let next = instrs.len() + 1;
                if let Instr::Block {
                    opcode: 0x04,
                    else_at,
                    ..
                } = &mut instrs[block]
                {
                    *else_at = next;
                }
                open.push(instrs.len());
                instrs.push(Instr::Else { end: 0 });
            }
            0x0c | 0x0d => {
                let depth = r.uleb()? as u32;
                instrs.push(if opcode == 0x0c {
                    Instr::Br(depth)
                } else {
                    Instr::BrIf(depth)
                });
            }
            0x0e => {
                let count = r.uleb()? as usize;
                if count > 100_000 {
                    return Err("wasm br_table is too large".to_string());
                }
                let mut targets = Vec::with_capacity(count);
                for _ in 0..count {
                    targets.push(r.uleb()? as u32);
                }
                let default = r.uleb()? as u32;
                instrs.push(Instr::BrTable(targets.into_boxed_slice(), default));
            }
            0x10 => {
                let index = r.uleb()? as u32;
                instrs.push(Instr::Call(index));
            }
            0x28..=0x2b | 0x36..=0x39 => {
                let align = r.uleb()? as u32;
                let offset = r.uleb()? as u32;
                instrs.push(Instr::Mem(opcode, align, offset));
            }
            0xfc => {
                let sub = r.uleb()?;
                match sub {
                    0x0a => {
                        if r.u8()? != 0 || r.u8()? != 0 {
                            return Err("memory.copy must target memories 0 and 0".to_string());
                        }
                        instrs.push(Instr::MemoryCopy);
                    }
                    other => return Err(format!("unsupported bulk memory opcode {other}")),
                }
            }
            0x20..=0x22 => {
                let index = r.uleb()? as u32;
                instrs.push(Instr::Local(opcode, index));
            }
            0x23..=0x24 => {
                let index = r.uleb()? as u32;
                instrs.push(Instr::Global(opcode, index));
            }
            0x41 => instrs.push(Instr::Const(opcode, WVal::I32(r.sleb()? as i32))),
            0x42 => instrs.push(Instr::Const(opcode, WVal::I64(r.sleb()?))),
            0x43 => {
                let mut bytes = [0u8; 4];
                for byte in bytes.iter_mut() {
                    *byte = r.u8()?;
                }
                instrs.push(Instr::Const(opcode, WVal::F32(f32::from_le_bytes(bytes))));
            }
            0x44 => {
                let mut bytes = [0u8; 8];
                for byte in bytes.iter_mut() {
                    *byte = r.u8()?;
                }
                instrs.push(Instr::Const(opcode, WVal::F64(f64::from_le_bytes(bytes))));
            }
            other if is_supported_numeric_opcode(other) => instrs.push(Instr::Op(other)),
            other => return Err(format!("unsupported wasm opcode 0x{other:02x}")),
        }
    }
    Ok((locals, instrs))
}

fn is_supported_numeric_opcode(opcode: u8) -> bool {
    matches!(opcode, 0x45..=0x8a | 0xa7 | 0xad)
}

pub fn decode(bytes: &[u8]) -> Result<Module, String> {
    if bytes.len() < 8 || &bytes[0..4] != b"\0asm" {
        return Err("not a wasm module (bad magic)".to_string());
    }
    if bytes[4..8] != [1, 0, 0, 0] {
        return Err("unsupported wasm version".to_string());
    }
    let mut r = Reader::new(bytes);
    r.pos = 8;
    let mut module = Module {
        types: Vec::new(),
        imports: Vec::new(),
        funcs: Vec::new(),
        globals: Vec::new(),
        exports: Vec::new(),
        memory_min_pages: 0,
        exports_memory: false,
        data: Vec::new(),
    };
    let mut func_type_indices: Vec<u32> = Vec::new();
    let mut code_bodies: Vec<(Vec<ValType>, Vec<Instr>)> = Vec::new();
    while !r.eof() {
        let id = r.u8()?;
        let size = r.uleb()? as usize;
        let mut section = r.sub(size)?;
        match id {
            1 => {
                let count = section.uleb()? as usize;
                if count > 100_000 {
                    return Err("too many wasm types".to_string());
                }
                for _ in 0..count {
                    if section.u8()? != 0x60 {
                        return Err("only function types are supported".to_string());
                    }
                    let np = section.uleb()? as usize;
                    let mut params = Vec::new();
                    for _ in 0..np.min(10_000) {
                        params.push(decode_valtype(section.u8()?)?);
                    }
                    let nr = section.uleb()? as usize;
                    let mut results = Vec::new();
                    for _ in 0..nr.min(10_000) {
                        results.push(decode_valtype(section.u8()?)?);
                    }
                    module.types.push(FuncType { params, results });
                }
            }
            2 => {
                let count = section.uleb()? as usize;
                for _ in 0..count {
                    let imported_module = section.name()?;
                    let name = section.name()?;
                    let kind = section.u8()?;
                    if kind != 0x00 {
                        return Err("only function imports are supported".to_string());
                    }
                    let type_index = section.uleb()? as u32;
                    module.imports.push(Import {
                        module: imported_module,
                        name,
                        type_index,
                    });
                }
            }
            3 => {
                let count = section.uleb()? as usize;
                for _ in 0..count {
                    func_type_indices.push(section.uleb()? as u32);
                }
            }
            6 => {
                let count = section.uleb()? as usize;
                for _ in 0..count {
                    let ty = decode_valtype(section.u8()?)?;
                    let mutable = match section.u8()? {
                        0 => false,
                        1 => true,
                        _ => return Err("bad global mutability".to_string()),
                    };
                    let (init_ty, init) = decode_const_expr(&mut section)?;
                    if init_ty != ty {
                        return Err("global initializer type mismatch".to_string());
                    }
                    module.globals.push(GlobalDef { ty, mutable, init });
                }
            }
            7 => {
                let count = section.uleb()? as usize;
                for _ in 0..count {
                    let name = section.name()?;
                    let kind = section.u8()?;
                    let index = section.uleb()? as u32;
                    match kind {
                        0x00 => module.exports.push((name, index)),
                        0x02 => {
                            if index != 0 {
                                return Err("memory export must refer to memory 0".to_string());
                            }
                            module.exports_memory = true;
                        }
                        other => {
                            return Err(format!("unsupported wasm export kind 0x{other:02x}"));
                        }
                    }
                }
            }
            5 => {
                let count = section.uleb()? as usize;
                if count != 1 {
                    return Err("only one linear memory is supported".to_string());
                }
                let flags = section.uleb()?;
                if flags > 1 {
                    return Err("unsupported memory limits".to_string());
                }
                let min = section.uleb()? as u32;
                if flags == 1 {
                    let _max = section.uleb()?;
                }
                module.memory_min_pages = min;
            }
            11 => {
                let count = section.uleb()? as usize;
                for _ in 0..count {
                    let flags = section.uleb()?;
                    if flags != 0 {
                        return Err(
                            "only active data segments for memory 0 are supported".to_string()
                        );
                    }
                    let (offset_ty, offset_value) = decode_const_expr(&mut section)?;
                    if offset_ty != ValType::I32 {
                        return Err("data segment offset must be i32".to_string());
                    }
                    let offset = match offset_value {
                        WVal::I32(value) => value as u32,
                        _ => return Err("bad data segment offset".to_string()),
                    };
                    let len = section.uleb()? as usize;
                    module.data.push(DataSegment {
                        offset,
                        bytes: section.bytes(len)?,
                    });
                }
            }
            10 => {
                let count = section.uleb()? as usize;
                for _ in 0..count {
                    let size = section.uleb()? as usize;
                    let mut body = section.sub(size)?;
                    code_bodies.push(decode_code_body(&mut body)?);
                }
            }
            other => {
                return Err(format!(
                    "wasm section {other} is not supported yet (memory/table/data land with the data model)"
                ));
            }
        }
    }
    if func_type_indices.len() != code_bodies.len() {
        return Err("function and code section lengths differ".to_string());
    }
    for (index, type_index) in func_type_indices.into_iter().enumerate() {
        if type_index as usize >= module.types.len() {
            return Err("function refers to an unknown type".to_string());
        }
        let (locals, code) = code_bodies[index].clone();
        module.funcs.push(Func {
            type_index,
            locals,
            code,
        });
    }
    Ok(module)
}

// ---------------------------------------------------------------------------
// interpreter
// ---------------------------------------------------------------------------

pub trait Host {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[WVal],
        memory: &mut [u8],
    ) -> Result<Vec<WVal>, String>;
}

#[derive(Clone, Copy)]
struct Label {
    kind: u8,
    end: usize,
    start: usize,
    height: usize,
}

struct Exec<'m, H: Host> {
    module: &'m Module,
    globals: Vec<WVal>,
    memory: Vec<u8>,
    stack: Vec<WVal>,
    labels: Vec<Label>,
    host: H,
    depth: usize,
}

fn pop(stack: &mut Vec<WVal>) -> Result<WVal, String> {
    stack
        .pop()
        .ok_or_else(|| "wasm operand stack underflow".to_string())
}

fn pop_i32(stack: &mut Vec<WVal>) -> Result<i32, String> {
    match pop(stack)? {
        WVal::I32(value) => Ok(value),
        other => Err(format!("expected i32, got {}", other.ty().name())),
    }
}

fn pop_i64(stack: &mut Vec<WVal>) -> Result<i64, String> {
    match pop(stack)? {
        WVal::I64(value) => Ok(value),
        other => Err(format!("expected i64, got {}", other.ty().name())),
    }
}

fn pop_f32(stack: &mut Vec<WVal>) -> Result<f32, String> {
    match pop(stack)? {
        WVal::F32(value) => Ok(value),
        other => Err(format!("expected f32, got {}", other.ty().name())),
    }
}

fn pop_f64(stack: &mut Vec<WVal>) -> Result<f64, String> {
    match pop(stack)? {
        WVal::F64(value) => Ok(value),
        other => Err(format!("expected f64, got {}", other.ty().name())),
    }
}

impl<'m, H: Host> Exec<'m, H> {
    fn new(module: &'m Module, host: H) -> Exec<'m, H> {
        let globals = module.globals.iter().map(|global| global.init).collect();
        let pages = module.memory_min_pages.min(256) as usize;
        let mut memory = vec![0u8; pages * 65_536];
        for segment in &module.data {
            let start = segment.offset as usize;
            let end = start + segment.bytes.len();
            if end <= memory.len() {
                memory[start..end].copy_from_slice(&segment.bytes);
            }
        }
        Exec {
            module,
            globals,
            memory,
            stack: Vec::new(),
            labels: Vec::new(),
            host,
            depth: 0,
        }
    }

    fn call_func(&mut self, index: u32, args: Vec<WVal>) -> Result<Vec<WVal>, String> {
        let import_count = self.module.imports.len() as u32;
        if index < import_count {
            let import = &self.module.imports[index as usize];
            return self
                .host
                .call(&import.module, &import.name, &args, &mut self.memory);
        }
        let func = self
            .module
            .funcs
            .get((index - import_count) as usize)
            .ok_or_else(|| format!("call to unknown function {index}"))?;
        let func_type = self
            .module
            .types
            .get(func.type_index as usize)
            .ok_or_else(|| "function type index out of range".to_string())?;
        if args.len() != func_type.params.len() {
            return Err(format!(
                "function expects {} arguments, got {}",
                func_type.params.len(),
                args.len()
            ));
        }
        for (arg, expected) in args.iter().zip(func_type.params.iter()) {
            if arg.ty() != *expected {
                return Err(format!(
                    "argument type mismatch: expected {}, got {}",
                    expected.name(),
                    arg.ty().name()
                ));
            }
        }
        let mut locals = args;
        for local in &func.locals {
            locals.push(zero_value(*local));
        }
        self.depth += 1;
        if self.depth > 1024 {
            self.depth -= 1;
            return Err("wasm call depth exceeded".to_string());
        }
        let base = self.stack.len();
        let code = &func.code;
        let labels_len = self.labels.len();
        self.labels.push(Label {
            kind: 0x02,
            end: code.len(),
            start: 0,
            height: base,
        });
        let outcome = self.run_body(code, &mut locals, base);
        self.labels.truncate(labels_len);
        self.depth -= 1;
        match outcome {
            Ok(values) => {
                let expected = func_type.results.len();
                if values.len() != expected {
                    return Err(format!(
                        "function returned {} values, expected {}",
                        values.len(),
                        expected
                    ));
                }
                Ok(values)
            }
            Err(error) => {
                self.stack.truncate(base);
                Err(error)
            }
        }
    }

    fn run_body(
        &mut self,
        code: &[Instr],
        locals: &mut [WVal],
        base: usize,
    ) -> Result<Vec<WVal>, String> {
        let mut ip = 0usize;
        let mut returned: Option<Vec<WVal>> = None;
        while ip < code.len() {
            match &code[ip] {
                Instr::Op(op) => match *op {
                    0x00 => return Err("wasm unreachable executed".to_string()),
                    0x01 => {}
                    0x0b => {
                        let label = self
                            .labels
                            .pop()
                            .ok_or_else(|| "wasm end without a label".to_string())?;
                        if self.labels.is_empty() {
                            returned = Some(self.stack.split_off(label.height.max(base)));
                            break;
                        }
                        self.stack.truncate(label.height);
                    }
                    0x0f => {
                        let root = self
                            .labels
                            .first()
                            .copied()
                            .ok_or_else(|| "wasm return without a frame".to_string())?;
                        self.labels.truncate(1);
                        returned = Some(self.stack.split_off(root.height.max(base)));
                        break;
                    }
                    0x1a => {
                        pop(&mut self.stack)?;
                    }
                    0x1b => {
                        let cond = pop_i32(&mut self.stack)?;
                        let b = pop(&mut self.stack)?;
                        let a = pop(&mut self.stack)?;
                        self.stack.push(if cond != 0 { a } else { b });
                    }
                    other => self.exec_numeric(other)?,
                },
                Instr::Const(_, value) => self.stack.push(*value),
                Instr::Mem(op, align, offset) => self.exec_mem(*op, *align, *offset)?,
                Instr::MemoryCopy => {
                    let len = pop_i32(&mut self.stack)? as usize;
                    let src = pop_i32(&mut self.stack)? as usize;
                    let dst = pop_i32(&mut self.stack)? as usize;
                    let end_src = src
                        .checked_add(len)
                        .ok_or_else(|| "memory.copy source overflow".to_string())?;
                    let end_dst = dst
                        .checked_add(len)
                        .ok_or_else(|| "memory.copy destination overflow".to_string())?;
                    if end_src > self.memory.len() || end_dst > self.memory.len() {
                        return Err("memory.copy out of bounds".to_string());
                    }
                    self.memory.copy_within(src..end_src, dst);
                }
                Instr::Call(index) => {
                    let index = *index;
                    let type_index = self.function_type(index)?;
                    let arity = self.module.types[type_index as usize].params.len();
                    if self.stack.len() < arity {
                        return Err("wasm call argument underflow".to_string());
                    }
                    let start = self.stack.len() - arity;
                    let args = self.stack.split_off(start);
                    let results = self.call_func(index, args)?;
                    self.stack.extend(results);
                }
                Instr::Local(op, index) => {
                    let index = *index as usize;
                    let op = *op;
                    let slot = locals
                        .get_mut(index)
                        .ok_or_else(|| format!("local index {index} out of range"))?;
                    match op {
                        0x20 => self.stack.push(*slot),
                        0x21 => *slot = pop(&mut self.stack)?,
                        0x22 => {
                            *slot = *self
                                .stack
                                .last()
                                .ok_or_else(|| "wasm operand stack underflow".to_string())?;
                        }
                        _ => return Err("bad local opcode".to_string()),
                    }
                }
                Instr::Global(op, index) => {
                    let index = *index as usize;
                    let op = *op;
                    let current = *self
                        .globals
                        .get(index)
                        .ok_or_else(|| format!("global index {index} out of range"))?;
                    match op {
                        0x23 => self.stack.push(current),
                        0x24 => {
                            let new_value = pop(&mut self.stack)?;
                            if new_value.ty() != current.ty() {
                                return Err("global type mismatch".to_string());
                            }
                            self.globals[index] = new_value;
                        }
                        _ => return Err("bad global opcode".to_string()),
                    }
                }
                Instr::Block {
                    opcode,
                    end,
                    start,
                    else_at,
                } => {
                    let opcode = *opcode;
                    let end = *end;
                    let else_at = *else_at;
                    let cond = if opcode == 0x04 {
                        Some(pop_i32(&mut self.stack)?)
                    } else {
                        None
                    };
                    self.labels.push(Label {
                        kind: opcode,
                        end,
                        start: *start,
                        height: self.stack.len(),
                    });
                    if let Some(cond) = cond {
                        if cond == 0 {
                            ip = if else_at == usize::MAX { end } else { else_at };
                            continue;
                        }
                    }
                }
                Instr::Else { end } => {
                    ip = *end;
                    continue;
                }
                Instr::Br(depth) => match self.branch(*depth, base)? {
                    Some(next) => {
                        ip = next;
                        continue;
                    }
                    None => {
                        returned = Some(std::mem::take(&mut self.stack));
                        break;
                    }
                },
                Instr::BrIf(depth) => {
                    let cond = pop_i32(&mut self.stack)?;
                    if cond != 0 {
                        match self.branch(*depth, base)? {
                            Some(next) => {
                                ip = next;
                                continue;
                            }
                            None => {
                                returned = Some(std::mem::take(&mut self.stack));
                                break;
                            }
                        }
                    }
                }
                Instr::BrTable(targets, default) => {
                    let index = pop_i32(&mut self.stack)? as usize;
                    let depth = targets.get(index).copied().unwrap_or(*default);
                    match self.branch(depth, base)? {
                        Some(next) => {
                            ip = next;
                            continue;
                        }
                        None => {
                            returned = Some(std::mem::take(&mut self.stack));
                            break;
                        }
                    }
                }
            }
            ip += 1;
        }
        Ok(returned.unwrap_or_default())
    }
    fn exec_mem(&mut self, opcode: u8, _align: u32, offset: u32) -> Result<(), String> {
        let at = |addr: i32| (addr as usize).saturating_add(offset as usize);
        match opcode {
            0x28 => {
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let bytes = self
                    .memory
                    .get(start..start + 4)
                    .ok_or_else(|| "wasm i32.load out of bounds".to_string())?;
                let value = i32::from_le_bytes(bytes.try_into().unwrap());
                self.stack.push(WVal::I32(value));
            }
            0x29 => {
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let bytes = self
                    .memory
                    .get(start..start + 8)
                    .ok_or_else(|| "wasm i64.load out of bounds".to_string())?;
                let value = i64::from_le_bytes(bytes.try_into().unwrap());
                self.stack.push(WVal::I64(value));
            }
            0x2a => {
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let bytes = self
                    .memory
                    .get(start..start + 4)
                    .ok_or_else(|| "wasm f32.load out of bounds".to_string())?;
                let value = f32::from_le_bytes(bytes.try_into().unwrap());
                self.stack.push(WVal::F32(value));
            }
            0x2b => {
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let bytes = self
                    .memory
                    .get(start..start + 8)
                    .ok_or_else(|| "wasm f64.load out of bounds".to_string())?;
                let value = f64::from_le_bytes(bytes.try_into().unwrap());
                self.stack.push(WVal::F64(value));
            }
            0x36 => {
                let value = pop_i32(&mut self.stack)?;
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let slot = self
                    .memory
                    .get_mut(start..start + 4)
                    .ok_or_else(|| "wasm i32.store out of bounds".to_string())?;
                slot.copy_from_slice(&value.to_le_bytes());
            }
            0x37 => {
                let value = pop_i64(&mut self.stack)?;
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let slot = self
                    .memory
                    .get_mut(start..start + 8)
                    .ok_or_else(|| "wasm i64.store out of bounds".to_string())?;
                slot.copy_from_slice(&value.to_le_bytes());
            }
            0x38 => {
                let value = pop_f32(&mut self.stack)?;
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let slot = self
                    .memory
                    .get_mut(start..start + 4)
                    .ok_or_else(|| "wasm f32.store out of bounds".to_string())?;
                slot.copy_from_slice(&value.to_le_bytes());
            }
            0x39 => {
                let value = pop_f64(&mut self.stack)?;
                let addr = pop_i32(&mut self.stack)?;
                let start = at(addr);
                let slot = self
                    .memory
                    .get_mut(start..start + 8)
                    .ok_or_else(|| "wasm f64.store out of bounds".to_string())?;
                slot.copy_from_slice(&value.to_le_bytes());
            }
            other => return Err(format!("unsupported memory opcode 0x{other:02x}")),
        }
        Ok(())
    }

    fn function_type(&self, index: u32) -> Result<u32, String> {
        let import_count = self.module.imports.len() as u32;
        if index < import_count {
            Ok(self.module.imports[index as usize].type_index)
        } else {
            self.module
                .funcs
                .get((index - import_count) as usize)
                .map(|func| func.type_index)
                .ok_or_else(|| format!("call to unknown function {index}"))
        }
    }

    /// Returns `Some(next_ip)` for a branch inside the function, or `None` for a
    /// branch to the function label (a return).
    fn branch(&mut self, depth: u32, base: usize) -> Result<Option<usize>, String> {
        let depth = depth as usize;
        let target = self
            .labels
            .len()
            .checked_sub(1 + depth)
            .ok_or_else(|| "wasm branch depth out of range".to_string())?;
        let label = self.labels[target];
        self.stack.truncate(label.height.max(base));
        if target == 0 {
            self.labels.truncate(1);
            return Ok(None);
        }
        if label.kind == 0x03 {
            self.labels.truncate(target + 1);
            Ok(Some(label.start))
        } else {
            self.labels.truncate(target);
            Ok(Some(label.end + 1))
        }
    }

    fn exec_numeric(&mut self, opcode: u8) -> Result<(), String> {
        let stack = &mut self.stack;
        match opcode {
            0x45 => {
                let v = pop_i32(stack)?;
                stack.push(WVal::I32((v == 0) as i32));
            }
            0x46..=0x4f => {
                let b = pop_i32(stack)?;
                let a = pop_i32(stack)?;
                stack.push(WVal::I32(i32_compare(opcode, a, b) as i32));
            }
            0x50 => {
                let v = pop_i64(stack)?;
                stack.push(WVal::I32((v == 0) as i32));
            }
            0x51..=0x5a => {
                let b = pop_i64(stack)?;
                let a = pop_i64(stack)?;
                stack.push(WVal::I32(i64_compare(opcode, a, b) as i32));
            }
            0x5b..=0x60 => {
                let b = pop_f32(stack)?;
                let a = pop_f32(stack)?;
                stack.push(WVal::I32(f32_compare(opcode, a, b) as i32));
            }
            0x61..=0x66 => {
                let b = pop_f64(stack)?;
                let a = pop_f64(stack)?;
                stack.push(WVal::I32(f64_compare(opcode, a, b) as i32));
            }
            0x67 => {
                let v = pop_i32(stack)?;
                stack.push(WVal::I32(v.leading_zeros() as i32));
            }
            0x68 => {
                let v = pop_i32(stack)?;
                stack.push(WVal::I32(v.trailing_zeros() as i32));
            }
            0x69 => {
                let v = pop_i32(stack)?;
                stack.push(WVal::I32(v.count_ones() as i32));
            }
            0x6a..=0x78 => {
                let b = pop_i32(stack)?;
                let a = pop_i32(stack)?;
                stack.push(WVal::I32(i32_binop(opcode, a, b)?));
            }
            0x79 => {
                let v = pop_i64(stack)?;
                stack.push(WVal::I64(v.leading_zeros() as i64));
            }
            0x7a => {
                let v = pop_i64(stack)?;
                stack.push(WVal::I64(v.trailing_zeros() as i64));
            }
            0x7b => {
                let v = pop_i64(stack)?;
                stack.push(WVal::I64(v.count_ones() as i64));
            }
            0x7c..=0x8a => {
                let b = pop_i64(stack)?;
                let a = pop_i64(stack)?;
                stack.push(WVal::I64(i64_binop(opcode, a, b)?));
            }
            0xa7 => {
                let value = pop_i64(stack)?;
                stack.push(WVal::I32(value as i32));
            }
            0xad => {
                let value = pop_i32(stack)?;
                stack.push(WVal::I64(value as u32 as i64));
            }
            other => return Err(format!("unsupported wasm opcode 0x{other:02x}")),
        }
        Ok(())
    }
}

fn i32_compare(opcode: u8, a: i32, b: i32) -> bool {
    match opcode {
        0x46 => a == b,
        0x47 => a != b,
        0x48 => a < b,
        0x49 => a <= b,
        0x4a => a > b,
        0x4b => a >= b,
        0x4c => (a as u32) < (b as u32),
        0x4d => (a as u32) <= (b as u32),
        0x4e => (a as u32) > (b as u32),
        _ => (a as u32) >= (b as u32),
    }
}

fn i64_compare(opcode: u8, a: i64, b: i64) -> bool {
    match opcode {
        0x51 => a == b,
        0x52 => a != b,
        0x53 => a < b,
        0x54 => a <= b,
        0x55 => a > b,
        0x56 => a >= b,
        0x57 => (a as u64) < (b as u64),
        0x58 => (a as u64) <= (b as u64),
        0x59 => (a as u64) > (b as u64),
        _ => (a as u64) >= (b as u64),
    }
}

fn f32_compare(opcode: u8, a: f32, b: f32) -> bool {
    match opcode {
        0x5b => a == b,
        0x5c => a != b,
        0x5d => a < b,
        0x5e => a <= b,
        0x5f => a > b,
        _ => a >= b,
    }
}

fn f64_compare(opcode: u8, a: f64, b: f64) -> bool {
    match opcode {
        0x61 => a == b,
        0x62 => a != b,
        0x63 => a < b,
        0x64 => a <= b,
        0x65 => a > b,
        _ => a >= b,
    }
}

fn i32_binop(opcode: u8, a: i32, b: i32) -> Result<i32, String> {
    Ok(match opcode {
        0x6a => a.wrapping_add(b),
        0x6b => a.wrapping_sub(b),
        0x6c => a.wrapping_mul(b),
        0x6d => {
            if b == 0 {
                return Err("wasm trap: integer divide by zero".to_string());
            }
            a.wrapping_div(b)
        }
        0x6e => {
            if b == 0 {
                return Err("wasm trap: integer divide by zero".to_string());
            }
            a.wrapping_rem(b)
        }
        0x6f => a & b,
        0x70 => a | b,
        0x71 => a ^ b,
        0x72 => a.wrapping_shl(b as u32 & 31),
        0x73 => a.wrapping_shr(b as u32 & 31),
        0x74 => (a as u32).wrapping_shr(b as u32 & 31) as i32,
        0x75 => a.rotate_left(b as u32 & 31),
        0x76 => a.rotate_right(b as u32 & 31),
        _ => return Err(format!("unsupported i32 opcode 0x{opcode:02x}")),
    })
}

fn i64_binop(opcode: u8, a: i64, b: i64) -> Result<i64, String> {
    Ok(match opcode {
        0x7c => a.wrapping_add(b),
        0x7d => a.wrapping_sub(b),
        0x7e => a.wrapping_mul(b),
        0x7f => {
            if b == 0 {
                return Err("wasm trap: integer divide by zero".to_string());
            }
            a.wrapping_div(b)
        }
        0x80 => {
            if b == 0 {
                return Err("wasm trap: integer divide by zero".to_string());
            }
            a.wrapping_rem(b)
        }
        0x81 => a & b,
        0x82 => a | b,
        0x83 => a ^ b,
        0x84 => a.wrapping_shl(b as u32 & 63),
        0x85 => a.wrapping_shr(b as u32 & 63),
        0x86 => (a as u64).wrapping_shr(b as u32 & 63) as i64,
        0x87 => a.rotate_left(b as u32 & 63),
        0x88 => a.rotate_right(b as u32 & 63),
        _ => return Err(format!("unsupported i64 opcode 0x{opcode:02x}")),
    })
}

// ---------------------------------------------------------------------------
// public entry points
// ---------------------------------------------------------------------------

pub fn validate(bytes: &[u8]) -> Result<Module, String> {
    decode(bytes)
}

/// Run the `_start` export, forwarding `env.print_i64` and `env.print_utf8`
/// calls to the two sinks.
pub fn run_start<F, G>(bytes: &[u8], mut on_i64: F, mut on_str: G) -> Result<(), String>
where
    F: FnMut(i64),
    G: FnMut(&str),
{
    let module = decode(bytes)?;
    let mut exec = Exec::new(
        &module,
        PrintHost {
            on_i64: &mut on_i64,
            on_str: &mut on_str,
            next_alloc: 8192,
        },
    );
    let start = module
        .exports
        .iter()
        .find(|(name, _)| name == "_start")
        .map(|(_, index)| *index)
        .ok_or_else(|| "wasm module has no _start export".to_string())?;
    exec.call_func(start, Vec::new())?;
    Ok(())
}

struct PrintHost<'a, F: FnMut(i64), G: FnMut(&str)> {
    on_i64: &'a mut F,
    on_str: &'a mut G,
    /// Bump allocator for the prototype `env.alloc` import, 8-byte aligned.
    next_alloc: u32,
}

impl<'a, F: FnMut(i64), G: FnMut(&str)> Host for PrintHost<'a, F, G> {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[WVal],
        memory: &mut [u8],
    ) -> Result<Vec<WVal>, String> {
        match (module, name) {
            ("env", "print_i64") => {
                let value = match args.first() {
                    Some(WVal::I64(value)) => *value,
                    _ => return Err("env.print_i64 expects one i64".to_string()),
                };
                (self.on_i64)(value);
                Ok(vec![WVal::I64(value)])
            }
            ("env", "alloc") => {
                let bytes = match args.first() {
                    Some(WVal::I32(value)) => *value,
                    _ => return Err("env.alloc expects one i32".to_string()),
                };
                if bytes <= 0 {
                    return Ok(vec![WVal::I32(0)]);
                }
                let start = self.next_alloc;
                let end = start.saturating_add(bytes as u32);
                if end as usize > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                self.next_alloc = (end + 7) & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "print_utf8") => {
                let ptr = match args.first() {
                    Some(WVal::I32(value)) => *value as usize,
                    _ => return Err("env.print_utf8 expects (i32 ptr, i32 len)".to_string()),
                };
                let len = match args.get(1) {
                    Some(WVal::I32(value)) => *value as usize,
                    _ => return Err("env.print_utf8 expects (i32 ptr, i32 len)".to_string()),
                };
                let end = ptr
                    .checked_add(len)
                    .ok_or_else(|| "env.print_utf8 range overflow".to_string())?;
                let slice = memory
                    .get(ptr..end)
                    .ok_or_else(|| "env.print_utf8 range is outside memory".to_string())?;
                let text = String::from_utf8_lossy(slice).to_string();
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            _ => Err(format!("unimplemented wasm host import: {module}.{name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Collect {
        numbers: Vec<i64>,
        strings: Vec<String>,
        next_alloc: u32,
    }

    impl Host for Collect {
        fn call(
            &mut self,
            module: &str,
            name: &str,
            args: &[WVal],
            memory: &mut [u8],
        ) -> Result<Vec<WVal>, String> {
            match (module, name) {
                ("env", "print_i64") => {
                    let value = match args.first() {
                        Some(WVal::I64(value)) => *value,
                        _ => return Err("bad argument".to_string()),
                    };
                    self.numbers.push(value);
                    Ok(vec![WVal::I64(value)])
                }
                ("env", "alloc") => {
                    let bytes = match args.first() {
                        Some(WVal::I32(value)) => *value,
                        _ => return Err("env.alloc expects one i32".to_string()),
                    };
                    if bytes <= 0 {
                        return Ok(vec![WVal::I32(0)]);
                    }
                    let start = self.next_alloc;
                    let end = start.saturating_add(bytes as u32);
                    if end as usize > memory.len() {
                        return Ok(vec![WVal::I32(0)]);
                    }
                    self.next_alloc = (end + 7) & !7;
                    Ok(vec![WVal::I32(start as i32)])
                }
                ("env", "print_utf8") => {
                    let ptr = match args.first() {
                        Some(WVal::I32(value)) => *value as usize,
                        _ => return Err("bad argument".to_string()),
                    };
                    let len = match args.get(1) {
                        Some(WVal::I32(value)) => *value as usize,
                        _ => return Err("bad argument".to_string()),
                    };
                    let slice = memory
                        .get(ptr..ptr + len)
                        .ok_or_else(|| "range outside memory".to_string())?;
                    self.strings
                        .push(String::from_utf8_lossy(slice).to_string());
                    Ok(Vec::new())
                }
                _ => Err("unexpected import".to_string()),
            }
        }
    }

    fn run_export(bytes: &[u8]) -> Collect {
        let module = validate(bytes).expect("validate");
        let mut exec = Exec::new(
            &module,
            Collect {
                numbers: Vec::new(),
                strings: Vec::new(),
                next_alloc: 8192,
            },
        );
        exec.call_func(1, Vec::new()).expect("run");
        // Move the host back out of the interpreter.
        let host = exec.host;
        host
    }

    #[test]
    fn encoder_decoder_round_trip() {
        let mut builder = Builder::new();
        let print_ty = builder.add_type(vec![ValType::I64], vec![ValType::I64]);
        let start_ty = builder.add_type(vec![], vec![]);
        builder.import_func("env", "print_i64", print_ty);
        builder.add_global(ValType::I64, true, WVal::I64(0));
        builder.begin_function(start_ty);
        builder.push(Instr::Const(0x42, WVal::I64(20)));
        builder.push(Instr::Const(0x42, WVal::I64(22)));
        builder.push(Instr::Op(0x7c));
        builder.push(Instr::Global(0x24, 0));
        builder.push(Instr::Global(0x23, 0));
        builder.push(Instr::Call(0));
        builder.push(Instr::Op(0x1a));
        let bytes = builder.finish("_start").expect("build");
        assert_eq!(&bytes[0..4], b"\0asm");
        assert_eq!(run_export(&bytes).numbers, vec![42]);
    }

    #[test]
    fn strings_live_in_linear_memory() {
        let mut builder = Builder::new();
        let print_ty = builder.add_type(vec![ValType::I32, ValType::I32], vec![]);
        let start_ty = builder.add_type(vec![], vec![]);
        builder.import_func("env", "print_utf8", print_ty);
        builder.enable_memory(1);
        let offset = builder.add_data("héllo".as_bytes());
        builder.begin_function(start_ty);
        builder.push(Instr::Const(0x41, WVal::I32(offset as i32)));
        builder.push(Instr::Const(0x41, WVal::I32("héllo".len() as i32)));
        builder.push(Instr::Call(0));
        let bytes = builder.finish("_start").expect("build");
        assert_eq!(run_export(&bytes).strings, vec!["héllo".to_string()]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(validate(b"not wasm").is_err());
        assert!(validate(b"\0asm\x01\x00\x00\x00").is_ok());
        let mut bytes = b"\0asm\x01\x00\x00\x00".to_vec();
        bytes.push(3);
        bytes.push(0);
        assert!(validate(&bytes).is_err());
    }

    #[test]
    fn arithmetic_preserves_precedence() {
        let mut builder = Builder::new();
        let print_ty = builder.add_type(vec![ValType::I64], vec![ValType::I64]);
        let start_ty = builder.add_type(vec![], vec![]);
        builder.import_func("env", "print_i64", print_ty);
        builder.begin_function(start_ty);
        builder.push(Instr::Const(0x42, WVal::I64(7)));
        builder.push(Instr::Const(0x42, WVal::I64(2)));
        builder.push(Instr::Const(0x42, WVal::I64(3)));
        builder.push(Instr::Op(0x7e));
        builder.push(Instr::Op(0x7d));
        builder.push(Instr::Call(0));
        builder.push(Instr::Op(0x1a));
        let bytes = builder.finish("_start").expect("build");
        assert_eq!(run_export(&bytes).numbers, vec![1]);
    }
}
