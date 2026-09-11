use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repo_root() -> PathBuf {
    if let Some(root) = std::env::var_os("MINICON_REPO_ROOT") {
        return PathBuf::from(root);
    }
    // minicon is its own repository: the package manifest directory *is* the
    // repo root, and the contract, the evidence registry and the PRD all sit
    // on it. Inside agenterm this used to have to climb out of
    // `crates/minicon/`.
    Path::new(env!("CARGO_MANIFEST_DIR")).to_owned()
}

fn load_json(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn required_str<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} must be a string in {value}"))
}

fn required_array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be an array in {value}"))
}

fn assert_exact_keys(value: &Value, expected: &[&str], context: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{context} must be an object"));
    let actual: BTreeSet<_> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<_> = expected.iter().copied().collect();
    assert_eq!(actual, expected, "unknown or missing fields in {context}");
}

#[test]
fn machine_contract_matches_public_cli_and_registered_journeys() {
    let root = repo_root();
    let package = root.clone();
    let contract = load_json(&package.join("alignment-contract.json"));
    let registry = load_json(&package.join("evidence-registry.json"));

    assert_exact_keys(
        &contract,
        &[
            "schema_version",
            "product",
            "public_commands",
            "capabilities",
        ],
        "contract",
    );
    assert_exact_keys(
        &registry,
        &["schema_version", "product", "evidence"],
        "registry",
    );

    for document in [&contract, &registry] {
        assert_eq!(document["schema_version"], 1, "unsupported schema");
        assert_eq!(document["product"], "minicon", "wrong product");
    }

    let mut registered = BTreeMap::new();
    let manifest = fs::read_to_string(package.join("Cargo.toml")).expect("read minicon manifest");
    for item in required_array(&registry, "evidence") {
        assert_exact_keys(
            item,
            &[
                "id",
                "kind",
                "emitter",
                "platform",
                "test_target",
                "test_name",
                "source",
            ],
            "evidence entry",
        );
        let id = required_str(item, "id");
        let target = required_str(item, "test_target");
        let test_name = required_str(item, "test_name");
        let source = required_str(item, "source");
        // Sources are package-relative now that the package owns the repo.
        assert!(
            !source.starts_with('/') && !source.contains(".."),
            "minicon evidence source must be package-owned: {source}"
        );
        let package_source = source;
        assert_eq!(required_str(item, "kind"), "public-black-box");
        assert_eq!(required_str(item, "emitter"), "cargo-test-harness");
        assert_eq!(required_str(item, "platform"), "windows-x86_64");
        assert_eq!(id, format!("{target}::{test_name}"));
        assert!(
            registered.insert(id, item).is_none(),
            "duplicate evidence {id}"
        );

        let source_path = root.join(source);
        let source_text = fs::read_to_string(&source_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", source_path.display()));
        assert!(
            source_text.contains(&format!("fn {test_name}()")),
            "registered test {id} is absent from {source}"
        );
        assert!(
            manifest.contains(&format!("name = \"{target}\""))
                && manifest.contains(&format!("path = \"{package_source}\"")),
            "registered target {target} and source {source} are absent from Cargo.toml"
        );
    }

    let mut capability_ids = BTreeSet::new();
    let mut referenced_evidence = BTreeSet::new();
    let mut contracted_commands = BTreeSet::new();
    let declared_commands: BTreeSet<_> = required_array(&contract, "public_commands")
        .iter()
        .map(|command| command.as_str().expect("public command must be a string"))
        .collect();
    assert_eq!(
        declared_commands.len(),
        required_array(&contract, "public_commands").len(),
        "duplicate public command"
    );
    let control_prd = fs::read_to_string(root.join("prd/PRD_02_26_con_control_cli.md"))
        .expect("read minicon control PRD");
    for capability in required_array(&contract, "capabilities") {
        assert_exact_keys(
            capability,
            &[
                "id",
                "kind",
                "status",
                "evidence_mode",
                "prd",
                "prd_anchor",
                "evidence",
                "commands",
            ],
            "capability",
        );
        let id = required_str(capability, "id");
        assert!(capability_ids.insert(id), "duplicate capability {id}");
        assert!(
            id.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || b".-".contains(&byte)),
            "invalid capability id {id}"
        );
        assert!(matches!(
            required_str(capability, "kind"),
            "behavior" | "visual" | "architecture"
        ));
        assert_eq!(required_str(capability, "status"), "shipped");
        assert_eq!(required_str(capability, "evidence_mode"), "black-box");

        let prd = required_str(capability, "prd");
        assert!(prd.starts_with("prd/PRD_02_"), "invalid PRD owner {prd}");
        let prd_text = fs::read_to_string(root.join(prd))
            .unwrap_or_else(|error| panic!("read PRD owner {prd}: {error}"));
        let anchor = required_str(capability, "prd_anchor");
        assert!(
            anchor.starts_with("- [x] "),
            "non-shipped PRD anchor for {id}"
        );
        assert!(
            prd_text.contains(anchor),
            "missing exact PRD anchor for {id}"
        );

        let evidence = required_array(capability, "evidence");
        assert!(!evidence.is_empty(), "capability {id} has no evidence");
        for evidence_id in evidence {
            let evidence_id = evidence_id
                .as_str()
                .unwrap_or_else(|| panic!("non-string evidence for {id}"));
            assert!(
                registered.contains_key(evidence_id),
                "capability {id} cites unregistered evidence {evidence_id}"
            );
            referenced_evidence.insert(evidence_id);
        }

        for command in required_array(capability, "commands") {
            let command = command
                .as_str()
                .unwrap_or_else(|| panic!("non-string command for {id}"));
            assert!(
                contracted_commands.insert(command),
                "command {command} has multiple capability owners"
            );
            assert!(
                control_prd.contains(&format!("`{command}`")),
                "public command {command} is absent from the control PRD"
            );
        }
    }
    assert!(!capability_ids.is_empty(), "contract has no capabilities");
    assert_eq!(
        referenced_evidence,
        registered.keys().copied().collect(),
        "evidence registry contains an orphan or the contract missed an entry"
    );
    assert_eq!(
        declared_commands, contracted_commands,
        "public command ownership drifted"
    );

    let contract_text = contract.to_string();
    let registry_text = registry.to_string();
    for forbidden in [
        "prd/alignment-contract.json",
        "agenterm cli",
        "agenterm.tasks.json",
    ] {
        assert!(
            !contract_text.contains(forbidden),
            "contract borrowed {forbidden}"
        );
        assert!(
            !registry_text.contains(forbidden),
            "registry borrowed {forbidden}"
        );
    }

    let executable = std::env::var_os("MINICON_TEST_BINARY")
        .map(PathBuf::from)
        .or_else(|| option_env!("CARGO_BIN_EXE_minicon").map(PathBuf::from));
    let Some(executable) = executable else {
        return;
    };
    let output = Command::new(&executable)
        .args(["cli", "list-commands"])
        .output()
        .expect("launch minicon cli list-commands");
    assert!(
        output.status.success(),
        "list-commands failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("command catalog is UTF-8");
    let public_commands: BTreeSet<_> = stdout.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        public_commands, contracted_commands,
        "machine contract and running minicon CLI catalog diverged"
    );

    // The control-CLI reference is prose, so nothing else keeps it honest.
    // Every command the build accepts must be described there.
    let reference = fs::read_to_string(repo_root().join("docs/control-cli.html"))
        .expect("read the control CLI reference");
    for command in &public_commands {
        assert!(
            reference.contains(command),
            "docs/control-cli.html does not mention the public command {command}"
        );
    }
}

