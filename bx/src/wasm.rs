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
    argv: &[String],
) -> PrintHost<'static, BoxedI64Sink, BoxedStrSink> {
    let on_i64: &'static mut BoxedI64Sink = Box::leak(Box::new(on_i64));
    let on_str: &'static mut BoxedStrSink = Box::leak(Box::new(on_str));
    PrintHost {
        on_i64,
        on_str,
        next_alloc: 8192,
        argv: argv.to_vec(),
        next_argv: 65_536,
        next_file: 131_072,
        next_map: 327_680,
        next_proc: 196_608,
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
            ("env", "print_arr_f64") => {
                let ptr = match args.first() {
                    Some(WVal::I32(ptr)) => *ptr as usize,
                    _ => return Err("env.print_arr_f64 expects one i32".to_string()),
                };
                let text = format_f64_array(memory, ptr);
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
                let bytes = memory
                    .get(ptr..ptr.saturating_add(len))
                    .ok_or_else(|| "env.str_slice string is outside memory".to_string())?;
                let length = bytes.len() as i64;
                let clamp = |value: i64| -> i64 {
                    let normalized = if value < 0 { length + value } else { value };
                    normalized.clamp(0, length)
                };
                let from = clamp(start);
                let to = clamp(end);
                let slice: Vec<u8> = if from <= to {
                    bytes[from as usize..to as usize].to_vec()
                } else {
                    Vec::new()
                };
                match store_text_block(memory, &mut self.next_file, &slice) {
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
                let position = text.find(&needle).map(|at| at as i64).unwrap_or(-1);
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
                    Err(_) => Err(format!("cannot convert '{}' to int", trimmed)),
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
                let path = String::from_utf8_lossy(path_bytes);
                let bytes = match std::fs::read(path.as_ref()) {
                    Ok(bytes) => bytes,
                    Err(error) => return Err(format!("cannot read '{}': {}", path, error)),
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
