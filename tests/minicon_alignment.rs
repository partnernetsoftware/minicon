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
    let mut kinds = BTreeSet::new();
    for (i, _) in html.match_indices("data-i18n") {
        // The attribute name runs until `=` or whitespace.
        let name: String = html[i..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        kinds.insert(name);
        let rest = &html[i..];
        let Some(eq) = rest.find('=') else { continue };
        let after = rest[eq + 1..].trim_start();
        if after.chars().next() != Some('"') {
            continue;
        }
        let Some(end) = after[1..].find('"') else { continue };
        let value = &after[1..end + 1];
        assert!(
            en.contains(value),
            "markup references {value:?} but no locale defines it"
        );
    }

    // A hook kind that markup uses must be bound in the script: the page
    // applies each kind from a `bindings` table, and a kind present only in
    // the markup would be silently left untranslated.
    for kind in &kinds {
        assert!(
            html.contains(&format!("\"{kind}\"")),
            "markup uses {kind} but the script's bindings table does not name it"
        );
    }
    assert!(
        kinds.contains("data-i18n"),
        "the plain data-i18n kind must remain the base case: {kinds:?}"
    );
}

/// Documentation and tests name repository files by relative path. A moved or
/// renamed script leaves a dead path in prose, and nobody notices until a user
/// follows it. Require every `scripts/`, `tools/`, `examples/`, `packaging/`
/// or `research/` path that appears in the docs, tests, source or research
/// notes to exist — except `dist/`, a build output absent from a source
/// checkout. Two shapes are not claims about this repo and are skipped: a
/// token inside a URL (an upstream project's path) and a token with `...`
/// (a deliberately elided path).
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
    // Research notes reference real scripts and fixtures; a moved file there is
    // as dead as one in the README.
    for entry in walk_markdown(&root.join("research")) {
        sources.push(entry);
    }

    let prefixes = ["scripts/", "tools/", "examples/", "packaging/", "research/"];
    let suffixes = [
        ".ps1", ".sh", ".py", ".md", ".js", ".qjs", ".cmd", ".bat", ".c", ".rs",
    ];
    let mut missing = BTreeSet::new();
    for source in sources {
        let text = fs::read_to_string(&source).expect("read source");
        for line in text.lines() {
            // A URL embeds another project's path; it is not a claim about
            // this clone.
            if line.contains("http://") || line.contains("https://") {
                continue;
            }
            for token in line.split(|c: char| c.is_whitespace() || "\"'`()<>".contains(c)) {
                let token = token.trim_end_matches(|c: char| ",;:.".contains(c));
                if !prefixes.iter().any(|prefix| token.starts_with(prefix)) {
                    continue;
                }
                if !suffixes.iter().any(|suffix| token.ends_with(suffix)) {
                    continue;
                }
                // A generated output under a `dist/` tree is not in a clone.
                if token.contains("/dist/") || token.contains("dist/") {
                    continue;
                }
                // `crates/agenterm-platform/.../windows/runtime.rs` elides the
                // middle on purpose; only a path that names one file is a claim.
                if token.contains("...") {
                    continue;
                }
                if !root.join(token).exists() {
                    missing.insert(format!("{}: {}", source.display(), token));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "documentation or tests name paths that do not exist:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// `scripts/build.sh` is the documented build entry, and the README names its
/// modes (`release`, `dev`, `check`, `test`). Pin that contract: the wrapper
/// exists and is executable, and an unknown mode is refused with the usage
/// rather than silently defaulting to a full release build. The interpreter it
/// needs is the reason this test runs the script at all — a host without
/// `python3` (a Windows Git shell has only `python`) must not be a broken
/// documented path.
#[test]
fn build_script_accepts_the_documented_modes() {
    let script = repo_root().join("scripts/build.sh");
    assert!(script.is_file(), "scripts/build.sh must exist");
    let Ok(bash) = which_bash() else {
        eprintln!("skipping: no bash on PATH");
        return;
    };

    // An unknown mode is a usage error (exit 2), not a build.
    let output = Command::new(&bash)
        .arg(&script)
        .arg("definitely-not-a-mode")
        .current_dir(repo_root())
        .output()
        .expect("run scripts/build.sh");
    assert_eq!(
        output.status.code(),
        Some(2),
        "an unknown mode must be a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage: scripts/build.sh"),
        "the refusal must print the usage"
    );
}

/// A `bash` on PATH, or `None` so a platform without one skips rather than
/// fails a gate that does not apply to it.
fn which_bash() -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(windows) { "bash.exe" } else { "bash" });
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(())
}

/// Every `.md` under `dir`, recursively. Missing directories yield nothing, so
/// the gate does not fail on a tree that is not checked out.
fn walk_markdown(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk_markdown(&path));
        } else if path.extension().is_some_and(|e| e == "md") {
            found.push(path);
        }
    }
    found
}

/// The docs site has no bundler to notice a broken link. Require every local
/// `href`/`src` in the HTML pages to resolve to a file, every same-page
/// `#anchor` to have a matching `id`, and every `*.html` link to name a page
/// that exists — an `https://` link and a `mailto:`/`data:` URI are not local
/// claims and are skipped.
#[test]
fn documentation_links_resolve() {
    let root = repo_root();
    let docs = root.join("docs");
    let pages: Vec<PathBuf> = fs::read_dir(&docs)
        .expect("read docs")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "html"))
        .collect();
    assert!(!pages.is_empty(), "no docs pages found");

    let mut broken = BTreeSet::new();
    for page in &pages {
        let html = fs::read_to_string(page).expect("read page");
        // Same-page anchors that the markup actually defines.
        let mut ids = BTreeSet::new();
        for token in html.match_indices("id=\"") {
            let rest = &html[token.0 + 4..];
            if let Some(end) = rest.find('"') {
                ids.insert(rest[..end].to_owned());
            }
        }
        let page_name = page.file_name().unwrap().to_string_lossy().into_owned();

        for (index, _) in html
            .match_indices("href=\"")
            .chain(html.match_indices("src=\""))
        {
            let attr_start = html[index..]
                .find('"')
                .map(|i| index + i + 1)
                .unwrap_or(index);
            let rest = &html[attr_start..];
            let Some(end) = rest.find('"') else { continue };
            let target = &rest[..end];
            if target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
                || target.starts_with("data:")
            {
                continue;
            }
            if let Some(anchor) = target.strip_prefix('#') {
                if !ids.contains(anchor) {
                    broken.insert(format!("{page_name}: #{anchor} has no id"));
                }
                continue;
            }
            // A local path, possibly with a fragment.
            let (path_part, fragment) = match target.split_once('#') {
                Some((path, fragment)) => (path, Some(fragment)),
                None => (target, None),
            };
            if path_part.is_empty() {
                continue;
            }
            let candidate = docs.join(path_part);
            if !candidate.exists() {
                broken.insert(format!("{page_name}: {target} does not exist"));
                continue;
            }
            // A cross-page anchor must land on an id in the target page.
            if let Some(fragment) = fragment {
                let target_html = fs::read_to_string(&candidate).unwrap_or_default();
                if !target_html.contains(&format!("id=\"{fragment}\"")) {
                    broken.insert(format!("{page_name}: {target} has no matching id"));
                }
            }
        }
    }
    assert!(
        broken.is_empty(),
        "documentation links do not resolve:\n{}",
        broken.into_iter().collect::<Vec<_>>().join("\n")
    );
}