/// The landing page ships three hand-maintained locale tables. A key added to
/// one and not the others renders as English (or blank) for the other two, and
/// no build step catches it — the page has no bundler. Read the source tables
/// and require identical key sets, and require every `data-i18n`/`data-i18n-*`
/// hook in the markup to name a key that all three define.
#[test]
fn landing_page_locales_define_the_same_keys() {
    let html =
        fs::read_to_string(repo_root().join("docs/index.html")).expect("read the landing page");

    /// Top-level keys of one `name: { ... }` locale table, up to the next
    /// locale marker.
    fn keys_between(html: &str, from: &str, to: Option<&str>) -> BTreeSet<String> {
        let start = html.find(from).expect("locale marker present") + from.len();
        let end = to.map_or(html.len(), |to| {
            html[start..].find(to).map_or(html.len(), |i| start + i)
        });
        let mut keys = BTreeSet::new();
        for line in html[start..end].lines() {
            let trimmed = line.trim_start();
            if let Some((key, _)) = trimmed.split_once(':') {
                let key = key.trim();
                if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    keys.insert(key.to_owned());
                }
            }
        }
        keys
    }

    let en = keys_between(&html, "en: {", Some("\"zh-CN\": {"));
    let zh_cn = keys_between(&html, "\"zh-CN\": {", Some("\"zh-Hant\": {"));
    let zh_hant = keys_between(&html, "\"zh-Hant\": {", None);
    assert!(!en.is_empty(), "the English locale table was not found");

    for (name, keys) in [("zh-CN", &zh_cn), ("zh-Hant", &zh_hant)] {
        let missing: Vec<_> = en.difference(keys).collect();
        let extra: Vec<_> = keys.difference(&en).collect();
        assert!(
            missing.is_empty(),
            "{name} is missing English keys: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "{name} has keys English does not: {extra:?}"
        );
    }

    // Every `data-i18n` and `data-i18n-*` hook must resolve in all locales.
    for (attr, value) in html.match_indices("data-i18n").filter_map(|(i, _)| {
        let rest = &html[i..];
        let eq = rest.find('=')?;
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' {
            return None;
        }
        let end = after[1..].find('"')? + 1;
        Some(("data-i18n", after[1..end].to_owned()))
    }) {
        let _ = attr;
        assert!(
            en.contains(&value),
            "markup references {value:?} but no locale defines it"
        );
    }
}

