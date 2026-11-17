//! The absorbed `tinyvm` core as bx's wasm engine (PRD_02_44 P1).
//!
//! bx emits standard `.wasm`; this module decodes, validates and interprets it
//! with the vendored core. It is the only wasm engine in the binary: P1 deleted
//! the in-tree interpreter after the core passed the differential (the
//! interpreter silently accepted modules the core rejects). The host functions
//! stay in [`crate::wasm`] as the existing `PrintHost` arms, reached through
//! one adapter, so the ABI keeps a single implementation.

use crate::wasm::{BoxedI64Sink, BoxedStrSink, WVal};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tinyvm::wasm::{ImportDesc, Module, Val};
use tinyvm::{FunctionSite, Limits, LoadError, WasmError};

/// Host error prefix that carries a requested process exit code.
pub const EXIT_MARKER: &str = "\u{0}bx-exit:";

fn to_wval(value: &Val) -> Result<WVal, WasmError> {
    match value {
        Val::I32(v) => Ok(WVal::I32(*v)),
        Val::I64(v) => Ok(WVal::I64(*v)),
        Val::F32(v) => Ok(WVal::F32(*v)),
        Val::F64(v) => Ok(WVal::F64(*v)),
        _ => Err(WasmError::Trap("bx host expects numeric values")),
    }
}

fn from_wval(value: WVal) -> Val {
    match value {
        WVal::I32(v) => Val::I32(v),
        WVal::I64(v) => Val::I64(v),
        WVal::F32(v) => Val::F32(v),
        WVal::F64(v) => Val::F64(v),
    }
}

fn site_text(site: &FunctionSite) -> String {
    let mut text = format!(" in function #{}", site.index);
    if let Some(name) = &site.name {
        text.push_str(&format!(" `{}`", name));
    }
    if let Some(line) = site.line {
        text.push_str(&format!(" (line {}", line));
        if let Some(column) = site.column {
            text.push_str(&format!(", column {}", column));
        }
        text.push(')');
    }
    text
}

fn load_error_text(err: &LoadError) -> String {
    let mut text = format!("wasm module rejected: {}", err.error.message());
    if let Some(site) = &err.function {
        text.push_str(&site_text(site));
    }
    text
}

fn trap_text(err: &WasmError) -> String {
    let message = err.message();
    if let Some(ceiling) = err.ceiling() {
        format!("wasm execution stopped: {} ({:?})", message, ceiling)
    } else {
        format!("wasm execution stopped: {}", message)
    }
}

/// Import names declared by the module, de-duplicated in declaration order.
fn import_names(module: &Module) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = Vec::new();
    for ImportDesc { module, field, .. } in module.imports() {
        let pair = (module.clone(), field.clone());
        if !names.contains(&pair) {
            names.push(pair);
        }
    }
    names
}

/// Decode and validate a module with the absorbed core.
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    Module::from_bytes_explained(bytes, Limits::default())
        .map(|_| ())
        .map_err(|err| load_error_text(&err))
}

/// Execute the module's `_start` export through the absorbed core.
// Kept as the convenience wrapper for callers that have no stderr sink; the CLI
// uses `run_start_with_stderr` directly.
#[allow(dead_code)]
pub fn run_start<F, G>(
    bytes: &[u8],
    argv: &[String],
    max_steps: u64,
    timeout_ms: u64,
    on_i64: F,
    on_str: G,
) -> Result<i64, String>
where
    F: FnMut(i64) + 'static,
    G: FnMut(&str) + 'static,
{
    run_start_with_stderr(
        bytes,
        argv,
        max_steps,
        timeout_ms,
        on_i64,
        on_str,
        |_: &str| {},
    )
}

