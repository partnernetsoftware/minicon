use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Attribute, ItemFn, Macro, Meta, Token};

#[test]
fn no_platform_dependency_creeps_in() {
    let manifest = include_str!("../../Cargo.toml").replace("\r\n", "\n");
    let dependencies = manifest
        .split("\n[dependencies]\n")
        .nth(1)
        .expect("a dependencies section")
        .split("\n[")
        .next()
        .expect("a bounded section");
    for forbidden in [
        "minicon-platform",
        "minicon-ui-core",
        "windows-sys",
        "winit",
        "softbuffer",
        "vt100",
        "libc",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "{forbidden} would tie this crate to a host; it belongs above this layer"
        );
    }
}

fn source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read core source directory") {
        let entry = entry.expect("read core source entry");
        let path = entry.path();
        if entry.file_type().expect("read core source type").is_dir() {
            source_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[derive(Default)]
struct TargetSelectors {
    offenders: Vec<String>,
    legacy_review_file: bool,
    function: Option<String>,
}

impl TargetSelectors {
    fn inspect(&mut self, meta: &Meta) {
        if let Some(name) = meta.path().get_ident() {
            let name = name.to_string();
            if name == "windows" || name == "unix" || name.starts_with("target_") {
                // Freeze the existing one-argument API's Windows newline
                // behavior and its test. No other selector/value is exempt.
                let legacy_review = self.legacy_review_file
                    && matches!(
                        self.function.as_deref(),
                        Some("paste_review_display_text")
                            | Some(
                                "paste_review_display_uses_the_host_editor_newline_so_it_paints_rows"
                            )
                    )
                    && name == "target_os"
                    && matches!(meta, Meta::NameValue(value)
                        if matches!(&value.value, syn::Expr::Lit(value)
                            if matches!(&value.lit, syn::Lit::Str(value) if value.value() == "windows")));
                if !legacy_review {
                    self.offenders.push(name);
                }
            }
        }
        if let Meta::List(list) = meta {
            for nested in list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .expect("parse conditional metadata")
            {
                self.inspect(&nested);
            }
        }
    }
}

impl<'ast> Visit<'ast> for TargetSelectors {
    fn visit_item_fn(&mut self, function: &'ast ItemFn) {
        let previous = self.function.replace(function.sig.ident.to_string());
        syn::visit::visit_item_fn(self, function);
        self.function = previous;
    }

    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        if attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr") {
            self.inspect(&attribute.meta);
        }
    }

    fn visit_macro(&mut self, expression: &'ast Macro) {
        if expression.path.is_ident("cfg") {
            self.inspect(&syn::parse2(expression.tokens.clone()).expect("parse cfg! predicate"));
        }
    }
}

#[test]
fn core_source_target_conditionals_stay_within_the_legacy_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    source_files(&root.join("src"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "source boundary must inspect real files");
    let mut offenders = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path).expect("read core Rust source");
        let syntax = syn::parse_file(&source).expect("parse core Rust source");
        let mut selectors = TargetSelectors {
            legacy_review_file: path == root.join("src/composer.rs"),
            ..TargetSelectors::default()
        };
        selectors.visit_file(&syntax);
        for selector in selectors.offenders {
            offenders.push(format!(
                "{}: {selector}",
                path.strip_prefix(root)
                    .expect("core-relative path")
                    .display()
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "core contains target-dependent conditionals: {offenders:#?}"
    );
}
