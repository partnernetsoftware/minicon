mod ast;
mod builtins;
mod chunk;
mod json;
mod lexer;
mod parser;
mod value;
mod vm;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chunk::Chunk;
use value::{str_val, Value};
use vm::Vm;

const VERSION: &str = "0.1.0";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_help();
        std::process::exit(0);
    }
    // keep the harness process small: run the interpreter on a roomy Rust stack
    let handle = std::thread::Builder::new()
        .name("bx-vm".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || cli(args.remove(0), args));
    let code = match handle {
        Ok(h) => match h.join() {
            Ok(c) => c,
            Err(_) => {
                eprintln!("bx: internal error (panic in interpreter thread)");
                70
            }
        },
        Err(e) => {
            eprintln!("bx: cannot start interpreter thread: {}", e);
            70
        }
    };
    std::process::exit(code);
}

fn cli(cmd: String, args: Vec<String>) -> i32 {
    match cmd.as_str() {
        "run" => cmd_run(args),
        "eval" | "e" => cmd_eval(args),
        "build" => cmd_build(args),
        "dis" | "disasm" => cmd_dis(args),
        "check" => cmd_check(args),
        "help" | "-h" | "--help" => {
            if let Some(name) = args.first() {
                print_builtin_help(name);
            } else {
                print_help();
            }
            0
        }
        "version" | "-v" | "--version" => {
            println!("bx {}", VERSION);
            0
        }
        other => {
            eprintln!("bx: unknown command '{}' (try: bx help)", other);
            2
        }
    }
}

fn print_help() {
    let builtin_list = builtins::BUILTINS
        .iter()
        .map(|b| b.sig)
        .collect::<Vec<_>>()
        .join("\n  ");
    println!(
        r#"bx {version} - binary scripting for agents

usage
  bx run <script.bxs|script.bxb> [args...]   execute a script
  bx eval "<source>"                         evaluate source and print the result
  bx build <script.bxs> [-o out.bxb]         compile source to bytecode
  bx dis <script.bxs|out.bxb>                disassemble bytecode
  bx check <script.bxs|script.bxb>           compile only (no execution)
  bx help [builtin]                          this text, or one builtin's contract
  bx version

the language (js-like, statements end at newline or ';')
  let x = 1; let a = [1,2]; let o = {{a:1}}
  fn add(a, b) {{ return a + b }}
  if / else / while / for (;;) / for x in xs / break / continue / return
  a[0]  a[-1]  o.key  o["key"]  f(x)  "a,b".split(",")
  use "lib.bxs"   include another file (relative to this file)
  argv            array of script arguments
  script          script file name

the binary primitive
  exec(["git","status"])            -> {{code, ok, out, err, ms, timeout}}
  exec("git status", {{timeout: 5000}})
  sh("dir")                          through cmd /C
  a script compiles to .bxb bytecode; `bx run` executes bytecode directly

builtins
  {builtin_list}"#,
        version = VERSION,
        builtin_list = builtin_list
    );
}

fn print_builtin_help(name: &str) {
    match builtins::BUILTINS.iter().find(|b| b.name == name) {
        Some(b) => {
            println!("{}", b.sig);
            println!("  {}", b.doc);
        }
        None => {
            eprintln!("bx: no builtin named '{}'", name);
            eprintln!("run `bx help` for the full list");
        }
    }
}

fn cmd_run(args: Vec<String>) -> i32 {
    if args.is_empty() {
        eprintln!("bx run: missing script path");
        return 2;
    }
    let path = &args[0];
    let script_args: Vec<Value> = args[1..].iter().map(|s| str_val(s.clone())).collect();

    let (chunks, source_name) = match load_program(Path::new(path), false) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("bx: {}", e);
            return 1;
        }
    };

    let mut machine = Vm::new(
        chunks,
        source_name,
        script_args,
        script_dir(Path::new(path)),
    );
    match machine.run() {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("bx: {}", e);
            1
        }
    }
}

