mod ast;
mod builtins;
mod chunk;
mod json;
mod lexer;
mod parser;
mod value;
mod vm;
mod wasm_target;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chunk::Chunk;
use value::{str_val, Value};
use vm::Vm;

/// Taken from Cargo.toml so the CLI, the cache key and the packed metadata
/// cannot drift apart.
const VERSION: &str = env!("CARGO_PKG_VERSION");

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
        "test" => cmd_test(args),
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
  bx run [--max-steps N] [--timeout MS] <script.bxs|.bxb|.wasm> [args...]
  bx build --wasm <script.bxs> [-o out.wasm]      compile the numeric subset to WebAssembly
  bx eval [--max-steps N] [--timeout MS] "<source>"          evaluate and print
  bx build <script.bxs> [-o out.bxb]         compile source to bytecode
  bx dis <script.bxs|out.bxb>                disassemble bytecode
  bx check [--json] <script.bxs|script.bxb>  compile only (no execution)
  bx test <script.bxs>                       run test(name, fn) blocks and report
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

/// Strip leading `--max-steps`/`--timeout` flags before the script or source.
/// Flags after the script stay in the script's `argv`, so existing callers do
/// not change meaning.
struct Budget {
    args: Vec<String>,
    max_steps: Option<u64>,
    timeout_ms: Option<u64>,
}

fn parse_budget(mut args: Vec<String>) -> Result<Budget, String> {
    let mut max_steps = None;
    let mut timeout_ms = None;
    loop {
        match args.first().map(String::as_str) {
            Some("--max-steps") => {
                let value = args
                    .get(1)
                    .ok_or("--max-steps needs a number")?
                    .parse::<u64>()
                    .map_err(|_| "--max-steps needs a number")?;
                max_steps = Some(value);
                args.drain(0..2);
            }
            Some("--timeout") => {
                let value = args
                    .get(1)
                    .ok_or("--timeout needs a number of milliseconds")?
                    .parse::<u64>()
                    .map_err(|_| "--timeout needs a number of milliseconds")?;
                timeout_ms = Some(value);
                args.drain(0..2);
            }
            _ => break,
        }
    }
    Ok(Budget {
        args,
        max_steps,
        timeout_ms,
    })
}

