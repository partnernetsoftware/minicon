//! Wasm support owned by bx: the encoder that emits modules and the native host
//! that serves their imports. Execution, validation and decoding live in the
//! absorbed core (`crate::wasm_core`, vendored `tinyvm`, PRD_02_44).

// ---------------------------------------------------------------------------
// values and types
// ---------------------------------------------------------------------------

#[allow(dead_code)] // The encoder carries the core value surface; bx emits a subset.
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
}

#[derive(Clone, Copy, Debug)]
pub enum WVal {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

#[derive(Clone, Debug)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
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

#[allow(dead_code)] // The encoder carries the core instruction surface; bx emits a subset.
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
pub struct DataSegment {
    pub offset: u32,
    pub bytes: Vec<u8>,
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

pub trait Host {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[WVal],
        memory: &mut [u8],
    ) -> Result<Vec<WVal>, String>;
}
fn store_text_block(memory: &mut [u8], next_file: &mut u32, text: &[u8]) -> Option<u32> {
    let start = *next_file as usize;
    let size = 4 + text.len();
    if start + size > memory.len() {
        return None;
    }
    memory[start..start + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
    memory[start + 4..start + size].copy_from_slice(text);
    *next_file = (start + size + 7) as u32 & !7;
    Some(start as u32)
}
fn memory_text(memory: &[u8], ptr: usize, len: usize) -> String {
    if ptr + len > memory.len() {
        return String::new();
    }
    String::from_utf8_lossy(&memory[ptr..ptr + len]).to_string()
}

fn format_i64_array(memory: &[u8], ptr: usize) -> String {
    if ptr + 4 > memory.len() {
        return "[]".to_string();
    }
    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
    let mut pieces: Vec<String> = Vec::new();
    for index in 0..len {
        let offset = ptr + 4 + index * 8;
        if offset + 8 > memory.len() {
            break;
        }
        let value = i64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
        pieces.push(value.to_string());
    }
    format!("[{}]", pieces.join(","))
}

fn format_str_array(memory: &[u8], ptr: usize) -> String {
    if ptr + 4 > memory.len() {
        return "[]".to_string();
    }
    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
    let mut pieces: Vec<String> = Vec::new();
    for index in 0..len {
        let offset = ptr + 4 + index * 8;
        if offset + 8 > memory.len() {
            break;
        }
        let data = u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(memory[offset + 4..offset + 8].try_into().unwrap()) as usize;
        pieces.push(format!("\"{}\"", memory_text(memory, data, size)));
    }
    format!("[{}]", pieces.join(","))
}

fn format_f64_array(memory: &[u8], ptr: usize) -> String {
    if ptr + 4 > memory.len() {
        return "[]".to_string();
    }
    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
    let mut pieces: Vec<String> = Vec::new();
    for index in 0..len {
        let offset = ptr + 4 + index * 8;
        if offset + 8 > memory.len() {
            break;
        }
        let value = f64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
        pieces.push(if value.is_finite() {
            crate::value::fmt_float(value)
        } else {
            "null".to_string()
        });
    }
    format!("[{}]", pieces.join(","))
}
fn format_map(memory: &[u8], ptr: usize) -> String {
    if ptr + 4 > memory.len() {
        return "{}".to_string();
    }
    let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
    let mut pieces: Vec<String> = Vec::new();
    for index in 0..count.min(64) {
        let at = ptr + 8 + index * 16;
        if at + 16 > memory.len() {
            break;
        }
        let key_ptr = u32::from_le_bytes(memory[at..at + 4].try_into().unwrap()) as usize;
        let key_len = u32::from_le_bytes(memory[at + 4..at + 8].try_into().unwrap()) as usize;
        let value = i64::from_le_bytes(memory[at + 8..at + 16].try_into().unwrap());
        pieces.push(format!(
            "\"{}\":{}",
            memory_text(memory, key_ptr, key_len),
            value
        ));
    }
    format!("{{{}}}", pieces.join(","))
}
fn format_arr_arr(memory: &[u8], ptr: usize) -> String {
    if ptr + 4 > memory.len() {
        return "[]".to_string();
    }
    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
    let mut pieces: Vec<String> = Vec::new();
    for index in 0..len {
        let offset = ptr + 4 + index * 8;
        if offset + 4 > memory.len() {
            break;
        }
        let inner = u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap()) as usize;
        pieces.push(format_i64_array(memory, inner));
    }
    format!("[{}]", pieces.join(","))
}
fn format_record(memory: &[u8], table: usize, count: usize, rec: usize) -> String {
    let mut pieces: Vec<String> = Vec::new();
    for index in 0..count {
        let offset_at = table + index * 4;
        if offset_at + 4 > memory.len() {
            break;
        }
        let block = table
            + u32::from_le_bytes(memory[offset_at..offset_at + 4].try_into().unwrap()) as usize;
        if block + 4 > memory.len() {
            break;
        }
        let size = u32::from_le_bytes(memory[block..block + 4].try_into().unwrap()) as usize;
        let key = memory_text(memory, block + 4, size);
        let field = rec + index * 8;
        if field + 8 > memory.len() {
            break;
        }
        let value = i64::from_le_bytes(memory[field..field + 8].try_into().unwrap());
        pieces.push(format!("\"{}\":{}", key, value));
    }
    format!("{{{}}}", pieces.join(","))
}
/// Join a bool array into text: `true<sep>false`.
fn join_bool_array(memory: &[u8], ptr: usize, sep_ptr: usize, sep_len: usize) -> String {
    let sep = memory_text(memory, sep_ptr, sep_len);
    let mut pieces: Vec<String> = Vec::new();
    if ptr + 4 <= memory.len() {
        let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
        for index in 0..count {
            let offset = ptr + 4 + index * 8;
            if offset + 8 > memory.len() {
                break;
            }
            let value = i64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
            pieces.push(if value != 0 { "true" } else { "false" }.to_string());
        }
    }
    pieces.join(&sep)
}
/// Array of bools rendered as `[true,false]`.
fn format_bool_array(memory: &[u8], ptr: usize) -> String {
    let mut pieces: Vec<String> = Vec::new();
    if ptr + 4 <= memory.len() {
        let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
        for index in 0..count {
            let offset = ptr + 4 + index * 8;
            if offset + 8 > memory.len() {
                break;
            }
            let value = i64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
            pieces.push(if value != 0 { "true" } else { "false" }.to_string());
        }
    }
    format!("[{}]", pieces.join(","))
}
/// Array of string arrays (`[[ "a" ], [ "b", "c" ]]`).
fn format_arr_strarr(memory: &[u8], ptr: usize) -> String {
    let mut pieces: Vec<String> = Vec::new();
    if ptr + 4 <= memory.len() {
        let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
        for index in 0..count {
            let slot = ptr + 4 + index * 8;
            if slot + 8 > memory.len() {
                break;
            }
            let inner = u32::from_le_bytes(memory[slot..slot + 4].try_into().unwrap()) as usize;
            pieces.push(format_str_array(memory, inner));
        }
    }
    format!("[{}]", pieces.join(","))
}

/// Read a `(kinds_ptr, keys_table_ptr, field_count)` descriptor written by the
/// emitter for record and record-array stringification.
fn read_record_descriptor(memory: &[u8], aux: usize) -> (usize, usize, usize) {
    if aux + 12 > memory.len() {
        return (0, 0, 0);
    }
    let kinds = u32::from_le_bytes(memory[aux..aux + 4].try_into().unwrap()) as usize;
    let table = u32::from_le_bytes(memory[aux + 4..aux + 8].try_into().unwrap()) as usize;
    let count = u32::from_le_bytes(memory[aux + 8..aux + 12].try_into().unwrap()) as usize;
    (kinds, table, count)
}

/// Stringify any container the emitter can print: kind 4 int array, 5 string
/// array, 6 float array, 7 record array, 8 nested array, 9 text-row array,
/// 10 record.
fn format_container(memory: &[u8], kind: i32, ptr: usize, aux: usize) -> String {
    match kind {
        4 => format_i64_array(memory, ptr),
        5 => format_str_array(memory, ptr),
        6 => format_f64_array(memory, ptr),
        7 => {
            let (kinds, table, fields) = read_record_descriptor(memory, aux);
            let mut pieces: Vec<String> = Vec::new();
            if ptr + 4 <= memory.len() {
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                for index in 0..count {
                    let slot = ptr + 4 + index * 8;
                    if slot + 8 > memory.len() {
                        break;
                    }
                    let rec =
                        u32::from_le_bytes(memory[slot..slot + 4].try_into().unwrap()) as usize;
                    pieces.push(format_record_typed(memory, kinds, table, fields, rec));
                }
            }
            format!("[{}]", pieces.join(","))
        }
        8 => format_arr_arr(memory, ptr),
        11 => format_bool_array(memory, ptr),
        9 => format_arr_strarr(memory, ptr),
        10 => {
            let (kinds, table, fields) = read_record_descriptor(memory, aux);
            format_record_typed(memory, kinds, table, fields, ptr)
        }
        _ => String::new(),
    }
}

/// Typed variant of [`format_record`]: `kinds` is one byte per field
/// (0 int, 1 string, 2 float, 3 bool) and every field occupies an 8-byte slot.
fn format_record_typed(
    memory: &[u8],
    kinds: usize,
    table: usize,
    count: usize,
    rec: usize,
) -> String {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for index in 0..count {
        let offset_at = table + index * 4;
        if offset_at + 4 > memory.len() {
            break;
        }
        let block = table
            + u32::from_le_bytes(memory[offset_at..offset_at + 4].try_into().unwrap()) as usize;
        if block + 4 > memory.len() {
            break;
        }
        let size = u32::from_le_bytes(memory[block..block + 4].try_into().unwrap()) as usize;
        let key = memory_text(memory, block + 4, size);
        let field = rec + index * 8;
        if field + 8 > memory.len() {
            break;
        }
        let kind = memory.get(kinds + index).copied().unwrap_or(0);
        let rendered = match kind {
            1 => {
                let ptr = u32::from_le_bytes(memory[field..field + 4].try_into().unwrap()) as usize;
                let len =
                    u32::from_le_bytes(memory[field + 4..field + 8].try_into().unwrap()) as usize;
                let mut text = String::new();
                crate::value::write_json_str(&mut text, &memory_text(memory, ptr, len));
                text
            }
            2 => {
                let value = f64::from_le_bytes(memory[field..field + 8].try_into().unwrap());
                if value.is_finite() {
                    crate::value::fmt_float(value)
                } else {
                    "null".to_string()
                }
            }
            3 => {
                let value = i64::from_le_bytes(memory[field..field + 8].try_into().unwrap());
                if value != 0 {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            8 => {
                let cell =
                    u32::from_le_bytes(memory[field..field + 4].try_into().unwrap()) as usize;
                format_dyn_cell(memory, cell, true).unwrap_or_else(|_| "null".to_string())
            }
            _ => i64::from_le_bytes(memory[field..field + 8].try_into().unwrap()).to_string(),
        };
        pairs.push((key, rendered));
    }
    // The VM prints records with sorted keys; keep that order here.
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let pieces: Vec<String> = pairs
        .into_iter()
        .map(|(key, rendered)| format!("\"{}\":{}", key, rendered))
        .collect();
    format!("{{{}}}", pieces.join(","))
}
/// Owned sink types for the core host adapter (PRD_02_44 P1).
pub(crate) type BoxedI64Sink = Box<dyn FnMut(i64)>;
pub(crate) type BoxedStrSink = Box<dyn FnMut(&str)>;

/// Build the native host for the absorbed `tinyvm` core.
///
/// The sinks are leaked for the process lifetime: a CLI run uses one host, and
/// the core adapter needs a `'static` callback table.
pub(crate) fn core_print_host(
    on_i64: BoxedI64Sink,
    on_str: BoxedStrSink,
    on_err: BoxedStrSink,
    argv: &[String],
    script_dir: Option<String>,
) -> PrintHost<'static, BoxedI64Sink, BoxedStrSink> {
    let on_i64: &'static mut BoxedI64Sink = Box::leak(Box::new(on_i64));
    let on_str: &'static mut BoxedStrSink = Box::leak(Box::new(on_str));
    PrintHost {
        on_i64,
        on_str,
        on_err,
        script_dir,
        exec_env: Vec::new(),
        next_alloc: 8192,
        argv: argv.to_vec(),
        next_argv: 65_536,
        next_file: 131_072,
        next_map: 327_680,
        next_proc: 196_608,
        next_dyn: 262_144,
        dyn_null: 0,
        started: std::time::Instant::now(),
        gl: GlBackend::new(),
    }
}
impl<'a, F: FnMut(i64), G: FnMut(&str)> PrintHost<'a, F, G> {
    /// One import call, for adapters that are not the in-tree interpreter.
    pub(crate) fn dispatch(
        &mut self,
        module: &str,
        name: &str,
        args: &[WVal],
        memory: &mut [u8],
    ) -> Result<Vec<WVal>, String> {
        <Self as Host>::call(self, module, name, args, memory)
    }
}
/// Run one command line through the platform shell, mirroring bx `sh`.
/// Allocate one 16-byte dynamic value cell: `[tag u32][aux u32][payload u64]`.
fn alloc_dyn_cell(next_dyn: &mut u32) -> Result<usize, String> {
    let ptr = *next_dyn as usize;
    if ptr + 16 > 327_680 {
        return Err("dynamic value region exhausted".to_string());
    }
    *next_dyn += 16;
    Ok(ptr)
}

fn write_dyn_cell(memory: &mut [u8], ptr: usize, tag: u32, aux: u32, payload: u64) {
    memory[ptr..ptr + 4].copy_from_slice(&tag.to_le_bytes());
    memory[ptr + 4..ptr + 8].copy_from_slice(&aux.to_le_bytes());
    memory[ptr + 8..ptr + 16].copy_from_slice(&payload.to_le_bytes());
}

/// Allocate raw bytes in the dynamic region.
fn alloc_dyn_bytes(next_dyn: &mut u32, bytes: usize) -> Result<usize, String> {
    let ptr = *next_dyn as usize;
    let size = (bytes + 15) & !15;
    if ptr + size > 327_680 {
        return Err("dynamic value region exhausted".to_string());
    }
    *next_dyn += size as u32;
    Ok(ptr)
}

fn dyn_null_cell(memory: &mut [u8], next_dyn: &mut u32, cached: &mut u32) -> Result<usize, String> {
    if *cached != 0 {
        return Ok(*cached as usize);
    }
    let cell = alloc_dyn_cell(next_dyn)?;
    write_dyn_cell(memory, cell, 0, 0, 0);
    *cached = cell as u32;
    Ok(cell)
}

/// Format a dynamic cell (P2a scalars + P2b arrays/objects) as text.
fn format_dyn_cell(memory: &[u8], cell: usize, json: bool) -> Result<String, String> {
    if cell + 16 > memory.len() {
        return Err("dynamic cell out of bounds".to_string());
    }
    let (tag, aux, payload) = dyn_tag(memory, cell);
    Ok(match tag {
        0 => "null".to_string(),
        1 => (payload as i64).to_string(),
        2 => crate::value::fmt_float(f64::from_bits(payload)),
        3 => {
            if payload != 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        4 => {
            let text = memory_text(memory, payload as usize, aux as usize);
            if json {
                let mut rendered = String::new();
                crate::value::write_json_str(&mut rendered, &text);
                rendered
            } else {
                text
            }
        }
        5 => {
            let count = aux as usize;
            let base = payload as usize;
            let mut pieces = Vec::with_capacity(count);
            for index in 0..count {
                pieces.push(format_dyn_cell(memory, base + index * 16, true)?);
            }
            format!("[{}]", pieces.join(","))
        }
        6 => {
            let count = aux as usize;
            let base = payload as usize;
            let mut pieces = Vec::with_capacity(count);
            for index in 0..count {
                let entry = base + index * 12;
                if entry + 12 > memory.len() {
                    break;
                }
                let key_ptr = u32::from_le_bytes(memory[entry..entry + 4].try_into().unwrap());
                let key_len = u32::from_le_bytes(memory[entry + 4..entry + 8].try_into().unwrap());
                let value_ptr =
                    u32::from_le_bytes(memory[entry + 8..entry + 12].try_into().unwrap());
                let key = memory_text(memory, key_ptr as usize, key_len as usize);
                let mut rendered_key = String::new();
                crate::value::write_json_str(&mut rendered_key, &key);
                let rendered_value = format_dyn_cell(memory, value_ptr as usize, true)?;
                pieces.push(format!("{rendered_key}:{rendered_value}"));
            }
            format!("{{{}}}", pieces.join(","))
        }
        _ => return Err("unknown dynamic value tag".to_string()),
    })
}

/// Store a JSON value as a dynamic cell tree (P2a scalars, P2b arrays/objects).
fn store_dyn_scalar(
    memory: &mut [u8],
    next_dyn: &mut u32,
    next_file: &mut u32,
    value: &crate::value::Value,
) -> Result<usize, String> {
    use crate::value::Value;
    match value {
        Value::Array(items) => {
            let cells = items.borrow();
            let count = cells.len();
            let base = alloc_dyn_bytes(next_dyn, count * 16)?;
            let cell = alloc_dyn_cell(next_dyn)?;
            for (index, item) in cells.iter().enumerate() {
                let child = store_dyn_scalar(memory, next_dyn, next_file, item)?;
                let slot = base + index * 16;
                memory.copy_within(child..child + 16, slot);
            }
            write_dyn_cell(memory, cell, 5, count as u32, base as u64);
            Ok(cell)
        }
        Value::Object(entries) => {
            let map = entries.borrow();
            let count = map.len();
            let base = alloc_dyn_bytes(next_dyn, count * 12)?;
            let cell = alloc_dyn_cell(next_dyn)?;
            for (index, (key, item)) in map.iter().enumerate() {
                let start = store_text_block(memory, next_file, key.as_bytes())
                    .ok_or_else(|| "dynamic key does not fit the file region".to_string())?;
                let key_len = u32::from_le_bytes(
                    memory[start as usize..start as usize + 4]
                        .try_into()
                        .unwrap(),
                );
                let child = store_dyn_scalar(memory, next_dyn, next_file, item)?;
                let entry = base + index * 12;
                memory[entry..entry + 4].copy_from_slice(&(start + 4).to_le_bytes());
                memory[entry + 4..entry + 8].copy_from_slice(&key_len.to_le_bytes());
                memory[entry + 8..entry + 12].copy_from_slice(&(child as u32).to_le_bytes());
            }
            write_dyn_cell(memory, cell, 6, count as u32, base as u64);
            Ok(cell)
        }
        Value::Null => {
            let cell = alloc_dyn_cell(next_dyn)?;
            write_dyn_cell(memory, cell, 0, 0, 0);
            Ok(cell)
        }
        Value::Bool(v) => {
            let cell = alloc_dyn_cell(next_dyn)?;
            write_dyn_cell(memory, cell, 3, 0, u64::from(*v));
            Ok(cell)
        }
        Value::Int(v) => {
            let cell = alloc_dyn_cell(next_dyn)?;
            write_dyn_cell(memory, cell, 1, 0, *v as u64);
            Ok(cell)
        }
        Value::Float(v) => {
            let cell = alloc_dyn_cell(next_dyn)?;
            write_dyn_cell(memory, cell, 2, 0, v.to_bits());
            Ok(cell)
        }
        Value::Str(text) => {
            let start = store_text_block(memory, next_file, text.as_bytes())
                .ok_or_else(|| "dynamic string does not fit the file region".to_string())?;
            let len = u32::from_le_bytes(
                memory[start as usize..start as usize + 4]
                    .try_into()
                    .unwrap(),
            );
            let cell = alloc_dyn_cell(next_dyn)?;
            write_dyn_cell(memory, cell, 4, len, u64::from(start) + 4);
            Ok(cell)
        }
        _ => Err("parse() of runtime text only supports JSON data values".to_string()),
    }
}

fn dyn_len_cell(memory: &[u8], cell: usize) -> Result<i64, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    Ok(match tag {
        4 => memory_text(memory, payload as usize, aux as usize)
            .chars()
            .count() as i64,
        5 | 6 => aux as i64,
        0 => return Err("cannot take len of null".to_string()),
        1 => return Err("cannot take len of int".to_string()),
        2 => return Err("cannot take len of float".to_string()),
        3 => return Err("cannot take len of bool".to_string()),
        _ => return Err("cannot take len of this dynamic value".to_string()),
    })
}

fn dyn_index_cell(
    memory: &mut [u8],
    next_dyn: &mut u32,
    null_slot: &mut u32,
    cell: usize,
    index: i64,
) -> Result<usize, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 5 {
        return Err("dynamic index works on dynamic arrays only".to_string());
    }
    let count = aux as i64;
    let mut at = index;
    if at < 0 {
        at += count;
    }
    if at < 0 || at >= count {
        return dyn_null_cell(memory, next_dyn, null_slot);
    }
    Ok(payload as usize + at as usize * 16)
}

fn dyn_member_cell(
    memory: &mut [u8],
    next_dyn: &mut u32,
    null_slot: &mut u32,
    cell: usize,
    key: &str,
) -> Result<usize, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 6 {
        return Err("dynamic field access works on dynamic objects only".to_string());
    }
    let count = aux as usize;
    let base = payload as usize;
    for index in 0..count {
        let entry = base + index * 12;
        if entry + 12 > memory.len() {
            break;
        }
        let key_ptr = u32::from_le_bytes(memory[entry..entry + 4].try_into().unwrap());
        let key_len = u32::from_le_bytes(memory[entry + 4..entry + 8].try_into().unwrap());
        let value_ptr = u32::from_le_bytes(memory[entry + 8..entry + 12].try_into().unwrap());
        let stored = memory_text(memory, key_ptr as usize, key_len as usize);
        if stored == key {
            return Ok(value_ptr as usize);
        }
    }
    dyn_null_cell(memory, next_dyn, null_slot)
}
fn dyn_tag(memory: &[u8], cell: usize) -> (u32, u32, u64) {
    let tag = u32::from_le_bytes(memory[cell..cell + 4].try_into().unwrap());
    let aux = u32::from_le_bytes(memory[cell + 4..cell + 8].try_into().unwrap());
    let payload = u64::from_le_bytes(memory[cell + 8..cell + 16].try_into().unwrap());
    (tag, aux, payload)
}