fn cmd_eval(args: Vec<String>) -> i32 {
    if args.is_empty() {
        eprintln!("bx eval: missing source");
        return 2;
    }
    let mut src = args.join(" ");
    if src == "-" {
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_err() {
            eprintln!("bx eval: cannot read stdin");
            return 1;
        }
        src = buf;
    }
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let chunks = match chunk::compile(&src, base, true) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bx: {}", e);
            return 1;
        }
    };
    let mut machine = Vm::new(chunks, "<eval>".to_string(), Vec::new(), cwd_string());
    match machine.run() {
        Ok(v) => {
            match v {
                Value::Null => {}
                other => println!("{}", other.display()),
            }
            0
        }
        Err(e) => {
            eprintln!("bx: {}", e);
            1
        }
    }
}

fn cmd_build(args: Vec<String>) -> i32 {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("bx build: -o needs a path");
                    return 2;
                }
                output = Some(args[i + 1].clone());
                i += 2;
            }
            other if input.is_none() => {
                input = Some(other.to_string());
                i += 1;
            }
            other => {
                eprintln!("bx build: unexpected argument '{}'", other);
                return 2;
            }
        }
    }
    let input = match input {
        Some(v) => v,
        None => {
            eprintln!("bx build: missing source path");
            return 2;
        }
    };
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bx build: cannot read '{}': {}", input, e);
            return 1;
        }
    };
    let base = Path::new(&input)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let chunks = match chunk::compile(&src, base, false) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bx: {}: {}", input, e);
            return 1;
        }
    };
    let bytes = chunk::encode(&chunks, false);
    let out = output.unwrap_or_else(|| {
        Path::new(&input)
            .with_extension("bxb")
            .to_string_lossy()
            .to_string()
    });
    if let Some(parent) = Path::new(&out).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    if let Err(e) = fs::write(&out, &bytes) {
        eprintln!("bx build: cannot write '{}': {}", out, e);
        return 1;
    }
    eprintln!("bx: wrote {} ({} bytes)", out, bytes.len());
    0
}

/// Compile without executing: validation for agents that want to gate a run.
fn cmd_check(args: Vec<String>) -> i32 {
    if args.is_empty() {
        eprintln!("bx check: missing script path");
        return 2;
    }
    let path = Path::new(&args[0]);
    match load_program(path, false) {
        Ok((chunks, name)) => {
            let chunks_n = chunks.len();
            println!(
                "ok {} ({} chunk{})",
                name,
                chunks_n,
                if chunks_n == 1 { "" } else { "s" }
            );
            0
        }
        Err(e) => {
            eprintln!("bx: {}", e);
            1
        }
    }
}
fn cmd_dis(args: Vec<String>) -> i32 {
    let path = match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("bx dis: missing path");
            return 2;
        }
    };
    let p = Path::new(&path);
    let chunks = if p.extension().map(|e| e == "bxb").unwrap_or(false) {
        match fs::read(p) {
            Ok(bytes) => match chunk::decode(&bytes) {
                Ok((c, _)) => c,
                Err(e) => {
                    eprintln!("bx: {}", e);
                    return 1;
                }
            },
            Err(e) => {
                eprintln!("bx dis: cannot read '{}': {}", path, e);
                return 1;
            }
        }
    } else {
        match chunk::compile_file(p, false) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("bx: {}", e);
                return 1;
            }
        }
    };
    print!("{}", chunk::disasm(&chunks));
    0
}