/// Documentation and tests name repository files by relative path. A moved or
/// renamed script leaves a dead path in prose, and nobody notices until a user
/// follows it. Require every `scripts/`, `tools/`, `examples/`, `packaging/`
/// or `research/` path that appears in the docs, tests or source to exist —
/// except `dist/`, which is a build output and is not in a source checkout.
#[test]
fn referenced_repository_paths_exist() {
    let root = repo_root();
    let mut sources = Vec::new();
    for dir in ["docs", "tests"] {
        for entry in fs::read_dir(root.join(dir)).expect("read directory") {
            let path = entry.expect("directory entry").path();
            if path.extension().is_some_and(|e| e == "html" || e == "rs") {
                sources.push(path);
            }
        }
    }
    sources.push(root.join("README.md"));
    if let Ok(entries) = fs::read_dir(root.join("src")) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "rs") {
                sources.push(entry.path());
            }
        }
    }

    let prefixes = ["scripts/", "tools/", "examples/", "packaging/", "research/"];
    let suffixes = [
        ".ps1", ".sh", ".py", ".md", ".js", ".qjs", ".cmd", ".bat", ".c", ".rs",
    ];
    let mut missing = BTreeSet::new();
    for source in sources {
        let text = fs::read_to_string(&source).expect("read source");
        for token in text.split(|c: char| c.is_whitespace() || "\"'`()<>".contains(c)) {
            let token = token.trim_end_matches(|c: char| ",;:.".contains(c));
            if !prefixes.iter().any(|prefix| token.starts_with(prefix)) {
                continue;
            }
            if !suffixes.iter().any(|suffix| token.ends_with(suffix)) {
                continue;
            }
            // A generated output under a `dist/` tree is not in a source clone.
            if token.contains("/dist/") || token.contains("dist/") {
                continue;
            }
            if !root.join(token).exists() {
                missing.insert(format!("{}: {}", source.display(), token));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "documentation or tests name paths that do not exist:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
}