/// Like [`run_start`], with a sink for `env.eprint` lines (stderr).
pub fn run_start_with_stderr<F, G, H>(
    bytes: &[u8],
    argv: &[String],
    max_steps: u64,
    timeout_ms: u64,
    on_i64: F,
    on_str: G,
    on_err: H,
) -> Result<i64, String>
where
    F: FnMut(i64) + 'static,
    G: FnMut(&str) + 'static,
    H: FnMut(&str) + 'static,
{
    let limits = Limits {
        max_steps: if max_steps > 0 { max_steps } else { u64::MAX },
        max_memory_pages: 64,
        ..Limits::default()
    };
    let mut module =
        Module::from_bytes_explained(bytes, limits).map_err(|err| load_error_text(&err))?;

    let host_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let error_slot = host_error.clone();
    let sink_i64: BoxedI64Sink = Box::new(on_i64);
    let sink_str: BoxedStrSink = Box::new(on_str);
    let sink_err: BoxedStrSink = Box::new(on_err);
    let host = Rc::new(RefCell::new(crate::wasm::core_print_host(
        sink_i64, sink_str, sink_err, argv,
    )));

    for (module_name, field) in import_names(&module) {
        let host = host.clone();
        let call_module = module_name.clone();
        let call_field = field.clone();
        let error_slot = error_slot.clone();
        module
            .bind_import_typed(&module_name, &field, move |args, memory| {
                let wargs: Vec<WVal> = args
                    .iter()
                    .map(to_wval)
                    .collect::<Result<Vec<_>, WasmError>>()?;
                let mut host = host.borrow_mut();
                let results = match host.dispatch(&call_module, &call_field, &wargs, memory) {
                    Ok(results) => results,
                    Err(message) => {
                        *error_slot.borrow_mut() = Some(message);
                        return Err(WasmError::Trap("bx host error"));
                    }
                };
                Ok(results.into_iter().map(from_wval).collect())
            })
            .map_err(|err| format!("binding {}.{}: {}", module_name, field, trap_text(&err)))?;
    }

    let interrupt = Arc::new(AtomicBool::new(false));
    if timeout_ms > 0 {
        let flag = interrupt.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms));
            flag.store(true, Ordering::Relaxed);
        });
    }

    let mut instance = module
        .instantiate_with_interrupt(interrupt.as_ref())
        .map_err(|err| trap_text(&err))?;
    if let Err(err) = instance.invoke_by_name_with_interrupt("_start", &[], interrupt.as_ref()) {
        if let Some(message) = host_error.borrow().clone() {
            if let Some(code) = message.strip_prefix(EXIT_MARKER) {
                return Ok(code.parse::<i64>().unwrap_or(1));
            }
            return Err(message);
        }
        return Err(trap_text(&err));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_module_with_mismatched_local_types() {
        // A minimal module whose function declares an i64 local but writes an
        // i32 into it. The in-tree interpreter used to accept this class; the
        // core must reject it at validation time.
        let bytes: Vec<u8> = vec![
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic + version
            0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type: () -> ()
            0x03, 0x02, 0x01, 0x00, // function section: one function, type 0
            0x0a, 0x09, 0x01, 0x07, 0x01, 0x01, 0x7e, 0x41, 0x00, 0x21, 0x00,
            0x0b, // code: 1 local i64; i32.const 0; local.set 0; end
        ];
        let error = validate(&bytes).expect_err("invalid module must be rejected");
        assert!(
            error.contains("rejected") || error.contains("mismatch"),
            "unexpected message: {error}"
        );
    }

    #[test]
    fn runs_a_compiled_bx_module() {
        let source = "print(1 + 2)\nprint(\"hi\")";
        let bytes = crate::wasm_target::compile_to_wasm(source).expect("compile");
        let numbers = Rc::new(RefCell::new(Vec::new()));
        let strings = Rc::new(RefCell::new(Vec::new()));
        let seen_i = numbers.clone();
        let seen_s = strings.clone();
        run_start(
            &bytes,
            &[],
            0,
            0,
            move |value| seen_i.borrow_mut().push(value),
            move |text| seen_s.borrow_mut().push(text.to_string()),
        )
        .expect("run through the core");
        assert_eq!(*numbers.borrow(), vec![3]);
        assert_eq!(*strings.borrow(), vec!["hi".to_string()]);
    }
}