fn load_program(path: &Path, keep_last: bool) -> Result<(Vec<Chunk>, String), String> {
    let is_bxb = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("bxb"))
        .unwrap_or(false);

    if path.as_os_str() == "-" {
        let mut src = String::new();
        std::io::stdin()
            .read_to_string(&mut src)
            .map_err(|e| format!("cannot read stdin: {}", e))?;
        let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let chunks = chunk::compile(&src, base, keep_last)?;
        return Ok((chunks, "<stdin>".to_string()));
    }

    if is_bxb {
        let bytes =
            fs::read(path).map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
        let (chunks, _want) = chunk::decode(&bytes)?;
        return Ok((chunks, display_name(path)));
    }

    let src =
        fs::read_to_string(path).map_err(|e| format!("cannot read '{}': {}", path.display(), e))?;
    let name = display_name(path);

    if let Some(dir) = cache_dir_for(path) {
        let key = cache_key(path, &src);
        let cpath = dir.join(format!("{}.bxb", key));
        if let Ok(bytes) = fs::read(&cpath) {
            if let Ok((chunks, _)) = chunk::decode(&bytes) {
                return Ok((chunks, name));
            }
        }
        let base = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let chunks = chunk::compile(&src, base, keep_last)?;
        let bytes = chunk::encode(&chunks, keep_last);
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(&cpath, bytes);
        return Ok((chunks, name));
    }

    let base = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let chunks = chunk::compile(&src, base, keep_last)?;
    Ok((chunks, name))
}

fn cwd_string() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn script_dir(path: &Path) -> String {
    if path.as_os_str() == "-" {
        return cwd_string();
    }
    std::path::absolute(path)
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_string_lossy().to_string()))
        .unwrap_or_else(cwd_string)
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Bytecode cache lives next to the script so it works in sandboxed worktrees.
/// Override with BX_CACHE_DIR; a failed cache write is never fatal.
fn cache_dir_for(path: &Path) -> Option<PathBuf> {
    if let Ok(d) = std::env::var("BX_CACHE_DIR") {
        if !d.is_empty() {
            return Some(PathBuf::from(d));
        }
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    match parent {
        Some(p) => Some(p.join(".bxcache")),
        None => Some(PathBuf::from(".bxcache")),
    }
}

fn hash_bytes(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = (*h).wrapping_mul(0x100_0000_01b3);
    }
}

/// A cached program stays valid only while the files it `use`s are unchanged:
/// the compiler inlines them into the chunk, so the key mixes every included
/// file's bytes, recursively.
fn cache_key(path: &Path, src: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    hash_bytes(&mut h, VERSION.as_bytes());
    hash_bytes(&mut h, b"\0source\0");
    hash_bytes(&mut h, src.as_bytes());
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut seen = std::collections::HashSet::new();
    mix_includes(&mut h, &base, src, &mut seen, 0);
    format!("{:016x}", h)
}

fn mix_includes(
    h: &mut u64,
    base: &Path,
    src: &str,
    seen: &mut std::collections::HashSet<PathBuf>,
    depth: u32,
) {
    if depth > 32 {
        return;
    }
    for rel in included_paths(src) {
        let raw = Path::new(&rel);
        let full = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            base.join(raw)
        };
        let canon = std::fs::canonicalize(&full).unwrap_or(full);
        if !seen.insert(canon.clone()) {
            continue;
        }
        hash_bytes(h, b"\0include\0");
        match std::fs::read(&canon) {
            Ok(bytes) => {
                hash_bytes(h, &bytes);
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let next = canon.parent().unwrap_or(base).to_path_buf();
                    mix_includes(h, &next, text, seen, depth + 1);
                }
            }
            Err(_) => hash_bytes(h, b"\0missing\0"),
        }
    }
}

/// `use "path"` occurrences taken from the token stream, so a mention inside a
/// comment or another string cannot invalidate the cache spuriously.
fn included_paths(src: &str) -> Vec<String> {
    use crate::lexer::{lex, Tok};
    let Ok(toks) = lex(src) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pair in toks.windows(2) {
        if let (Tok::Ident(k), (Tok::Str(s), _)) = (&pair[0].0, &pair[1]) {
            if k == "use" {
                out.push(s.clone());
            }
        }
    }
    out
}