fn dyn_number(memory: &[u8], cell: usize) -> Result<f64, String> {
    let (tag, _, payload) = dyn_tag(memory, cell);
    match tag {
        0 => Ok(0.0),
        1 => Ok(payload as i64 as f64),
        2 => Ok(f64::from_bits(payload)),
        3 => Ok(if payload != 0 { 1.0 } else { 0.0 }),
        _ => Err("cannot use a dynamic container as a number".to_string()),
    }
}

fn dyn_text(memory: &[u8], cell: usize) -> Option<(u32, u64)> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag == 4 {
        Some((aux, payload))
    } else {
        None
    }
}

fn dyn_eq_cells(memory: &[u8], a: usize, b: usize) -> Result<bool, String> {
    fn eq(memory: &[u8], a: usize, b: usize, depth: u32) -> Result<bool, String> {
        if depth > 256 {
            return Err("dynamic value is too deeply nested to compare".to_string());
        }
        let (tag_a, aux_a, payload_a) = dyn_tag(memory, a);
        let (tag_b, aux_b, payload_b) = dyn_tag(memory, b);
        match (tag_a, tag_b) {
            (5, 5) => {
                if aux_a != aux_b {
                    return Ok(false);
                }
                for index in 0..aux_a as usize {
                    let left = payload_a as usize + index * 16;
                    let right = payload_b as usize + index * 16;
                    if !eq(memory, left, right, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (6, 6) => {
                if aux_a != aux_b {
                    return Ok(false);
                }
                for index in 0..aux_a as usize {
                    let left = payload_a as usize + index * 12;
                    let right = payload_b as usize + index * 12;
                    if dyn_entry_key(memory, left)? != dyn_entry_key(memory, right)? {
                        return Ok(false);
                    }
                    let value_left = dyn_entry_value(memory, left)?;
                    let value_right = dyn_entry_value(memory, right)?;
                    if !eq(memory, value_left, value_right, depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (5, _) | (6, _) | (_, 5) | (_, 6) => Ok(false),
            _ => {
                if let (Some((len_a, ptr_a)), Some((len_b, ptr_b))) =
                    (dyn_text(memory, a), dyn_text(memory, b))
                {
                    return Ok(memory_text(memory, ptr_a as usize, len_a as usize)
                        == memory_text(memory, ptr_b as usize, len_b as usize));
                }
                if tag_a >= 4 || tag_b >= 4 {
                    return Ok(false);
                }
                Ok(dyn_number(memory, a)? == dyn_number(memory, b)?)
            }
        }
    }
    eq(memory, a, b, 0)
}

fn dyn_entry_key(memory: &[u8], entry: usize) -> Result<String, String> {
    if entry + 12 > memory.len() {
        return Err("dynamic object entry out of bounds".to_string());
    }
    let key_ptr = u32::from_le_bytes(memory[entry..entry + 4].try_into().unwrap());
    let key_len = u32::from_le_bytes(memory[entry + 4..entry + 8].try_into().unwrap());
    Ok(memory_text(memory, key_ptr as usize, key_len as usize))
}

fn dyn_entry_value(memory: &[u8], entry: usize) -> Result<usize, String> {
    if entry + 12 > memory.len() {
        return Err("dynamic object entry out of bounds".to_string());
    }
    Ok(u32::from_le_bytes(memory[entry + 8..entry + 12].try_into().unwrap()) as usize)
}

/// Dynamic for-loop value: array element or object entry value.
fn dyn_value_at_cell(
    memory: &mut [u8],
    next_dyn: &mut u32,
    null_slot: &mut u32,
    cell: usize,
    index: i64,
) -> Result<usize, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    match tag {
        5 => dyn_index_cell(memory, next_dyn, null_slot, cell, index),
        6 => {
            let count = aux as i64;
            if index < 0 || index >= count {
                return dyn_null_cell(memory, next_dyn, null_slot);
            }
            dyn_entry_value(memory, payload as usize + index as usize * 12)
        }
        _ => Err("dynamic for-loops need a dynamic array or object".to_string()),
    }
}

/// Concatenate two dynamic arrays into a fresh dynamic array.
fn dyn_concat_cells(
    memory: &mut [u8],
    next_dyn: &mut u32,
    a: usize,
    b: usize,
) -> Result<usize, String> {
    let (tag_a, count_a, payload_a) = dyn_tag(memory, a);
    let (tag_b, count_b, payload_b) = dyn_tag(memory, b);
    if tag_a != 5 || tag_b != 5 {
        return Err("dynamic concatenation needs two arrays".to_string());
    }
    let total = count_a as usize + count_b as usize;
    let base = alloc_dyn_bytes(next_dyn, total.max(1) * 16)?;
    for index in 0..count_a as usize {
        let source = payload_a as usize + index * 16;
        memory.copy_within(source..source + 16, base + index * 16);
    }
    for index in 0..count_b as usize {
        let source = payload_b as usize + index * 16;
        memory.copy_within(source..source + 16, base + (count_a as usize + index) * 16);
    }
    let cell = alloc_dyn_cell(next_dyn)?;
    write_dyn_cell(memory, cell, 5, total as u32, base as u64);
    Ok(cell)
}

/// Convert a static record (kinds/table/count descriptor plus record pointer)
/// into a dynamic object cell.
fn dyn_from_record(
    memory: &mut [u8],
    next_dyn: &mut u32,
    kinds: usize,
    table: usize,
    count: usize,
    rec: usize,
) -> Result<usize, String> {
    let base = alloc_dyn_bytes(next_dyn, count.max(1) * 12)?;
    for index in 0..count {
        let field = rec + index * 8;
        if field + 8 > memory.len() {
            return Err("static record field out of bounds".to_string());
        }
        let offset_at = table + index * 4;
        if offset_at + 4 > memory.len() {
            return Err("static record key table out of bounds".to_string());
        }
        let block = table
            + u32::from_le_bytes(memory[offset_at..offset_at + 4].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(memory[block..block + 4].try_into().unwrap()) as usize;
        let kind = memory.get(kinds + index).copied().unwrap_or(0);
        let cell = alloc_dyn_cell(next_dyn)?;
        match kind {
            0 => {
                let value = i64::from_le_bytes(memory[field..field + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 1, 0, value as u64);
            }
            1 => {
                let ptr = u32::from_le_bytes(memory[field..field + 4].try_into().unwrap());
                let len = u32::from_le_bytes(memory[field + 4..field + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 4, len, u64::from(ptr));
            }
            2 => {
                let bits = u64::from_le_bytes(memory[field..field + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 2, 0, bits);
            }
            3 => {
                let value = i64::from_le_bytes(memory[field..field + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 3, 0, u64::from(value != 0));
            }
            8 => {
                let ptr = u32::from_le_bytes(memory[field..field + 4].try_into().unwrap());
                let _ = cell;
                let entry = base + index * 12;
                memory[entry..entry + 4].copy_from_slice(&(block as u32 + 4).to_le_bytes());
                memory[entry + 4..entry + 8].copy_from_slice(&(size as u32).to_le_bytes());
                memory[entry + 8..entry + 12].copy_from_slice(&ptr.to_le_bytes());
                continue;
            }
            _ => return Err("unsupported record field kind in dyn_from_record".to_string()),
        }
        let entry = base + index * 12;
        memory[entry..entry + 4].copy_from_slice(&((block + 4) as u32).to_le_bytes());
        memory[entry + 4..entry + 8].copy_from_slice(&(size as u32).to_le_bytes());
        memory[entry + 8..entry + 12].copy_from_slice(&(cell as u32).to_le_bytes());
    }
    let cell = alloc_dyn_cell(next_dyn)?;
    write_dyn_cell(memory, cell, 6, count as u32, base as u64);
    Ok(cell)
}
/// Convert a static array (by element kind code) into a dynamic array cell.
fn dyn_from_static_array(
    memory: &mut [u8],
    next_dyn: &mut u32,
    next_file: &mut u32,
    kind: u32,
    ptr: usize,
) -> Result<usize, String> {
    if ptr + 4 > memory.len() {
        return Err("static array pointer out of bounds".to_string());
    }
    let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
    let base = alloc_dyn_bytes(next_dyn, count.max(1) * 16)?;
    for index in 0..count {
        let slot = ptr + 4 + index * 8;
        let cell = base + index * 16;
        match kind {
            4 => {
                let value = i64::from_le_bytes(memory[slot..slot + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 1, 0, value as u64);
            }
            5 => {
                let item_ptr = u32::from_le_bytes(memory[slot..slot + 4].try_into().unwrap());
                let item_len = u32::from_le_bytes(memory[slot + 4..slot + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 4, item_len, u64::from(item_ptr));
            }
            6 => {
                let value = f64::from_le_bytes(memory[slot..slot + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 2, 0, value.to_bits());
            }
            11 => {
                let value = i64::from_le_bytes(memory[slot..slot + 8].try_into().unwrap());
                write_dyn_cell(memory, cell, 3, 0, u64::from(value != 0));
            }
            _ => {
                let _ = next_file;
                return Err(
                    "only int/string/float/bool static arrays concatenate with dynamic ones"
                        .to_string(),
                );
            }
        }
    }
    let cell = alloc_dyn_cell(next_dyn)?;
    write_dyn_cell(memory, cell, 5, count as u32, base as u64);
    Ok(cell)
}
/// Order two numeric-or-string dynamic cells for sort/min/max.
fn dyn_order(memory: &[u8], a: usize, b: usize) -> Result<std::cmp::Ordering, String> {
    let (tag_a, aux_a, payload_a) = dyn_tag(memory, a);
    let (tag_b, aux_b, payload_b) = dyn_tag(memory, b);
    if tag_a == 4 && tag_b == 4 {
        let left = memory_text(memory, payload_a as usize, aux_a as usize);
        let right = memory_text(memory, payload_b as usize, aux_b as usize);
        return Ok(left.cmp(&right));
    }
    if tag_a == 4 || tag_b == 4 {
        return Err("cannot compare a dynamic string with a number".to_string());
    }
    let left = dyn_number(memory, a)?;
    let right = dyn_number(memory, b)?;
    Ok(left
        .partial_cmp(&right)
        .unwrap_or(std::cmp::Ordering::Equal))
}

/// Stable insertion sort of a dynamic array's 16-byte cells.
fn dyn_sort_cells(memory: &mut [u8], cell: usize) -> Result<(), String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 5 {
        return Err("sort() on a dynamic value needs a dynamic array".to_string());
    }
    let count = aux as usize;
    let base = payload as usize;
    for index in 1..count {
        let current = base + index * 16;
        let mut slot = 16usize;
        let mut scan = index;
        while scan > 0 {
            let previous = base + (scan - 1) * 16;
            if dyn_order(memory, previous, current)? != std::cmp::Ordering::Greater {
                break;
            }
            slot = previous;
            scan -= 1;
        }
        if slot != 16 {
            let mut temp = [0u8; 16];
            temp.copy_from_slice(&memory[current..current + 16]);
            memory.copy_within(slot..slot + (index - scan) * 16, slot + 16);
            memory[slot..slot + 16].copy_from_slice(&temp);
        }
    }
    Ok(())
}

fn dyn_reverse_cells(memory: &mut [u8], cell: usize) -> Result<(), String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 5 {
        return Err("reverse() on a dynamic value needs a dynamic array".to_string());
    }
    let count = aux as usize;
    let base = payload as usize;
    for index in 0..count / 2 {
        let left = base + index * 16;
        let right = base + (count - 1 - index) * 16;
        let mut temp = [0u8; 16];
        temp.copy_from_slice(&memory[left..left + 16]);
        memory.copy_within(right..right + 16, left);
        memory[right..right + 16].copy_from_slice(&temp);
    }
    Ok(())
}

fn dyn_sum_cells(memory: &mut [u8], next_dyn: &mut u32, cell: usize) -> Result<usize, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 5 {
        return Err("sum() on a dynamic value needs a dynamic array".to_string());
    }
    let count = aux as usize;
    let base = payload as usize;
    let mut total = 0.0;
    let mut all_int = true;
    for index in 0..count {
        let item = base + index * 16;
        let (item_tag, _, _) = dyn_tag(memory, item);
        if item_tag == 4 || item_tag >= 5 {
            return Err("sum() needs numbers".to_string());
        }
        if item_tag == 2 {
            all_int = false;
        }
        total += dyn_number(memory, item)?;
    }
    let result = alloc_dyn_cell(next_dyn)?;
    if all_int {
        write_dyn_cell(memory, result, 1, 0, total as i64 as u64);
    } else {
        write_dyn_cell(memory, result, 2, 0, total.to_bits());
    }
    Ok(result)
}

fn dyn_minmax_cells(
    memory: &mut [u8],
    next_dyn: &mut u32,
    cell: usize,
    want_min: bool,
) -> Result<usize, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 5 {
        return Err("min()/max() on a dynamic value needs a dynamic array".to_string());
    }
    let count = aux as usize;
    if count == 0 {
        return Err("min()/max() of an empty array".to_string());
    }
    let base = payload as usize;
    let mut best = base;
    for index in 1..count {
        let item = base + index * 16;
        let order = dyn_order(memory, item, best)?;
        let better = if want_min {
            order == std::cmp::Ordering::Less
        } else {
            order == std::cmp::Ordering::Greater
        };
        if better {
            best = item;
        }
    }
    let result = alloc_dyn_cell(next_dyn)?;
    memory.copy_within(best..best + 16, result);
    Ok(result)
}

fn dyn_join_cells(
    memory: &mut [u8],
    next_dyn: &mut u32,
    next_file: &mut u32,
    cell: usize,
    sep_ptr: usize,
    sep_len: usize,
) -> Result<i32, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 5 {
        return Err("join() on a dynamic value needs a dynamic array".to_string());
    }
    let sep = memory_text(memory, sep_ptr, sep_len);
    let count = aux as usize;
    let base = payload as usize;
    let mut pieces = Vec::with_capacity(count);
    for index in 0..count {
        pieces.push(format_dyn_cell(memory, base + index * 16, false)?);
    }
    let joined = pieces.join(&sep);
    let _ = next_dyn;
    store_text_block(memory, next_file, joined.as_bytes())
        .map(|start| start as i32)
        .ok_or_else(|| "dyn_join result does not fit the file region".to_string())
}
/// Dynamic for-loop key: boxed int index for arrays, boxed string key for objects.
fn dyn_key_at_cell(
    memory: &mut [u8],
    next_dyn: &mut u32,
    null_slot: &mut u32,
    cell: usize,
    index: i64,
) -> Result<usize, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag == 5 {
        let count = aux as i64;
        if index < 0 || index >= count {
            return dyn_null_cell(memory, next_dyn, null_slot);
        }
        let boxed = alloc_dyn_cell(next_dyn)?;
        write_dyn_cell(memory, boxed, 1, 0, index as u64);
        return Ok(boxed);
    }
    if tag != 6 {
        return Err("dynamic for-loops need a dynamic array or object".to_string());
    }
    let count = aux as i64;
    if index < 0 || index >= count {
        return dyn_null_cell(memory, next_dyn, null_slot);
    }
    let entry = payload as usize + index as usize * 12;
    if entry + 12 > memory.len() {
        return dyn_null_cell(memory, next_dyn, null_slot);
    }
    let key_ptr = u32::from_le_bytes(memory[entry..entry + 4].try_into().unwrap());
    let key_len = u32::from_le_bytes(memory[entry + 4..entry + 8].try_into().unwrap());
    let boxed = alloc_dyn_cell(next_dyn)?;
    write_dyn_cell(memory, boxed, 4, key_len, u64::from(key_ptr));
    Ok(boxed)
}
/// Dynamic for-loop element: array elements, or object keys (the VM's single
/// loop variable yields keys for objects).
fn dyn_iter_at_cell(
    memory: &mut [u8],
    next_dyn: &mut u32,
    null_slot: &mut u32,
    cell: usize,
    index: i64,
) -> Result<usize, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag == 5 {
        return dyn_index_cell(memory, next_dyn, null_slot, cell, index);
    }
    if tag != 6 {
        return Err("dynamic for-loops need a dynamic array or object".to_string());
    }
    let count = aux as i64;
    if index < 0 || index >= count {
        return dyn_null_cell(memory, next_dyn, null_slot);
    }
    let entry = payload as usize + index as usize * 12;
    if entry + 12 > memory.len() {
        return dyn_null_cell(memory, next_dyn, null_slot);
    }
    let key_ptr = u32::from_le_bytes(memory[entry..entry + 4].try_into().unwrap());
    let key_len = u32::from_le_bytes(memory[entry + 4..entry + 8].try_into().unwrap());
    let boxed = alloc_dyn_cell(next_dyn)?;
    write_dyn_cell(memory, boxed, 4, key_len, u64::from(key_ptr));
    Ok(boxed)
}
fn dyn_cmp_cells(memory: &[u8], a: usize, b: usize) -> Result<i64, String> {
    let text_a = dyn_text(memory, a);
    let text_b = dyn_text(memory, b);
    match (text_a, text_b) {
        (Some((len_a, ptr_a)), Some((len_b, ptr_b))) => {
            let left = memory_text(memory, ptr_a as usize, len_a as usize);
            let right = memory_text(memory, ptr_b as usize, len_b as usize);
            Ok(match left.cmp(&right) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            })
        }
        (None, None) => {
            let left = dyn_number(memory, a)?;
            let right = dyn_number(memory, b)?;
            Ok(if left < right {
                -1
            } else if left > right {
                1
            } else {
                0
            })
        }
        _ => Err("cannot compare a dynamic string with a number".to_string()),
    }
}

fn dyn_arith_cells(
    memory: &mut [u8],
    next_dyn: &mut u32,
    next_file: &mut u32,
    op: u32,
    a: usize,
    b: usize,
) -> Result<usize, String> {
    let (tag_a, _, _) = dyn_tag(memory, a);
    let (tag_b, _, _) = dyn_tag(memory, b);
    if op == 0 && tag_a == 5 && tag_b == 5 {
        return dyn_concat_cells(memory, next_dyn, a, b);
    }
    let text_a = dyn_text(memory, a);
    let text_b = dyn_text(memory, b);
    if op == 0 && (text_a.is_some() || text_b.is_some()) {
        let left = format_dyn_cell(memory, a, false)?;
        let right = format_dyn_cell(memory, b, false)?;
        let joined = format!("{left}{right}");
        let cell = alloc_dyn_cell(next_dyn)?;
        let start = store_text_block(memory, next_file, joined.as_bytes())
            .ok_or_else(|| "dynamic string does not fit the file region".to_string())?;
        let len = u32::from_le_bytes(
            memory[start as usize..start as usize + 4]
                .try_into()
                .unwrap(),
        );
        write_dyn_cell(memory, cell, 4, len, u64::from(start) + 4);
        return Ok(cell);
    }
    let left = dyn_number(memory, a)?;
    let right = dyn_number(memory, b)?;
    let both_int =
        matches!(dyn_tag(memory, a).0, 0..=1 | 3) && matches!(dyn_tag(memory, b).0, 0..=1 | 3);
    let cell = alloc_dyn_cell(next_dyn)?;
    match op {
        0..=2 if both_int => {
            let value = match op {
                0 => left as i64 + right as i64,
                1 => left as i64 - right as i64,
                _ => left as i64 * right as i64,
            };
            write_dyn_cell(memory, cell, 1, 0, value as u64);
        }
        0 => write_dyn_cell(memory, cell, 2, 0, (left + right).to_bits()),
        1 => write_dyn_cell(memory, cell, 2, 0, (left - right).to_bits()),
        2 => write_dyn_cell(memory, cell, 2, 0, (left * right).to_bits()),
        3 => {
            if right == 0.0 {
                return Err("division by zero".to_string());
            }
            write_dyn_cell(memory, cell, 2, 0, (left / right).to_bits());
        }
        4 => {
            if right == 0.0 {
                return Err("modulo by zero".to_string());
            }
            if both_int {
                write_dyn_cell(memory, cell, 1, 0, ((left as i64) % (right as i64)) as u64);
            } else {
                write_dyn_cell(memory, cell, 2, 0, (left % right).to_bits());
            }
        }
        _ => return Err("unknown dynamic arithmetic op".to_string()),
    }
    Ok(cell)
}

fn dyn_to_int_cell(memory: &[u8], cell: usize) -> Result<i64, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    Ok(match tag {
        0 => 0,
        1 => payload as i64,
        2 => f64::from_bits(payload) as i64,
        3 => i64::from(payload != 0),
        4 => memory_text(memory, payload as usize, aux as usize)
            .trim()
            .parse::<i64>()
            .map_err(|_| "cannot convert string to int".to_string())?,
        _ => return Err("cannot convert dynamic value to int".to_string()),
    })
}

fn dyn_to_float_cell(memory: &[u8], cell: usize) -> Result<f64, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    Ok(match tag {
        0 => 0.0,
        1 => payload as i64 as f64,
        2 => f64::from_bits(payload),
        3 => {
            if payload != 0 {
                1.0
            } else {
                0.0
            }
        }
        4 => memory_text(memory, payload as usize, aux as usize)
            .trim()
            .parse::<f64>()
            .map_err(|_| "cannot convert string to float".to_string())?,
        _ => return Err("cannot convert dynamic value to float".to_string()),
    })
}

fn dyn_truthy_cell(memory: &[u8], cell: usize) -> Result<bool, String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    Ok(match tag {
        0 => false,
        1 => payload as i64 != 0,
        2 => f64::from_bits(payload) != 0.0,
        3 => payload != 0,
        4 => aux != 0,
        5 | 6 => aux != 0,
        _ => return Err("cannot test a dynamic value for truthiness".to_string()),
    })
}
/// Allocate a dynamic array cell with `capacity` zeroed element cells.
fn dyn_array_alloc(
    memory: &mut [u8],
    next_dyn: &mut u32,
    capacity: usize,
) -> Result<usize, String> {
    let base = alloc_dyn_bytes(next_dyn, capacity.max(1) * 16)?;
    for index in 0..capacity.max(1) {
        write_dyn_cell(memory, base + index * 16, 0, 0, 0);
    }
    let cell = alloc_dyn_cell(next_dyn)?;
    write_dyn_cell(memory, cell, 5, 0, base as u64);
    Ok(cell)
}

fn dyn_array_push(memory: &mut [u8], cell: usize, value: usize) -> Result<(), String> {
    let (tag, aux, payload) = dyn_tag(memory, cell);
    if tag != 5 {
        return Err("dynamic array push needs a dynamic array".to_string());
    }
    let slot = payload as usize + aux as usize * 16;
    if slot + 16 > memory.len() {
        return Err("dynamic array push out of bounds".to_string());
    }
    memory.copy_within(value..value + 16, slot);
    memory[cell + 4..cell + 8].copy_from_slice(&(aux + 1).to_le_bytes());
    Ok(())
}
/// Store a `[status][len][data]` block for the soft file imports: status 0 is
/// success, 1 carries an error message in `data`.
fn store_soft_block(
    memory: &mut [u8],
    next_file: &mut u32,
    status: u32,
    data: &[u8],
) -> Result<i32, String> {
    let size = 8 + data.len();
    let start = *next_file as usize;
    if start + size > memory.len() {
        return Err("soft file result does not fit the file region".to_string());
    }
    memory[start..start + 4].copy_from_slice(&status.to_le_bytes());
    memory[start + 4..start + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
    memory[start + 8..start + size].copy_from_slice(data);
    *next_file = (start + size + 7) as u32 & !7;
    Ok(start as i32)
}

/// Soft (`attempt`-visible) exec failure: return a result block whose `flags`
/// bit 1 marks a spawn error, with the message in the `err` fields.
fn exec_error_block(
    memory: &mut [u8],
    next_proc: &mut u32,
    message: &str,
) -> Result<Vec<WVal>, String> {
    let bytes = message.as_bytes();
    let size = 28 + bytes.len();
    let start = *next_proc as usize;
    if start + size > memory.len() {
        return Err("exec error does not fit the result region".to_string());
    }
    let err_ptr = start + 28;
    memory[start..start + 4].copy_from_slice(&127i32.to_le_bytes());
    memory[start + 4..start + 8].copy_from_slice(&0u32.to_le_bytes());
    memory[start + 8..start + 12].copy_from_slice(&0u32.to_le_bytes());
    memory[start + 12..start + 16].copy_from_slice(&(err_ptr as u32).to_le_bytes());
    memory[start + 16..start + 20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    memory[start + 20..start + 24].copy_from_slice(&0u32.to_le_bytes());
    memory[start + 24..start + 28].copy_from_slice(&2u32.to_le_bytes());
    memory[err_ptr..err_ptr + bytes.len()].copy_from_slice(bytes);
    *next_proc = (start + size + 7) as u32 & !7;
    Ok(vec![WVal::I32(start as i32)])
}

/// Last path component; the VM treats both `/` and `\` as separators and
/// trims trailing separators first.
fn path_basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(index) => &trimmed[index + 1..],
        None => trimmed,
    }
}

/// Parent path with the VM's rules: trim trailing separators, cut at the last
/// separator, keep a bare root (for example `dirname("/a") == "/"`).
fn path_dirname(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match trimmed.rfind(['/', '\\']) {
        Some(index) => {
            let parent = &trimmed[..index];
            let parent = parent.trim_end_matches(['/', '\\']);
            if parent.is_empty() {
                &trimmed[..1]
            } else {
                parent
            }
        }
        None => "",
    }
}

fn shell_command(line: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut command = std::process::Command::new("cmd");
        command.arg("/C").arg(line);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg(line);
        command
    }
}

/// Deterministic software framebuffer behind the `webgl.*` imports.
struct GlBackend {
    width: u32,
    height: u32,
    clear: [f64; 4],
    pixels: Vec<u8>,
}

impl GlBackend {
    fn new() -> GlBackend {
        GlBackend {
            width: 0,
            height: 0,
            clear: [0.0, 0.0, 0.0, 1.0],
            pixels: Vec::new(),
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.clamp(1, 4096);
        self.height = height.clamp(1, 4096);
        self.pixels = vec![0; self.width as usize * self.height as usize * 4];
    }

    fn clear(&mut self) {
        let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        let fill = [
            channel(self.clear[0]),
            channel(self.clear[1]),
            channel(self.clear[2]),
            channel(self.clear[3]),
        ];
        for pixel in self.pixels.chunks_mut(4) {
            pixel.copy_from_slice(&fill);
        }
    }

    fn pixel(&self, x: u32, y: u32) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        let r = self.pixels[offset] as u32;
        let g = self.pixels[offset + 1] as u32;
        let b = self.pixels[offset + 2] as u32;
        let a = self.pixels[offset + 3] as u32;
        (r << 24) | (g << 16) | (b << 8) | a
    }
}

pub(crate) struct PrintHost<'a, F: FnMut(i64), G: FnMut(&str)> {
    on_i64: &'a mut F,
    on_str: &'a mut G,
    /// `env.eprint` sink (stderr); cloned lines include the trailing newline.
    on_err: BoxedStrSink,
    /// Directory of the executed module, exposed as `bx.script_dir` (`__dir__`).
    script_dir: Option<String>,
    /// Extra environment variables applied to the next `exec`/`sh` launch.
    exec_env: Vec<(String, String)>,
    /// Bump allocator for the prototype `env.alloc` import, 8-byte aligned.
    next_alloc: u32,
    /// Command-line arguments exposed through `bx.argc` / `bx.arg`.
    argv: Vec<String>,
    /// argv strings are written into the second memory page, [len u32][utf8].
    next_argv: u32,
    /// File contents returned by `bx.read_file` live in the third page.
    next_file: u32,
    /// Dynamic map blocks live in their own region (page 5 onward).
    next_map: u32,
    /// `bx.capture` result blocks live in the fourth page.
    next_proc: u32,
    /// Dynamic (P2a/P2b) 16-byte value cells live between the capture and map regions.
    next_dyn: u32,
    /// Lazily allocated dynamic null cell (index/member misses return it).
    dyn_null: u32,
    /// Clock origin for `web.performance_now`.
    started: std::time::Instant,
    /// Software framebuffer for the `webgl.*` namespace.
    gl: GlBackend,
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
            ("bx", "script_dir") => {
                let text = self.script_dir.clone().unwrap_or_default();
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("bx.script_dir result does not fit the file region".to_string()),
                }
            }
            ("env", "eprint") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.eprint expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default();
                let mut line = text;
                line.push('\n');
                (self.on_err)(&line);
                Ok(vec![])
            }
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
            ("bx", "argc") => Ok(vec![WVal::I32(self.argv.len() as i32)]),
            ("env", "print_f64") => {
                let value = match args.first() {
                    Some(WVal::F64(value)) => *value,
                    _ => return Err("env.print_f64 expects one f64".to_string()),
                };
                // Match the VM: finite floats use fmt_float, others print null.
                let text = if value.is_finite() {
                    crate::value::fmt_float(value)
                } else {
                    "null".to_string()
                };
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "arr_range") => {
                let (start, stop, step) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I64(start)), Some(WVal::I64(stop)), Some(WVal::I64(step))) => {
                        (*start, *stop, *step)
                    }
                    _ => {
                        return Err(
                            "env.arr_range expects (i64 start, i64 stop, i64 step)".to_string()
                        )
                    }
                };
                let mut values: Vec<i64> = Vec::new();
                if step > 0 {
                    let mut value = start;
                    while value < stop {
                        values.push(value);
                        value += step;
                    }
                }
                let size = 4 + 8 * values.len();
                let at = self.next_file as usize;
                if at + size > memory.len() {
                    return Err("range result does not fit the memory region".to_string());
                }
                memory[at..at + 4].copy_from_slice(&(values.len() as u32).to_le_bytes());
                for (index, value) in values.iter().enumerate() {
                    let offset = at + 4 + index * 8;
                    memory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
                }
                self.next_file = (at + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(at as i32)])
            }
            ("env", "arr_sum") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_sum expects one i32".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_sum array is outside memory".to_string());
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                let mut total: i64 = 0;
                for index in 0..count {
                    let offset = ptr + 4 + index * 8;
                    if offset + 8 > memory.len() {
                        break;
                    }
                    total += i64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
                }
                Ok(vec![WVal::I64(total)])
            }
            ("env", "arr_reverse") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_reverse expects one i32".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_reverse array is outside memory".to_string());
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                // The VM reverses in place, so keep the same array identity.
                for index in 0..count / 2 {
                    let left = ptr + 4 + index * 8;
                    let right = ptr + 4 + (count - 1 - index) * 8;
                    if right + 8 > memory.len() {
                        break;
                    }
                    let a = memory[left..left + 8].to_vec();
                    let b = memory[right..right + 8].to_vec();
                    memory[left..left + 8].copy_from_slice(&b);
                    memory[right..right + 8].copy_from_slice(&a);
                }
                Ok(vec![WVal::I32(ptr as i32)])
            }
            ("env", "print_arr_i64") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_arr_i64 expects one i32".to_string()),
                };
                let text = format_i64_array(memory, ptr);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "print_arr_str") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_arr_str expects one i32".to_string()),
                };
                let text = format_str_array(memory, ptr);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "print_record") => {
                let (table, count, rec) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(t)), Some(WVal::I32(c)), Some(WVal::I32(r))) => {
                        (*t as usize, *c as usize, *r as usize)
                    }
                    _ => return Err("env.print_record expects three i32".to_string()),
                };
                let text = format_record(memory, table, count, rec);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "print_arr_obj") => {
                let (table, count, arr) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(t)), Some(WVal::I32(c)), Some(WVal::I32(a))) => {
                        (*t as usize, *c as usize, *a as usize)
                    }
                    _ => return Err("env.print_arr_obj expects three i32".to_string()),
                };
                let mut pieces: Vec<String> = Vec::new();
                if arr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[arr..arr + 4].try_into().unwrap()) as usize;
                    for index in 0..len {
                        let offset = arr + 4 + index * 8;
                        if offset + 4 > memory.len() {
                            break;
                        }
                        let rec = u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap())
                            as usize;
                        pieces.push(format_record(memory, table, count, rec));
                    }
                }
                (self.on_str)(&format!("[{}]", pieces.join(",")));
                Ok(Vec::new())
            }
            ("env", "arr_index_of") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_index_of expects (i32 ptr, i64 value)".to_string()),
                };
                let needle = match args.get(1) {
                    Some(WVal::I64(value)) => *value,
                    _ => return Err("env.arr_index_of expects (i32 ptr, i64 value)".to_string()),
                };
                let index = if ptr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                    let mut found = -1i64;
                    for at in 0..len {
                        let offset = ptr + 4 + at * 8;
                        if offset + 8 > memory.len() {
                            break;
                        }
                        let value =
                            i64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
                        if value == needle {
                            found = at as i64;
                            break;
                        }
                    }
                    found
                } else {
                    -1
                };
                Ok(vec![WVal::I64(index)])
            }
            ("env", "arr_index_of_str") => {
                let (ptr, key_ptr, key_len) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(p)), Some(WVal::I32(kp)), Some(WVal::I32(kl))) => {
                        (*p as usize, *kp as usize, *kl as usize)
                    }
                    _ => {
                        return Err(
                            "env.arr_index_of_str expects (i32 ptr, i32 key, i32 len)".to_string()
                        )
                    }
                };
                let needle = memory_text(memory, key_ptr, key_len);
                let index = if ptr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                    let mut found = -1i64;
                    for at in 0..len {
                        let offset = ptr + 4 + at * 8;
                        if offset + 8 > memory.len() {
                            break;
                        }
                        let data =
                            u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap())
                                as usize;
                        let size =
                            u32::from_le_bytes(memory[offset + 4..offset + 8].try_into().unwrap())
                                as usize;
                        if memory_text(memory, data, size) == needle {
                            found = at as i64;
                            break;
                        }
                    }
                    found
                } else {
                    -1
                };
                Ok(vec![WVal::I64(index)])
            }
            ("env", "arr_minmax") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_minmax expects (i32 ptr, i32 want_min)".to_string()),
                };
                let want_min = match args.get(1) {
                    Some(WVal::I32(value)) => *value != 0,
                    _ => return Err("env.arr_minmax expects (i32 ptr, i32 want_min)".to_string()),
                };
                let mut best: Option<i64> = None;
                if ptr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                    for at in 0..len {
                        let offset = ptr + 4 + at * 8;
                        if offset + 8 > memory.len() {
                            break;
                        }
                        let value =
                            i64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
                        best = Some(match best {
                            None => value,
                            Some(current) => {
                                if want_min {
                                    current.min(value)
                                } else {
                                    current.max(value)
                                }
                            }
                        });
                    }
                }
                Ok(vec![WVal::I64(best.unwrap_or(0))])
            }
            ("env", "arr_minmax_str") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => {
                        return Err("env.arr_minmax_str expects (i32 ptr, i32 want_min)".to_string())
                    }
                };
                let want_min = match args.get(1) {
                    Some(WVal::I32(value)) => *value != 0,
                    _ => {
                        return Err("env.arr_minmax_str expects (i32 ptr, i32 want_min)".to_string())
                    }
                };
                let mut best: Option<String> = None;
                if ptr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                    for at in 0..len {
                        let offset = ptr + 4 + at * 8;
                        if offset + 8 > memory.len() {
                            break;
                        }
                        let data =
                            u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap())
                                as usize;
                        let size =
                            u32::from_le_bytes(memory[offset + 4..offset + 8].try_into().unwrap())
                                as usize;
                        let text = memory_text(memory, data, size);
                        best = Some(match best {
                            None => text,
                            Some(current) => {
                                if (want_min && text < current) || (!want_min && text > current) {
                                    text
                                } else {
                                    current
                                }
                            }
                        });
                    }
                }
                let text = best.unwrap_or_default();
                let at = self.next_file as usize;
                if at + 4 + text.len() > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[at..at + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                memory[at + 4..at + 4 + text.len()].copy_from_slice(text.as_bytes());
                self.next_file = ((at + 4 + text.len() + 7) & !7) as u32;
                Ok(vec![WVal::I32(at as i32)])
            }
            ("env", "print_arr_strarr") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_arr_strarr expects one i32".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.print_arr_strarr array is outside memory".to_string());
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                let mut rows: Vec<String> = Vec::new();
                for index in 0..count {
                    let offset = ptr + 4 + index * 8;
                    if offset + 4 > memory.len() {
                        break;
                    }
                    let inner =
                        u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap()) as usize;
                    rows.push(format_str_array(memory, inner));
                }
                (self.on_str)(&format!("[{}]", rows.join(",")));
                Ok(Vec::new())
            }
            ("env", "exit") => {
                let code = match args.first() {
                    Some(WVal::I32(code)) => *code as i64,
                    _ => return Err("env.exit expects one i32".to_string()),
                };
                Err(format!("{}{}", crate::wasm_core::EXIT_MARKER, code))
            }
            ("env", "error") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.error expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.error message is outside memory".to_string())?;
                Err(text)
            }
            ("env", "str_to_float") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.str_to_float expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_to_float string is outside memory".to_string())?;
                let value = text
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "cannot convert str to float".to_string())?;
                Ok(vec![WVal::F64(value)])
            }
            ("env", "print_record_typed") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(v)) => Some(*v as usize),
                    _ => None,
                };
                let (kinds, table, count, rec) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(kinds), Some(table), Some(count), Some(rec)) => {
                        (kinds, table, count, rec)
                    }
                    _ => {
                        return Err(
                            "env.print_record_typed expects (kinds, table, count, rec)".to_string()
                        )
                    }
                };
                let text = format_record_typed(memory, kinds, table, count, rec);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "print_arr_obj_typed") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(v)) => Some(*v as usize),
                    _ => None,
                };
                let (kinds, table, count_arr, arr) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(kinds), Some(table), Some(count_arr), Some(arr)) => {
                        (kinds, table, count_arr, arr)
                    }
                    _ => {
                        return Err("env.print_arr_obj_typed expects (kinds, table, count, arr)"
                            .to_string())
                    }
                };
                let mut pieces: Vec<String> = Vec::new();
                if arr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[arr..arr + 4].try_into().unwrap()) as usize;
                    for index in 0..len {
                        let slot = arr + 4 + index * 8;
                        if slot + 4 > memory.len() {
                            break;
                        }
                        let rec =
                            u32::from_le_bytes(memory[slot..slot + 4].try_into().unwrap()) as usize;
                        pieces.push(format_record_typed(memory, kinds, table, count_arr, rec));
                    }
                }
                (self.on_str)(&format!("[{}]", pieces.join(",")));
                Ok(Vec::new())
            }
            ("env", "str_len") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.str_len expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_len string is outside memory".to_string())?;
                Ok(vec![WVal::I64(text.chars().count() as i64)])
            }
            ("env", "print_arr_f64") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_arr_f64 expects one i32".to_string()),
                };
                let text = format_f64_array(memory, ptr);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "join_bool") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(v)) => Some(*v as usize),
                    _ => None,
                };
                let (ptr, sep_ptr, sep_len) =
                    match (read(args.first()), read(args.get(1)), read(args.get(2))) {
                        (Some(ptr), Some(sep_ptr), Some(sep_len)) => (ptr, sep_ptr, sep_len),
                        _ => return Err("env.join_bool expects three i32s".to_string()),
                    };
                let text = join_bool_array(memory, ptr, sep_ptr, sep_len);
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("join_bool result does not fit the file region".to_string()),
                }
            }
            ("env", "print_arr_bool") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_arr_bool expects one i32".to_string()),
                };
                let text = format_bool_array(memory, ptr);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "arr_sum_f") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_sum_f expects one i32".to_string()),
                };
                let mut total = 0.0f64;
                if ptr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                    for index in 0..len {
                        let offset = ptr + 4 + index * 8;
                        if offset + 8 > memory.len() {
                            break;
                        }
                        total += f64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
                    }
                }
                Ok(vec![WVal::F64(total)])
            }
            ("env", "arr_minmax_f") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_minmax_f expects (i32 ptr, i32 want_min)".to_string()),
                };
                let want_min = match args.get(1) {
                    Some(WVal::I32(value)) => *value != 0,
                    _ => return Err("env.arr_minmax_f expects (i32 ptr, i32 want_min)".to_string()),
                };
                let mut best: Option<f64> = None;
                if ptr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                    for index in 0..len {
                        let offset = ptr + 4 + index * 8;
                        if offset + 8 > memory.len() {
                            break;
                        }
                        let value =
                            f64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
                        best = Some(match best {
                            None => value,
                            Some(current) => {
                                if want_min {
                                    current.min(value)
                                } else {
                                    current.max(value)
                                }
                            }
                        });
                    }
                }
                Ok(vec![WVal::F64(best.unwrap_or(0.0))])
            }
            ("env", "arr_index_of_f") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_index_of_f expects (i32 ptr, f64 value)".to_string()),
                };
                let needle = match args.get(1) {
                    Some(WVal::F64(value)) => *value,
                    _ => return Err("env.arr_index_of_f expects (i32 ptr, f64 value)".to_string()),
                };
                let mut found = -1i64;
                if ptr + 4 <= memory.len() {
                    let len = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                    for index in 0..len {
                        let offset = ptr + 4 + index * 8;
                        if offset + 8 > memory.len() {
                            break;
                        }
                        let value =
                            f64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
                        if value == needle {
                            found = index as i64;
                            break;
                        }
                    }
                }
                Ok(vec![WVal::I64(found)])
            }
            ("env", "append_file") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(v)) => Some(*v as usize),
                    _ => None,
                };
                let (pp, pl, dp, dl) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(pp), Some(pl), Some(dp), Some(dl)) => (pp, pl, dp, dl),
                    _ => return Err("env.append_file expects four i32s".to_string()),
                };
                let path = memory_text(memory, pp, pl);
                let data = memory
                    .get(dp..dp.saturating_add(dl))
                    .ok_or_else(|| "env.append_file data is outside memory".to_string())?;
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut file| std::io::Write::write_all(&mut file, data))
                {
                    Ok(()) => Ok(vec![WVal::I32(0)]),
                    Err(error) => Err(format!("cannot append '{}': {}", path, error)),
                }
            }
            ("env", "remove_file") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.remove_file expects (i32 ptr, i32 len)".to_string()),
                };
                let path = memory_text(memory, ptr, len);
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(vec![WVal::I32(0)]),
                    Err(error) => Err(format!("cannot remove '{}': {}", path, error)),
                }
            }
            ("env", "dir_count") | ("env", "dir_entry") => {
                let is_count = name == "dir_count";
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("dir_count/dir_entry expect (i32 ptr, i32 len)".to_string()),
                };
                let path = memory_text(memory, ptr, len);
                let mut names: Vec<String> = match std::fs::read_dir(&path) {
                    Ok(entries) => entries
                        .filter_map(|entry| entry.ok())
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect(),
                    Err(error) => return Err(format!("cannot list '{}': {}", path, error)),
                };
                names.sort();
                if is_count {
                    return Ok(vec![WVal::I32(names.len() as i32)]);
                }
                let index = match args.get(2) {
                    Some(WVal::I32(index)) => *index as usize,
                    _ => {
                        return Err(
                            "env.dir_entry expects (i32 ptr, i32 len, i32 index)".to_string()
                        )
                    }
                };
                let text = names.get(index).cloned().unwrap_or_default();
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("dir_entry result does not fit the file region".to_string()),
                }
            }
            ("bx", "exec_env_clear") => {
                self.exec_env.clear();
                Ok(Vec::new())
            }
            ("bx", "exec_env_add") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(v)) => Some(*v as usize),
                    _ => None,
                };
                let (kp, kl, vp, vl) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(kp), Some(kl), Some(vp), Some(vl)) => (kp, kl, vp, vl),
                    _ => return Err("bx.exec_env_add expects four i32s".to_string()),
                };
                let key = memory_text(memory, kp, kl);
                let value = memory_text(memory, vp, vl);
                self.exec_env.push((key, value));
                Ok(Vec::new())
            }
            ("env", "format_container") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(v)) => Some(*v as usize),
                    _ => None,
                };
                let (kind, ptr, aux) =
                    match (read(args.first()), read(args.get(1)), read(args.get(2))) {
                        (Some(kind), Some(ptr), Some(aux)) => (kind as i32, ptr, aux),
                        _ => {
                            return Err("env.format_container expects (i32 kind, i32 ptr, i32 aux)"
                                .to_string())
                        }
                    };
                let text = format_container(memory, kind, ptr, aux);
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("format_container result does not fit the file region".to_string()),
                }
            }
            ("env", "json_str") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.json_str expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory_text(memory, ptr, len);
                let mut rendered = String::new();
                crate::value::write_json_str(&mut rendered, &text);
                match store_text_block(memory, &mut self.next_file, rendered.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("json_str result does not fit the file region".to_string()),
                }
            }
            ("env", "dyn_eq") => {
                let (a, b) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(a)), Some(WVal::I32(b))) => (*a as usize, *b as usize),
                    _ => return Err("env.dyn_eq expects (i32, i32)".to_string()),
                };
                let equal = dyn_eq_cells(memory, a, b)?;
                Ok(vec![WVal::I32(i32::from(equal))])
            }
            ("env", "dyn_cmp") => {
                let (a, b) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(a)), Some(WVal::I32(b))) => (*a as usize, *b as usize),
                    _ => return Err("env.dyn_cmp expects (i32, i32)".to_string()),
                };
                let order = dyn_cmp_cells(memory, a, b)?;
                Ok(vec![WVal::I64(order)])
            }
            ("env", "dyn_arith") => {
                let (op, a, b) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(op)), Some(WVal::I32(a)), Some(WVal::I32(b))) => {
                        (*op as u32, *a as usize, *b as usize)
                    }
                    _ => return Err("env.dyn_arith expects (i32 op, i32, i32)".to_string()),
                };
                let cell =
                    dyn_arith_cells(memory, &mut self.next_dyn, &mut self.next_file, op, a, b)?;
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_neg") => {
                let cell_in = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_neg expects one i32".to_string()),
                };
                let (tag, _, payload) = dyn_tag(memory, cell_in);
                let cell = alloc_dyn_cell(&mut self.next_dyn)?;
                match tag {
                    0 | 1 | 3 => {
                        write_dyn_cell(memory, cell, 1, 0, (0i64 - payload as i64) as u64);
                    }
                    2 => write_dyn_cell(memory, cell, 2, 0, (-f64::from_bits(payload)).to_bits()),
                    _ => return Err("cannot negate a dynamic string".to_string()),
                }
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_to_int") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_to_int expects one i32".to_string()),
                };
                Ok(vec![WVal::I64(dyn_to_int_cell(memory, cell)?)])
            }
            ("env", "dyn_to_float") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_to_float expects one i32".to_string()),
                };
                Ok(vec![WVal::F64(dyn_to_float_cell(memory, cell)?)])
            }
            ("env", "dyn_truthy") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_truthy expects one i32".to_string()),
                };
                Ok(vec![WVal::I32(i32::from(dyn_truthy_cell(memory, cell)?))])
            }
            ("env", "dyn_len") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_len expects one i32".to_string()),
                };
                Ok(vec![WVal::I64(dyn_len_cell(memory, cell)?)])
            }
            ("env", "dyn_index") => {
                let (cell, index) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I64(index))) => (*cell as usize, *index),
                    _ => return Err("env.dyn_index expects (i32, i64)".to_string()),
                };
                let slot =
                    dyn_index_cell(memory, &mut self.next_dyn, &mut self.dyn_null, cell, index)?;
                Ok(vec![WVal::I32(slot as i32)])
            }
            ("env", "dyn_member") => {
                let (cell, ptr, len) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(cell)), Some(WVal::I32(ptr)), Some(WVal::I32(len))) => {
                        (*cell as usize, *ptr as usize, *len as usize)
                    }
                    _ => return Err("env.dyn_member expects (i32, i32, i32)".to_string()),
                };
                let key = memory_text(memory, ptr, len);
                let slot =
                    dyn_member_cell(memory, &mut self.next_dyn, &mut self.dyn_null, cell, &key)?;
                Ok(vec![WVal::I32(slot as i32)])
            }
            ("env", "dyn_value_at") => {
                let (cell, index) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I64(index))) => (*cell as usize, *index),
                    _ => return Err("env.dyn_value_at expects (i32, i64)".to_string()),
                };
                let slot =
                    dyn_value_at_cell(memory, &mut self.next_dyn, &mut self.dyn_null, cell, index)?;
                Ok(vec![WVal::I32(slot as i32)])
            }
            ("env", "dyn_key_at") => {
                let (cell, index) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I64(index))) => (*cell as usize, *index),
                    _ => return Err("env.dyn_key_at expects (i32, i64)".to_string()),
                };
                let slot =
                    dyn_key_at_cell(memory, &mut self.next_dyn, &mut self.dyn_null, cell, index)?;
                Ok(vec![WVal::I32(slot as i32)])
            }
            ("env", "dyn_iter_at") => {
                let (cell, index) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I64(index))) => (*cell as usize, *index),
                    _ => return Err("env.dyn_iter_at expects (i32, i64)".to_string()),
                };
                let slot =
                    dyn_iter_at_cell(memory, &mut self.next_dyn, &mut self.dyn_null, cell, index)?;
                Ok(vec![WVal::I32(slot as i32)])
            }
            ("env", "dyn_array_alloc") => {
                let capacity = match args.first() {
                    Some(WVal::I32(capacity)) => *capacity as usize,
                    _ => return Err("env.dyn_array_alloc expects one i32".to_string()),
                };
                let cell = dyn_array_alloc(memory, &mut self.next_dyn, capacity)?;
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_array_push") => {
                let (cell, value) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I32(value))) => {
                        (*cell as usize, *value as usize)
                    }
                    _ => return Err("env.dyn_array_push expects (i32, i32)".to_string()),
                };
                dyn_array_push(memory, cell, value)?;
                Ok(vec![])
            }
            ("env", "dyn_sort") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_sort expects one i32".to_string()),
                };
                dyn_sort_cells(memory, cell)?;
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_reverse") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_reverse expects one i32".to_string()),
                };
                dyn_reverse_cells(memory, cell)?;
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_sum") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_sum expects one i32".to_string()),
                };
                let result = dyn_sum_cells(memory, &mut self.next_dyn, cell)?;
                Ok(vec![WVal::I32(result as i32)])
            }
            ("env", "dyn_minmax") => {
                let (cell, want_min) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I32(want_min))) => {
                        (*cell as usize, *want_min != 0)
                    }
                    _ => return Err("env.dyn_minmax expects (i32, i32)".to_string()),
                };
                let result = dyn_minmax_cells(memory, &mut self.next_dyn, cell, want_min)?;
                Ok(vec![WVal::I32(result as i32)])
            }
            ("env", "dyn_join") => {
                let (cell, sep_ptr, sep_len) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(cell)), Some(WVal::I32(ptr)), Some(WVal::I32(len))) => {
                        (*cell as usize, *ptr as usize, *len as usize)
                    }
                    _ => return Err("env.dyn_join expects (i32, i32, i32)".to_string()),
                };
                let start = dyn_join_cells(
                    memory,
                    &mut self.next_dyn,
                    &mut self.next_file,
                    cell,
                    sep_ptr,
                    sep_len,
                )?;
                Ok(vec![WVal::I32(start)])
            }
            ("env", "dyn_from_array") => {
                let (kind, ptr) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(kind)), Some(WVal::I32(ptr))) => (*kind as u32, *ptr as usize),
                    _ => return Err("env.dyn_from_array expects (i32, i32)".to_string()),
                };
                let cell = dyn_from_static_array(
                    memory,
                    &mut self.next_dyn,
                    &mut self.next_file,
                    kind,
                    ptr,
                )?;
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_cell_at") => {
                let (cell, index) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I64(index))) => (*cell as usize, *index),
                    _ => return Err("env.dyn_cell_at expects (i32, i64)".to_string()),
                };
                let (tag, aux, payload) = dyn_tag(memory, cell);
                if tag != 5 {
                    return Err("dynamic sort needs a dynamic array".to_string());
                }
                if index < 0 || index >= aux as i64 {
                    return Err("dynamic sort index out of bounds".to_string());
                }
                Ok(vec![WVal::I32(
                    (payload as usize + index as usize * 16) as i32,
                )])
            }
            ("env", "dyn_swap") => {
                let (cell, a, b) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(cell)), Some(WVal::I64(a)), Some(WVal::I64(b))) => {
                        (*cell as usize, *a, *b)
                    }
                    _ => return Err("env.dyn_swap expects (i32, i64, i64)".to_string()),
                };
                let (tag, aux, payload) = dyn_tag(memory, cell);
                if tag != 5 {
                    return Err("dynamic sort needs a dynamic array".to_string());
                }
                if a < 0 || b < 0 || a >= aux as i64 || b >= aux as i64 {
                    return Err("dynamic sort index out of bounds".to_string());
                }
                let left = payload as usize + a as usize * 16;
                let right = payload as usize + b as usize * 16;
                let mut temp = [0u8; 16];
                temp.copy_from_slice(&memory[left..left + 16]);
                memory.copy_within(right..right + 16, left);
                memory[right..right + 16].copy_from_slice(&temp);
                Ok(vec![])
            }
            ("env", "dyn_from_record") => {
                let (kinds, table, count, rec) =
                    match (args.first(), args.get(1), args.get(2), args.get(3)) {
                        (
                            Some(WVal::I32(kinds)),
                            Some(WVal::I32(table)),
                            Some(WVal::I32(count)),
                            Some(WVal::I32(rec)),
                        ) => (
                            *kinds as usize,
                            *table as usize,
                            *count as usize,
                            *rec as usize,
                        ),
                        _ => {
                            return Err(
                                "env.dyn_from_record expects (i32, i32, i32, i32)".to_string()
                            )
                        }
                    };
                let cell = dyn_from_record(memory, &mut self.next_dyn, kinds, table, count, rec)?;
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_parse") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.dyn_parse expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory_text(memory, ptr, len);
                let value = crate::json::parse(&text)?;
                let cell =
                    store_dyn_scalar(memory, &mut self.next_dyn, &mut self.next_file, &value)?;
                Ok(vec![WVal::I32(cell as i32)])
            }
            ("env", "dyn_str") => {
                let (cell, json) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(cell)), Some(WVal::I32(mode))) => (*cell as usize, *mode != 0),
                    _ => return Err("env.dyn_str expects (i32 cell, i32 mode)".to_string()),
                };
                let text = format_dyn_cell(memory, cell, json)?;
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("dyn_str result does not fit the file region".to_string()),
                }
            }
            ("env", "dyn_type") => {
                let cell = match args.first() {
                    Some(WVal::I32(cell)) => *cell as usize,
                    _ => return Err("env.dyn_type expects one i32".to_string()),
                };
                if cell + 16 > memory.len() {
                    return Err("dynamic cell out of bounds".to_string());
                }
                let tag = u32::from_le_bytes(memory[cell..cell + 4].try_into().unwrap());
                let name = match tag {
                    0 => "null",
                    1 => "int",
                    2 => "float",
                    3 => "bool",
                    4 => "str",
                    5 => "array",
                    6 => "object",
                    _ => "dynamic",
                };
                match store_text_block(memory, &mut self.next_file, name.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("dyn_type result does not fit the file region".to_string()),
                }
            }
            ("env", "print_map") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_map expects one i32".to_string()),
                };
                let text = format_map(memory, ptr);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "print_arr_arr") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_arr_arr expects one i32".to_string()),
                };
                let text = format_arr_arr(memory, ptr);
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("env", "arr_sort_f") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_sort_f expects one i32".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_sort_f array is outside memory".to_string());
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                let mut values: Vec<f64> = Vec::with_capacity(count);
                for index in 0..count {
                    let offset = ptr + 4 + index * 8;
                    if offset + 8 > memory.len() {
                        break;
                    }
                    values.push(f64::from_le_bytes(
                        memory[offset..offset + 8].try_into().unwrap(),
                    ));
                }
                values.sort_by(|left, right| left.total_cmp(right));
                for (index, value) in values.iter().enumerate() {
                    let offset = ptr + 4 + index * 8;
                    memory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
                }
                Ok(vec![WVal::I32(ptr as i32)])
            }
            ("env", "arr_sort_str") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_sort_str expects one i32".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_sort_str array is outside memory".to_string());
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                let mut entries: Vec<(usize, usize)> = Vec::with_capacity(count);
                for index in 0..count {
                    let offset = ptr + 4 + index * 8;
                    if offset + 8 > memory.len() {
                        break;
                    }
                    let piece_ptr =
                        u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap()) as usize;
                    let piece_len =
                        u32::from_le_bytes(memory[offset + 4..offset + 8].try_into().unwrap())
                            as usize;
                    entries.push((piece_ptr, piece_len));
                }
                entries.sort_by(|left, right| {
                    let a = memory
                        .get(left.0..left.0.saturating_add(left.1))
                        .unwrap_or(&[]);
                    let b = memory
                        .get(right.0..right.0.saturating_add(right.1))
                        .unwrap_or(&[]);
                    a.cmp(b)
                });
                for (index, (piece_ptr, piece_len)) in entries.iter().enumerate() {
                    let offset = ptr + 4 + index * 8;
                    memory[offset..offset + 4].copy_from_slice(&(*piece_ptr as u32).to_le_bytes());
                    memory[offset + 4..offset + 8]
                        .copy_from_slice(&(*piece_len as u32).to_le_bytes());
                }
                Ok(vec![WVal::I32(ptr as i32)])
            }
            ("env", "arr_join_int") => {
                let (ptr, sep_ptr, sep_len) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(sp)), Some(WVal::I32(sl))) => {
                        (*ptr as usize, *sp as usize, *sl as usize)
                    }
                    _ => return Err("env.arr_join_int expects (i32, i32, i32)".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_join_int array is outside memory".to_string());
                }
                let sep = memory_text(memory, sep_ptr, sep_len);
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                let mut text = String::new();
                for index in 0..count {
                    if index > 0 {
                        text.push_str(&sep);
                    }
                    let offset = ptr + 4 + index * 8;
                    if offset + 8 > memory.len() {
                        break;
                    }
                    text.push_str(
                        &i64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap())
                            .to_string(),
                    );
                }
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Ok(vec![WVal::I32(0)]),
                }
            }
            ("env", "arr_join_f") => {
                let (ptr, sep_ptr, sep_len) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(sp)), Some(WVal::I32(sl))) => {
                        (*ptr as usize, *sp as usize, *sl as usize)
                    }
                    _ => return Err("env.arr_join_f expects (i32, i32, i32)".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_join_f array is outside memory".to_string());
                }
                let sep = memory_text(memory, sep_ptr, sep_len);
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                let mut text = String::new();
                for index in 0..count {
                    if index > 0 {
                        text.push_str(&sep);
                    }
                    let offset = ptr + 4 + index * 8;
                    if offset + 8 > memory.len() {
                        break;
                    }
                    let value = f64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
                    text.push_str(&crate::value::fmt_float(value));
                }
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Ok(vec![WVal::I32(0)]),
                }
            }
            ("env", "arr_sort") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.arr_sort expects one i32".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_sort array is outside memory".to_string());
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                let mut values: Vec<i64> = Vec::with_capacity(count);
                for index in 0..count {
                    let offset = ptr + 4 + index * 8;
                    if offset + 8 > memory.len() {
                        break;
                    }
                    values.push(i64::from_le_bytes(
                        memory[offset..offset + 8].try_into().unwrap(),
                    ));
                }
                values.sort_unstable();
                // The VM sorts in place, so keep the same array identity.
                for (index, value) in values.iter().enumerate() {
                    let offset = ptr + 4 + index * 8;
                    memory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
                }
                Ok(vec![WVal::I32(ptr as i32)])
            }
            ("env", "str_split") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (ptr, len, sep_ptr, sep_len) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(ptr), Some(len), Some(sp), Some(sl)) => (ptr, len, sp, sl),
                    _ => return Err("env.str_split expects (i32,i32,i32,i32)".to_string()),
                };
                let sep = memory
                    .get(sep_ptr..sep_ptr.saturating_add(sep_len))
                    .ok_or_else(|| "env.str_split separator is outside memory".to_string())?;
                if sep.is_empty() {
                    return Err("wasm target split() needs a non-empty separator".to_string());
                }
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "env.str_split string is outside memory".to_string())?;
                let mut pieces: Vec<(u32, u32)> = Vec::new();
                let mut cursor = 0usize;
                while let Some(found) = text[cursor..]
                    .windows(sep.len())
                    .position(|window| window == sep)
                {
                    pieces.push(((ptr + cursor) as u32, found as u32));
                    cursor += found + sep.len();
                }
                pieces.push(((ptr + cursor) as u32, (text.len() - cursor) as u32));
                let count = pieces.len() as u32;
                let size = 4 + 8 * pieces.len();
                let start = self.next_file as usize;
                if start + size > memory.len() {
                    return Err("split result does not fit the memory region".to_string());
                }
                memory[start..start + 4].copy_from_slice(&count.to_le_bytes());
                for (index, (piece_ptr, piece_len)) in pieces.iter().enumerate() {
                    let at = start + 4 + index * 8;
                    memory[at..at + 4].copy_from_slice(&piece_ptr.to_le_bytes());
                    memory[at + 4..at + 8].copy_from_slice(&piece_len.to_le_bytes());
                }
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "str_join") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (array_ptr, sep_ptr, sep_len) =
                    match (read(args.first()), read(args.get(1)), read(args.get(2))) {
                        (Some(array), Some(sp), Some(sl)) => (array, sp, sl),
                        _ => {
                            return Err(
                                "env.str_join expects (i32 array, i32 ptr, i32 len)".to_string()
                            )
                        }
                    };
                let sep = memory
                    .get(sep_ptr..sep_ptr.saturating_add(sep_len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_join separator is outside memory".to_string())?;
                if array_ptr + 4 > memory.len() {
                    return Err("env.str_join array is outside memory".to_string());
                }
                let count =
                    u32::from_le_bytes(memory[array_ptr..array_ptr + 4].try_into().unwrap());
                let mut text = String::new();
                for index in 0..count as usize {
                    if index > 0 {
                        text.push_str(&sep);
                    }
                    let at = array_ptr + 4 + index * 8;
                    if at + 8 > memory.len() {
                        break;
                    }
                    let piece_ptr =
                        u32::from_le_bytes(memory[at..at + 4].try_into().unwrap()) as usize;
                    let piece_len =
                        u32::from_le_bytes(memory[at + 4..at + 8].try_into().unwrap()) as usize;
                    if let Some(bytes) = memory.get(piece_ptr..piece_ptr + piece_len) {
                        text.push_str(&String::from_utf8_lossy(bytes));
                    }
                }
                match store_text_block(memory, &mut self.next_file, text.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("string result does not fit the memory region".to_string()),
                }
            }
            ("env", "str_slice") => {
                let (ptr, len, start, end) =
                    match (args.first(), args.get(1), args.get(2), args.get(3)) {
                        (
                            Some(WVal::I32(ptr)),
                            Some(WVal::I32(len)),
                            Some(WVal::I64(start)),
                            Some(WVal::I64(end)),
                        ) => (*ptr as usize, *len as usize, *start, *end),
                        _ => {
                            return Err(
                                "env.str_slice expects (i32 ptr, i32 len, i64, i64)".to_string()
                            )
                        }
                    };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_slice string is outside memory".to_string())?;
                let chars: Vec<char> = text.chars().collect();
                let length = chars.len() as i64;
                let clamp = |value: i64| -> i64 {
                    let normalized = if value < 0 { length + value } else { value };
                    normalized.clamp(0, length)
                };
                let from = clamp(start);
                let to = clamp(end);
                let slice: String = if from <= to {
                    chars[from as usize..to as usize].iter().collect()
                } else {
                    String::new()
                };
                match store_text_block(memory, &mut self.next_file, slice.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("string result does not fit the memory region".to_string()),
                }
            }
            ("env", "arr_slice") => {
                let (ptr, start, end) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I64(start)), Some(WVal::I64(end))) => {
                        (*ptr as usize, *start, *end)
                    }
                    _ => return Err("env.arr_slice expects (i32 ptr, i64, i64)".to_string()),
                };
                if ptr + 4 > memory.len() {
                    return Err("env.arr_slice array is outside memory".to_string());
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as i64;
                let clamp = |value: i64| -> i64 {
                    let normalized = if value < 0 { count + value } else { value };
                    normalized.clamp(0, count)
                };
                let from = clamp(start);
                let to = clamp(end);
                let keep = if from <= to { (to - from) as u32 } else { 0 };
                let size = 4 + 8 * keep as usize;
                let at = self.next_file as usize;
                if at + size > memory.len() {
                    return Err("slice result does not fit the memory region".to_string());
                }
                memory[at..at + 4].copy_from_slice(&keep.to_le_bytes());
                let src = ptr + 4 + from as usize * 8;
                let dst = at + 4;
                if keep > 0 {
                    memory.copy_within(src..src + keep as usize * 8, dst);
                }
                self.next_file = (at + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(at as i32)])
            }
            ("env", "str_trim") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.str_trim expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_trim string is outside memory".to_string())?;
                let trimmed = text.trim();
                match store_text_block(memory, &mut self.next_file, trimmed.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("string result does not fit the memory region".to_string()),
                }
            }
            ("env", "str_upper") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.str_upper expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_upper string is outside memory".to_string())?;
                let upper = text.to_uppercase();
                match store_text_block(memory, &mut self.next_file, upper.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("string result does not fit the memory region".to_string()),
                }
            }
            ("env", "str_lower") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.str_lower expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_lower string is outside memory".to_string())?;
                let lower = text.to_lowercase();
                match store_text_block(memory, &mut self.next_file, lower.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("string result does not fit the memory region".to_string()),
                }
            }
            ("env", "str_replace") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (ptr, len, from_ptr, from_len, to_ptr, to_len) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                    read(args.get(4)),
                    read(args.get(5)),
                ) {
                    (Some(ptr), Some(len), Some(fp), Some(fl), Some(tp), Some(tl)) => {
                        (ptr, len, fp, fl, tp, tl)
                    }
                    _ => {
                        return Err("env.str_replace expects (i32,i32,i32,i32,i32,i32)".to_string())
                    }
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_replace string is outside memory".to_string())?;
                let from = memory
                    .get(from_ptr..from_ptr.saturating_add(from_len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_replace pattern is outside memory".to_string())?;
                let to = memory
                    .get(to_ptr..to_ptr.saturating_add(to_len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_replace replacement is outside memory".to_string())?;
                let replaced = text.replace(&from, &to);
                match store_text_block(memory, &mut self.next_file, replaced.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("string result does not fit the memory region".to_string()),
                }
            }
            ("env", "str_index_of") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (ptr, len, needle_ptr, needle_len) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(ptr), Some(len), Some(np), Some(nl)) => (ptr, len, np, nl),
                    _ => return Err("env.str_index_of expects (i32,i32,i32,i32)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_index_of string is outside memory".to_string())?;
                let needle = memory
                    .get(needle_ptr..needle_ptr.saturating_add(needle_len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_index_of needle is outside memory".to_string())?;
                let position = text
                    .find(&needle)
                    .map(|at| text[..at].chars().count() as i64)
                    .unwrap_or(-1);
                Ok(vec![WVal::I64(position)])
            }
            ("env", "str_repeat") => {
                let (ptr, len, count) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len)), Some(WVal::I64(count))) => {
                        (*ptr as usize, *len as usize, *count)
                    }
                    _ => {
                        return Err(
                            "env.str_repeat expects (i32 ptr, i32 len, i64 count)".to_string()
                        )
                    }
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_repeat string is outside memory".to_string())?;
                let repeated = text.repeat(count.max(0) as usize);
                match store_text_block(memory, &mut self.next_file, repeated.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("string result does not fit the memory region".to_string()),
                }
            }
            ("env", "str_char_at") => {
                let (ptr, len, index) = match (args.first(), args.get(1), args.get(2)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len)), Some(WVal::I32(index))) => {
                        (*ptr as usize, *len as usize, *index)
                    }
                    _ => {
                        return Err(
                            "env.str_char_at expects (i32 ptr, i32 len, i32 index)".to_string()
                        )
                    }
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.str_char_at string is outside memory".to_string())?;
                let chars: Vec<char> = text.chars().collect();
                let count = chars.len() as i64;
                let mut at = index as i64;
                if at < 0 {
                    at += count;
                }
                if at < 0 || at >= count {
                    return Err("index out of bounds".to_string());
                }
                let mut encoded = [0u8; 4];
                let char_text = chars[at as usize].encode_utf8(&mut encoded);
                let start = self.next_file as usize;
                let size = 4 + char_text.len();
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(char_text.len() as u32).to_le_bytes());
                memory[start + 4..start + size].copy_from_slice(char_text.as_bytes());
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "panic") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.panic expects (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .unwrap_or_default();
                Err(text)
            }
            ("env", "str_test") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(value)) => Some(*value as usize),
                    _ => None,
                };
                let (hp, hl, np, nl, mode) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                    read(args.get(4)),
                ) {
                    (Some(hp), Some(hl), Some(np), Some(nl), Some(mode)) => (hp, hl, np, nl, mode),
                    _ => return Err("env.str_test expects (i32,i32,i32,i32,i32)".to_string()),
                };
                let hay = memory
                    .get(hp..hp.saturating_add(hl))
                    .ok_or_else(|| "env.str_test haystack is outside memory".to_string())?;
                let needle = memory
                    .get(np..np.saturating_add(nl))
                    .ok_or_else(|| "env.str_test needle is outside memory".to_string())?;
                let found = match mode {
                    0 => {
                        if needle.is_empty() {
                            true
                        } else {
                            hay.windows(needle.len()).any(|window| window == needle)
                        }
                    }
                    1 => hay.starts_with(needle),
                    2 => hay.ends_with(needle),
                    _ => return Err("env.str_test mode must be 0-2".to_string()),
                };
                Ok(vec![WVal::I32(if found { 1 } else { 0 })])
            }
            ("env", "bool_to_str") => {
                let value = match args.first() {
                    Some(WVal::I32(value)) => *value,
                    _ => return Err("env.bool_to_str expects one i32".to_string()),
                };
                let text = if value == 0 { "false" } else { "true" };
                let start = self.next_file as usize;
                let size = 4 + text.len();
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                memory[start + 4..start + size].copy_from_slice(text.as_bytes());
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "int_to_str") => {
                let value = match args.first() {
                    Some(WVal::I64(value)) => *value,
                    _ => return Err("env.int_to_str expects one i64".to_string()),
                };
                let text = value.to_string();
                let start = self.next_file as usize;
                let size = 4 + text.len();
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                memory[start + 4..start + size].copy_from_slice(text.as_bytes());
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "f64_to_str") => {
                let value = match args.first() {
                    Some(WVal::F64(value)) => *value,
                    _ => return Err("env.f64_to_str expects one f64".to_string()),
                };
                let text = if value.is_finite() {
                    crate::value::fmt_float(value)
                } else {
                    "null".to_string()
                };
                let start = self.next_file as usize;
                let size = 4 + text.len();
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                memory[start + 4..start + size].copy_from_slice(text.as_bytes());
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "str_to_int") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.str_to_int expects (i32 ptr, i32 len)".to_string()),
                };
                let bytes = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "env.str_to_int range is outside memory".to_string())?;
                let text = String::from_utf8_lossy(bytes);
                let trimmed = text.trim();
                match trimmed.parse::<i64>() {
                    Ok(value) => Ok(vec![WVal::I64(value)]),
                    Err(_) => Err("cannot convert str to int".to_string()),
                }
            }
            ("env", "map_new") => {
                const MAP_CAPACITY: u32 = 64;
                let start = self.next_map as usize;
                let size = 8 + MAP_CAPACITY as usize * 16;
                if start + size > memory.len() {
                    return Err("map allocation failed (module heap exhausted)".to_string());
                }
                memory[start..start + 4].copy_from_slice(&0u32.to_le_bytes());
                memory[start + 4..start + 8].copy_from_slice(&MAP_CAPACITY.to_le_bytes());
                self.next_map = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "map_set") => {
                const MAP_CAPACITY: u32 = 64;
                let read_i32 = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let ptr = match read_i32(args.first()) {
                    Some(ptr) => ptr,
                    None => {
                        return Err("env.map_set expects (i32 map, i32 ptr, i32 len, i64 value)"
                            .to_string())
                    }
                };
                let key_ptr = match read_i32(args.get(1)) {
                    Some(ptr) => ptr,
                    None => return Err("env.map_set expects a key pointer".to_string()),
                };
                let key_len = match read_i32(args.get(2)) {
                    Some(len) => len,
                    None => return Err("env.map_set expects a key length".to_string()),
                };
                let value = match args.get(3) {
                    Some(WVal::I64(value)) => *value,
                    _ => return Err("env.map_set expects one i64 value".to_string()),
                };
                if ptr == 0 {
                    return Err("map is not initialized".to_string());
                }
                if ptr + 8 + MAP_CAPACITY as usize * 16 > memory.len() {
                    return Err("map is not initialized".to_string());
                }
                let key = memory
                    .get(key_ptr..key_ptr.saturating_add(key_len))
                    .ok_or_else(|| "env.map_set key is outside memory".to_string())?
                    .to_vec();
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                for entry in 0..count.min(MAP_CAPACITY as usize) {
                    let at = ptr + 8 + entry * 16;
                    let existing_ptr =
                        u32::from_le_bytes(memory[at..at + 4].try_into().unwrap()) as usize;
                    let existing_len =
                        u32::from_le_bytes(memory[at + 4..at + 8].try_into().unwrap()) as usize;
                    let same = existing_len == key.len()
                        && memory
                            .get(existing_ptr..existing_ptr + existing_len)
                            .map(|existing| existing == key.as_slice())
                            .unwrap_or(false);
                    if same {
                        memory[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
                        return Ok(vec![WVal::I32(ptr as i32)]);
                    }
                }
                if count >= MAP_CAPACITY as usize {
                    return Err("map is full (64 entries)".to_string());
                }
                let at = ptr + 8 + count * 16;
                memory[at..at + 4].copy_from_slice(&(key_ptr as u32).to_le_bytes());
                memory[at + 4..at + 8].copy_from_slice(&(key_len as u32).to_le_bytes());
                memory[at + 8..at + 16].copy_from_slice(&value.to_le_bytes());
                memory[ptr..ptr + 4].copy_from_slice(&((count + 1) as u32).to_le_bytes());
                Ok(vec![WVal::I32(ptr as i32)])
            }
            ("env", "map_get") => {
                let read_i32 = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let ptr = match read_i32(args.first()) {
                    Some(ptr) => ptr,
                    None => {
                        return Err("env.map_get expects (i32 map, i32 ptr, i32 len)".to_string())
                    }
                };
                let key_ptr = match read_i32(args.get(1)) {
                    Some(ptr) => ptr,
                    None => return Err("env.map_get expects a key pointer".to_string()),
                };
                let key_len = match read_i32(args.get(2)) {
                    Some(len) => len,
                    None => return Err("env.map_get expects a key length".to_string()),
                };
                if ptr == 0 {
                    return Err("map is not initialized".to_string());
                }
                let key = memory
                    .get(key_ptr..key_ptr.saturating_add(key_len))
                    .ok_or_else(|| "env.map_get key is outside memory".to_string())?
                    .to_vec();
                let count = if ptr + 4 <= memory.len() {
                    u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize
                } else {
                    return Err("map is not initialized".to_string());
                };
                for entry in 0..count.min(64) {
                    let at = ptr + 8 + entry * 16;
                    if at + 16 > memory.len() {
                        break;
                    }
                    let existing_ptr =
                        u32::from_le_bytes(memory[at..at + 4].try_into().unwrap()) as usize;
                    let existing_len =
                        u32::from_le_bytes(memory[at + 4..at + 8].try_into().unwrap()) as usize;
                    let same = existing_len == key.len()
                        && memory
                            .get(existing_ptr..existing_ptr + existing_len)
                            .map(|existing| existing == key.as_slice())
                            .unwrap_or(false);
                    if same {
                        let value = i64::from_le_bytes(memory[at + 8..at + 16].try_into().unwrap());
                        return Ok(vec![WVal::I64(value)]);
                    }
                }
                Ok(vec![WVal::I64(0)])
            }
            ("env", "map_has") => {
                let read_i32 = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (ptr, key_ptr, key_len) = match (
                    read_i32(args.first()),
                    read_i32(args.get(1)),
                    read_i32(args.get(2)),
                ) {
                    (Some(ptr), Some(key_ptr), Some(key_len)) => (ptr, key_ptr, key_len),
                    _ => return Err("env.map_has expects (i32 map, i32 ptr, i32 len)".to_string()),
                };
                if ptr == 0 {
                    return Err("map is not initialized".to_string());
                }
                let key = memory
                    .get(key_ptr..key_ptr.saturating_add(key_len))
                    .ok_or_else(|| "env.map_has key is outside memory".to_string())?
                    .to_vec();
                let count = if ptr + 4 <= memory.len() {
                    u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize
                } else {
                    return Err("map is not initialized".to_string());
                };
                for entry in 0..count.min(64) {
                    let at = ptr + 8 + entry * 16;
                    if at + 16 > memory.len() {
                        break;
                    }
                    let existing_ptr =
                        u32::from_le_bytes(memory[at..at + 4].try_into().unwrap()) as usize;
                    let existing_len =
                        u32::from_le_bytes(memory[at + 4..at + 8].try_into().unwrap()) as usize;
                    let same = existing_len == key.len()
                        && memory
                            .get(existing_ptr..existing_ptr + existing_len)
                            .map(|existing| existing == key.as_slice())
                            .unwrap_or(false);
                    if same {
                        return Ok(vec![WVal::I32(1)]);
                    }
                }
                Ok(vec![WVal::I32(0)])
            }
            ("env", "map_len") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.map_len expects one i32 map".to_string()),
                };
                if ptr == 0 {
                    return Err("map is not initialized".to_string());
                }
                let count = if ptr + 4 <= memory.len() {
                    u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap())
                } else {
                    return Err("map is not initialized".to_string());
                };
                Ok(vec![WVal::I32(count as i32)])
            }
            ("env", "map_key_at") => {
                let (ptr, index) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(index))) => {
                        (*ptr as usize, *index as usize)
                    }
                    _ => return Err("env.map_key_at expects (i32 map, i32 index)".to_string()),
                };
                if ptr + 8 > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                if index >= count {
                    return Ok(vec![WVal::I32(0)]);
                }
                let at = ptr + 8 + index * 16;
                let key_ptr = u32::from_le_bytes(memory[at..at + 4].try_into().unwrap()) as usize;
                let key_len =
                    u32::from_le_bytes(memory[at + 4..at + 8].try_into().unwrap()) as usize;
                let start = self.next_file as usize;
                let size = 4 + key_len;
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(key_len as u32).to_le_bytes());
                let key = memory
                    .get(key_ptr..key_ptr + key_len)
                    .map(|key| key.to_vec())
                    .unwrap_or_default();
                memory[start + 4..start + size].copy_from_slice(&key);
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "map_value_at") => {
                let (ptr, index) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(index))) => {
                        (*ptr as usize, *index as usize)
                    }
                    _ => return Err("env.map_value_at expects (i32 map, i32 index)".to_string()),
                };
                if ptr + 8 > memory.len() {
                    return Ok(vec![WVal::I64(0)]);
                }
                let count = u32::from_le_bytes(memory[ptr..ptr + 4].try_into().unwrap()) as usize;
                if index >= count {
                    return Ok(vec![WVal::I64(0)]);
                }
                let at = ptr + 8 + index * 16;
                let value = i64::from_le_bytes(memory[at + 8..at + 16].try_into().unwrap());
                Ok(vec![WVal::I64(value)])
            }
            ("env", "now_ms") => {
                let millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or(0);
                Ok(vec![WVal::I64(millis)])
            }
            ("env", "sleep_ms") => {
                let millis = match args.first() {
                    Some(WVal::I64(value)) => *value,
                    _ => return Err("env.sleep_ms expects one i64".to_string()),
                };
                if millis > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(millis as u64));
                }
                Ok(Vec::new())
            }
            ("env", "cwd") => {
                let text = std::env::current_dir()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let start = self.next_file as usize;
                let size = 4 + text.len();
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                memory[start + 4..start + size].copy_from_slice(text.as_bytes());
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "env_get") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.env_get expects (i32 ptr, i32 len)".to_string()),
                };
                let name = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "env.env_get name is outside memory".to_string())?;
                let name = String::from_utf8_lossy(name).to_string();
                let text = std::env::var(&name).unwrap_or_default();
                let start = self.next_file as usize;
                let size = 4 + text.len();
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                memory[start + 4..start + size].copy_from_slice(text.as_bytes());
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("env", "env_get_default") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (np, nl, dp, dl) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(np), Some(nl), Some(dp), Some(dl)) => (np, nl, dp, dl),
                    _ => return Err("env.env_get_default argument mismatch".to_string()),
                };
                let name = memory
                    .get(np..np.saturating_add(nl))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.env_get_default name is outside memory".to_string())?;
                let default = memory
                    .get(dp..dp.saturating_add(dl))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.env_get_default default is outside memory".to_string())?;
                let value = std::env::var(&name).unwrap_or(default);
                match store_text_block(memory, &mut self.next_file, value.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => {
                        Err("env.env_get_default result does not fit the file region".to_string())
                    }
                }
            }
            ("env", "mkdir_soft") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.mkdir_soft expects (i32 ptr, i32 len)".to_string()),
                };
                let path = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "env.mkdir_soft path is outside memory".to_string())?;
                let start = match std::fs::create_dir_all(&path) {
                    Ok(()) => store_soft_block(memory, &mut self.next_file, 0, &[])?,
                    Err(error) => store_soft_block(
                        memory,
                        &mut self.next_file,
                        1,
                        format!("cannot create '{}': {}", path, error).as_bytes(),
                    )?,
                };
                Ok(vec![WVal::I32(start)])
            }
            ("env", "mkdir") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.mkdir expects (i32 ptr, i32 len)".to_string()),
                };
                let path = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "env.mkdir path is outside memory".to_string())?;
                let path = String::from_utf8_lossy(path).to_string();
                let ok = std::fs::create_dir_all(&path).is_ok();
                Ok(vec![WVal::I32(if ok { 0 } else { -1 })])
            }
            ("env", "file_exists") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("env.file_exists expects (i32 ptr, i32 len)".to_string()),
                };
                let path = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "env.file_exists path is outside memory".to_string())?;
                let path = String::from_utf8_lossy(path);
                Ok(vec![WVal::I32(
                    if std::path::Path::new(path.as_ref()).exists() {
                        1
                    } else {
                        0
                    },
                )])
            }
            ("env", "fmod") => {
                let (a, b) = match (args.first(), args.get(1)) {
                    (Some(WVal::F64(a)), Some(WVal::F64(b))) => (*a, *b),
                    _ => return Err("env.fmod expects two f64 values".to_string()),
                };
                Ok(vec![WVal::F64(a % b)])
            }
            ("env", "str_compare") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (ap, al, bp, bl) =
                    match (
                        read(args.first()),
                        read(args.get(1)),
                        read(args.get(2)),
                        read(args.get(3)),
                    ) {
                        (Some(ap), Some(al), Some(bp), Some(bl)) => (ap, al, bp, bl),
                        _ => return Err(
                            "env.str_compare expects (i32 a_ptr, i32 a_len, i32 b_ptr, i32 b_len)"
                                .to_string(),
                        ),
                    };
                let a = memory
                    .get(ap..ap.saturating_add(al))
                    .ok_or_else(|| "env.str_compare a is outside memory".to_string())?;
                let b = memory
                    .get(bp..bp.saturating_add(bl))
                    .ok_or_else(|| "env.str_compare b is outside memory".to_string())?;
                let order = match a.cmp(b) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                Ok(vec![WVal::I32(order)])
            }
            ("webgl", "create") => {
                let (width, height) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(width)), Some(WVal::I32(height))) => {
                        (*width as u32, *height as u32)
                    }
                    _ => return Err("webgl.create expects (i32 width, i32 height)".to_string()),
                };
                self.gl.resize(width, height);
                Ok(vec![WVal::I32(1)])
            }
            ("webgl", "clear_color") => {
                let mut channels = [0.0f64; 4];
                for (index, channel) in channels.iter_mut().enumerate() {
                    *channel = match args.get(index) {
                        Some(WVal::F64(value)) => *value,
                        Some(WVal::I64(value)) => *value as f64,
                        _ => return Err("webgl.clear_color expects (f64 r, g, b, a)".to_string()),
                    };
                }
                self.gl.clear = channels;
                Ok(Vec::new())
            }
            ("webgl", "clear") => {
                self.gl.clear();
                Ok(Vec::new())
            }
            ("webgl", "read_pixel") => {
                let (x, y) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(x)), Some(WVal::I32(y))) => (*x as u32, *y as u32),
                    _ => return Err("webgl.read_pixel expects (i32 x, i32 y)".to_string()),
                };
                Ok(vec![WVal::I32(self.gl.pixel(x, y) as i32)])
            }
            ("webgl", "to_png") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("webgl.to_png expects (i32 ptr, i32 len)".to_string()),
                };
                let path_bytes = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "webgl.to_png path is outside memory".to_string())?;
                let path = String::from_utf8_lossy(path_bytes).to_string();
                let encoded =
                    crate::png::encode_rgba(self.gl.width, self.gl.height, &self.gl.pixels);
                let status = match encoded {
                    Ok(png) => {
                        if std::fs::write(&path, png).is_ok() {
                            0
                        } else {
                            -1
                        }
                    }
                    Err(_) => -1,
                };
                Ok(vec![WVal::I32(status)])
            }
            ("web", "console_log_i64") => {
                let value = match args.first() {
                    Some(WVal::I64(value)) => *value,
                    _ => return Err("web.console_log_i64 expects one i64".to_string()),
                };
                (self.on_i64)(value);
                Ok(Vec::new())
            }
            ("web", "console_log_f64") => {
                let value = match args.first() {
                    Some(WVal::F64(value)) => *value,
                    _ => return Err("web.console_log_f64 expects one f64".to_string()),
                };
                let text = if value.is_finite() {
                    crate::value::fmt_float(value)
                } else {
                    "null".to_string()
                };
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("web", "console_log_utf8") => {
                let ptr = match args.first() {
                    Some(WVal::I32(value)) => *value as usize,
                    _ => return Err("web.console_log_utf8 expects (i32 ptr, i32 len)".to_string()),
                };
                let len = match args.get(1) {
                    Some(WVal::I32(value)) => *value as usize,
                    _ => return Err("web.console_log_utf8 expects (i32 ptr, i32 len)".to_string()),
                };
                let end = ptr
                    .checked_add(len)
                    .ok_or_else(|| "web.console_log_utf8 range overflow".to_string())?;
                let slice = memory
                    .get(ptr..end)
                    .ok_or_else(|| "web.console_log_utf8 range is outside memory".to_string())?;
                let text = String::from_utf8_lossy(slice).to_string();
                (self.on_str)(&text);
                Ok(Vec::new())
            }
            ("web", "performance_now") => {
                let millis = self.started.elapsed().as_secs_f64() * 1000.0;
                Ok(vec![WVal::F64(millis)])
            }
            ("bx", name @ ("exec_record" | "exec_record_soft")) => {
                let soft = name == "exec_record_soft";
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(v)) => Some(*v as usize),
                    _ => None,
                };
                let params: Vec<usize> = (0..10)
                    .map(|index| read(args.get(index)).unwrap_or(0))
                    .collect();
                let (mode, cmd_ptr, cmd_len) = (params[0], params[1], params[2]);
                let (cwd_ptr, cwd_len) = (params[3], params[4]);
                let (stdin_ptr, stdin_len) = (params[5], params[6]);
                let timeout_ms = params[7];
                let shell = params[8] != 0;
                let text_at = |ptr: usize, len: usize| -> String {
                    memory
                        .get(ptr..ptr.saturating_add(len))
                        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                        .unwrap_or_default()
                };
                let mut command = if mode == 1 {
                    let mut argv: Vec<String> = Vec::new();
                    if cmd_ptr + 4 <= memory.len() {
                        let count =
                            u32::from_le_bytes(memory[cmd_ptr..cmd_ptr + 4].try_into().unwrap())
                                as usize;
                        for index in 0..count {
                            let slot = cmd_ptr + 4 + index * 8;
                            if slot + 8 > memory.len() {
                                break;
                            }
                            let ptr = u32::from_le_bytes(memory[slot..slot + 4].try_into().unwrap())
                                as usize;
                            let len =
                                u32::from_le_bytes(memory[slot + 4..slot + 8].try_into().unwrap())
                                    as usize;
                            argv.push(text_at(ptr, len));
                        }
                    }
                    let Some(program) = argv.first() else {
                        if soft {
                            return exec_error_block(
                                memory,
                                &mut self.next_proc,
                                "exec() needs at least one argument",
                            );
                        }
                        return Err("exec() needs at least one argument".to_string());
                    };
                    let mut command = std::process::Command::new(program);
                    command.args(&argv[1..]);
                    command
                } else {
                    let line = text_at(cmd_ptr, cmd_len);
                    if shell {
                        shell_command(&line)
                    } else {
                        let mut words = line.split_whitespace();
                        let Some(program) = words.next() else {
                            if soft {
                                return exec_error_block(
                                    memory,
                                    &mut self.next_proc,
                                    "exec() needs a command",
                                );
                            }
                            return Err("exec() needs a command".to_string());
                        };
                        let mut command = std::process::Command::new(program);
                        command.args(words);
                        command
                    }
                };
                for (key, value) in self.exec_env.drain(..) {
                    command.env(key, value);
                }
                if cwd_len > 0 {
                    command.current_dir(text_at(cwd_ptr, cwd_len));
                }
                let wants_stdin = stdin_len > 0;
                if wants_stdin {
                    command.stdin(std::process::Stdio::piped());
                }
                command.stdout(std::process::Stdio::piped());
                command.stderr(std::process::Stdio::piped());
                let started = std::time::Instant::now();
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(_) => {
                        let program = match command.get_program().to_str() {
                            Some(name) => name.to_string(),
                            None => String::new(),
                        };
                        let message = format!("cannot run '{}': program not found", program);
                        if soft {
                            return exec_error_block(memory, &mut self.next_proc, &message);
                        }
                        return Err(message);
                    }
                };
                if wants_stdin {
                    if let Some(mut sink) = child.stdin.take() {
                        let input = text_at(stdin_ptr, stdin_len);
                        let _ = std::io::Write::write_all(&mut sink, input.as_bytes());
                    }
                }
                let mut timed_out = false;
                if timeout_ms > 0 {
                    let deadline = std::time::Instant::now()
                        + std::time::Duration::from_millis(timeout_ms as u64);
                    loop {
                        match child.try_wait() {
                            Ok(Some(_)) => break,
                            Ok(None) => {
                                if std::time::Instant::now() >= deadline {
                                    let _ = child.kill();
                                    timed_out = true;
                                    break;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(5));
                            }
                            Err(_) => break,
                        }
                    }
                }
                let output = child.wait_with_output().ok();
                let elapsed = started.elapsed().as_millis() as u32;
                let (code, out, err) = match output {
                    Some(output) => (
                        output.status.code().unwrap_or(-1),
                        output.stdout,
                        output.stderr,
                    ),
                    None => (-1, Vec::new(), Vec::new()),
                };
                let size = 28 + out.len() + err.len();
                let start = self.next_proc as usize;
                if start + size > memory.len() {
                    return Err(format!(
                        "exec output of {} bytes does not fit the result region",
                        out.len() + err.len()
                    ));
                }
                let out_ptr = start + 28;
                let err_ptr = out_ptr + out.len();
                memory[start..start + 4].copy_from_slice(&code.to_le_bytes());
                memory[start + 4..start + 8].copy_from_slice(&(out_ptr as u32).to_le_bytes());
                memory[start + 8..start + 12].copy_from_slice(&(out.len() as u32).to_le_bytes());
                memory[start + 12..start + 16].copy_from_slice(&(err_ptr as u32).to_le_bytes());
                memory[start + 16..start + 20].copy_from_slice(&(err.len() as u32).to_le_bytes());
                memory[start + 20..start + 24].copy_from_slice(&elapsed.to_le_bytes());
                memory[start + 24..start + 28]
                    .copy_from_slice(&(u32::from(timed_out)).to_le_bytes());
                memory[out_ptr..out_ptr + out.len()].copy_from_slice(&out);
                memory[err_ptr..err_ptr + err.len()].copy_from_slice(&err);
                self.next_proc = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("bx", "run") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("bx.run expects (i32 ptr, i32 len)".to_string()),
                };
                let command = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "bx.run command is outside memory".to_string())?;
                let line = String::from_utf8_lossy(command).to_string();
                let status = shell_command(&line).status();
                let code = match status {
                    Ok(status) => status.code().unwrap_or(-1),
                    Err(_) => -1,
                };
                Ok(vec![WVal::I32(code)])
            }
            ("bx", "capture") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("bx.capture expects (i32 ptr, i32 len)".to_string()),
                };
                let command = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "bx.capture command is outside memory".to_string())?;
                let line = String::from_utf8_lossy(command).to_string();
                let output = shell_command(&line).output();
                let (code, stdout) = match output {
                    Ok(output) => (output.status.code().unwrap_or(-1), output.stdout),
                    Err(_) => (-1, Vec::new()),
                };
                let start = self.next_proc as usize;
                let size = 8 + stdout.len();
                if start + size > memory.len() {
                    return Err(format!(
                        "capture output of {} bytes does not fit the result region",
                        stdout.len()
                    ));
                }
                memory[start..start + 4].copy_from_slice(&code.to_le_bytes());
                memory[start + 4..start + 8].copy_from_slice(&(stdout.len() as u32).to_le_bytes());
                memory[start + 8..start + size].copy_from_slice(&stdout);
                self.next_proc = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("bx", "read_file") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("bx.read_file expects (i32 ptr, i32 len)".to_string()),
                };
                let path_bytes = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "bx.read_file path is outside memory".to_string())?;
                let path = String::from_utf8_lossy(path_bytes).to_string();
                let bytes = if path == "-" {
                    let mut buffer = Vec::new();
                    if let Err(error) =
                        std::io::Read::read_to_end(&mut std::io::stdin(), &mut buffer)
                    {
                        return Err(format!("cannot read stdin: {}", error));
                    }
                    buffer
                } else {
                    match std::fs::read(&path) {
                        Ok(bytes) => bytes,
                        Err(error) => return Err(format!("cannot read '{}': {}", path, error)),
                    }
                };
                let size = 4 + bytes.len();
                let start = self.next_file as usize;
                if start + size > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
                memory[start + 4..start + size].copy_from_slice(&bytes);
                self.next_file = (start + size + 7) as u32 & !7;
                Ok(vec![WVal::I32(start as i32)])
            }
            ("bx", "read_file_soft") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("bx.read_file_soft expects (i32 ptr, i32 len)".to_string()),
                };
                let path = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "bx.read_file_soft path is outside memory".to_string())?;
                let result = if path == "-" {
                    let mut buffer = Vec::new();
                    match std::io::Read::read_to_end(&mut std::io::stdin(), &mut buffer) {
                        Ok(_) => Ok(buffer),
                        Err(error) => Err(format!("cannot read stdin: {}", error)),
                    }
                } else {
                    std::fs::read(&path)
                        .map_err(|error| format!("cannot read '{}': {}", path, error))
                };
                let start = match result {
                    Ok(bytes) => store_soft_block(memory, &mut self.next_file, 0, &bytes)?,
                    Err(message) => {
                        store_soft_block(memory, &mut self.next_file, 1, message.as_bytes())?
                    }
                };
                Ok(vec![WVal::I32(start)])
            }
            ("bx", "write_file_soft") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (pp, pl, dp, dl) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(pp), Some(pl), Some(dp), Some(dl)) => (pp, pl, dp, dl),
                    _ => return Err("bx.write_file_soft argument mismatch".to_string()),
                };
                let path = memory
                    .get(pp..pp.saturating_add(pl))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "bx.write_file_soft path is outside memory".to_string())?;
                let data = memory
                    .get(dp..dp.saturating_add(dl))
                    .ok_or_else(|| "bx.write_file_soft data is outside memory".to_string())?;
                let start = match std::fs::write(&path, data) {
                    Ok(()) => store_soft_block(memory, &mut self.next_file, 0, &[])?,
                    Err(error) => store_soft_block(
                        memory,
                        &mut self.next_file,
                        1,
                        format!("cannot write '{}': {}", path, error).as_bytes(),
                    )?,
                };
                Ok(vec![WVal::I32(start)])
            }
            ("bx", "write_file") => {
                let read = |value: Option<&WVal>| match value {
                    Some(WVal::I32(ptr)) => Some(*ptr as usize),
                    _ => None,
                };
                let (pp, pl, dp, dl) = match (
                    read(args.first()),
                    read(args.get(1)),
                    read(args.get(2)),
                    read(args.get(3)),
                ) {
                    (Some(pp), Some(pl), Some(dp), Some(dl)) => (pp, pl, dp, dl),
                    _ => {
                        return Err(
                            "bx.write_file expects (i32 path_ptr, i32 path_len, i32 data_ptr, i32 data_len)"
                                .to_string(),
                        )
                    }
                };
                let path_bytes = memory
                    .get(pp..pp.saturating_add(pl))
                    .ok_or_else(|| "bx.write_file path is outside memory".to_string())?;
                let path = String::from_utf8_lossy(path_bytes).to_string();
                let data = memory
                    .get(dp..dp.saturating_add(dl))
                    .ok_or_else(|| "bx.write_file data is outside memory".to_string())?;
                let ok = std::fs::write(&path, data).is_ok();
                Ok(vec![WVal::I32(if ok { 0 } else { -1 })])
            }
            ("env", "basename") | ("env", "dirname") => {
                let is_base = name == "basename";
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("basename/dirname expect (i32 ptr, i32 len)".to_string()),
                };
                let text = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "basename/dirname path is outside memory".to_string())?;
                let result = if is_base {
                    path_basename(&text).to_string()
                } else {
                    path_dirname(&text).to_string()
                };
                match store_text_block(memory, &mut self.next_file, result.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("basename/dirname result does not fit the file region".to_string()),
                }
            }
            ("bx", "which") => {
                let (ptr, len) = match (args.first(), args.get(1)) {
                    (Some(WVal::I32(ptr)), Some(WVal::I32(len))) => (*ptr as usize, *len as usize),
                    _ => return Err("bx.which expects (i32 ptr, i32 len)".to_string()),
                };
                let name = memory
                    .get(ptr..ptr.saturating_add(len))
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                    .ok_or_else(|| "bx.which name is outside memory".to_string())?;
                let line = if cfg!(windows) {
                    format!("where {}", name)
                } else {
                    format!("which {}", name)
                };
                let found = shell_command(&line)
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
                    .and_then(|text| {
                        text.lines()
                            .map(|line| line.trim())
                            .find(|line| !line.is_empty())
                            .map(|line| line.to_string())
                    })
                    .unwrap_or_default();
                match store_text_block(memory, &mut self.next_file, found.as_bytes()) {
                    Some(start) => Ok(vec![WVal::I32(start as i32)]),
                    None => Err("which result does not fit the memory region".to_string()),
                }
            }
            ("bx", "arg") => {
                let index = match args.first() {
                    Some(WVal::I32(value)) => *value,
                    _ => return Err("bx.arg expects one i32".to_string()),
                };
                if index < 0 || index as usize >= self.argv.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                let text = self.argv[index as usize].as_bytes();
                let size = 4 + text.len();
                let start = self.next_argv as usize;
                let end = start + size;
                if end > memory.len() {
                    return Ok(vec![WVal::I32(0)]);
                }
                memory[start..start + 4].copy_from_slice(&(text.len() as u32).to_le_bytes());
                memory[start + 4..end].copy_from_slice(text);
                self.next_argv = end as u32;
                Ok(vec![WVal::I32(start as i32)])
            }
            _ => Err(format!("unimplemented wasm host import: {module}.{name}")),
        }
    }
}