fn json_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn cmd_run(args: Vec<String>) -> i32 {
    let budget = match parse_budget(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("bx: {}", e);
            return 2;
        }
    };
    let args = budget.args;
    if let Some(steps) = budget.max_steps {
        std::env::set_var("BX_MAX_STEPS", steps.to_string());
    }
    if let Some(ms) = budget.timeout_ms {
        std::env::set_var("BX_TIMEOUT_MS", ms.to_string());
    }
    if args.is_empty() {
        eprintln!("bx run: missing script path");
        return 2;
    }
    let path = &args[0];
    if path.to_ascii_lowercase().ends_with(".wasm") {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("bx: cannot read '{}': {}", path, e);
                return 1;
            }
        };
        return match wasm_target::execute(&bytes, |value| println!("{}", value)) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("bx: {}", e);
                1
            }
        };
    }
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
    let budget = match parse_budget(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("bx: {}", e);
            return 2;
        }
    };
    let args = budget.args;
    if let Some(steps) = budget.max_steps {
        std::env::set_var("BX_MAX_STEPS", steps.to_string());
    }
    if let Some(ms) = budget.timeout_ms {
        std::env::set_var("BX_TIMEOUT_MS", ms.to_string());
    }
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
    let chunks = match chunk::compile(&src, base, "<eval>".to_string(), true) {
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
    let mut wasm = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--wasm" => {
                wasm = true;
                i += 1;
            }
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
    if wasm {
        let bytes = match wasm_target::compile_to_wasm(&src) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("bx: {}", e);
                return 1;
            }
        };
        let out = output.unwrap_or_else(|| {
            Path::new(&input)
                .with_extension("wasm")
                .to_string_lossy()
                .into_owned()
        });
        if let Some(parent) = Path::new(&out).parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(&out, &bytes) {
            eprintln!("bx build: cannot write '{}': {}", out, e);
            return 1;
        }
        eprintln!("bx: wrote {} ({} bytes, wasm)", out, bytes.len());
        return 0;
    }
    let base = Path::new(&input)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let chunks = match chunk::compile(&src, base, input.clone(), false) {
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

/// Run `test(name, fn)` blocks and report the summary.
fn cmd_test(args: Vec<String>) -> i32 {
    let budget = match parse_budget(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("bx: {}", e);
            return 2;
        }
    };
    let args = budget.args;
    if let Some(steps) = budget.max_steps {
        std::env::set_var("BX_MAX_STEPS", steps.to_string());
    }
    if let Some(ms) = budget.timeout_ms {
        std::env::set_var("BX_TIMEOUT_MS", ms.to_string());
    }
    if args.is_empty() {
        eprintln!("bx test: missing script path");
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
    if let Err(e) = machine.run() {
        eprintln!("bx: {}", e);
        return 1;
    }
    for failure in &machine.test_failures {
        eprintln!("FAIL {}", failure);
    }
    let passed = machine.tests_passed;
    let failed = machine.tests_failed;
    let total = passed + failed;
    if total == 0 {
        println!("no tests in {}", path);
        return 0;
    }
    println!(
        "{} {} test{}",
        if failed == 0 { "PASS" } else { "FAIL" },
        total,
        if total == 1 { "" } else { "s" }
    );
    if failed == 0 {
        0
    } else {
        1
    }
}
/// Compile without executing: validation for agents that want to gate a run.
/// `--json` prints one machine-readable line on stdout in both outcomes.
fn cmd_check(args: Vec<String>) -> i32 {
    let mut json_mode = false;
    let mut rest: Vec<String> = Vec::new();
    for a in args {
        if a == "--json" {
            json_mode = true;
        } else {
            rest.push(a);
        }
    }
    if rest.is_empty() {
        eprintln!("bx check: missing script path");
        return 2;
    }
    let path = Path::new(&rest[0]);
    if rest[0].to_ascii_lowercase().ends_with(".wasm") {
        let outcome = fs::read(&rest[0])
            .map_err(|e| format!("cannot read '{}': {}", rest[0], e))
            .and_then(|bytes| wasm_target::validate(&bytes));
        return match outcome {
            Ok(()) => {
                if json_mode {
                    println!(
                        "{{\"ok\":true,\"file\":{},\"target\":\"wasm\"}}",
                        json_quote(&rest[0])
                    );
                } else {
                    println!("ok {} (wasm module)", rest[0]);
                }
                0
            }
            Err(e) => {
                if json_mode {
                    println!(
                        "{{\"ok\":false,\"file\":{},\"error\":{}}}",
                        json_quote(&rest[0]),
                        json_quote(&e)
                    );
                } else {
                    eprintln!("bx: {}", e);
                }
                1
            }
        };
    }
    match load_program(path, false) {
        Ok((chunks, name)) => {
            let chunks_n = chunks.len();
            if json_mode {
                println!(
                    "{{\"ok\":true,\"file\":{},\"chunks\":{}}}",
                    json_quote(&name),
                    chunks_n
                );
            } else {
                println!(
                    "ok {} ({} chunk{})",
                    name,
                    chunks_n,
                    if chunks_n == 1 { "" } else { "s" }
                );
            }
            0
        }
        Err(e) => {
            if json_mode {
                println!(
                    "{{\"ok\":false,\"file\":{},\"error\":{}}}",
                    json_quote(&rest[0]),
                    json_quote(&e)
                );
            } else {
                eprintln!("bx: {}", e);
            }
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
        let chunks = chunk::compile(&src, base, "<stdin>".to_string(), keep_last)?;
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
        let chunks = chunk::compile(&src, base, path.display().to_string(), keep_last)?;
        let bytes = chunk::encode(&chunks, keep_last);
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(&cpath, bytes);
        return Ok((chunks, name));
    }

    let base = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let chunks = chunk::compile(&src, base, path.display().to_string(), keep_last)?;
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
    // Bytecode layout changes must not be served from an older cache entry.
    hash_bytes(&mut h, b"\0bxb2\0");
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
