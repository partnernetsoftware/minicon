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
    let mut claimed_anchors = BTreeSet::new();
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
            anchor.starts_with("- [v] "),
            "non-shipped PRD anchor for {id}"
        );
        assert!(
            prd_text.contains(anchor),
            "missing exact PRD anchor for {id}"
        );
        // A capability ships a `[v]` assertion. The same assertion text must not
        // also appear as `[-]` (in progress) or `[_]` (not started) elsewhere in
        // the PRD — a contradictory duplicate would let the contract claim
        // shipped while the PRD still lists the work as outstanding.
        let assertion = &anchor["- [v] ".len()..];
        for state in ["- [-] ", "- [_] "] {
            assert!(
                !prd_text.contains(&format!("{state}{assertion}")),
                "{id} claims the statement shipped, but {prd} also lists it as \
                 {state:?}: {assertion}"
            );
        }
        // One PRD assertion belongs to one capability. A duplicated anchor
        // would let a second capability cite evidence that does not actually
        // cover the assertion it claims to ship.
        assert!(
            claimed_anchors.insert(anchor.to_owned()),
            "PRD anchor {anchor:?} is claimed by more than one capability"
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
        if !after.starts_with('"') {
            continue;
        }
        let Some(end) = after[1..].find('"') else {
            continue;
        };
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
/// A local path override must never reach a commit.
///
/// Verifying a platform-crate change means pointing the dependency at a working
/// copy, which is correct while it is local and invisible once it is pushed:
/// the machine that made the change still builds, and CI fails with "failed to
/// load source for dependency" on a directory it has never had. That happened
/// during 0.1.20 and cost a build round; nothing but this notices.
/// Every self-test is reachable from the one entrypoint that runs them.
///
/// Seven `*-selftest` scripts existed with nothing invoking them — not
/// `build.sh`, not six-cell, not CI, not a line in `AGENTS.md`. They all
/// passed when finally run, which is the point: a dormant check is
/// indistinguishable from a working one until something it guards ships. This
/// keeps a new one from joining them silently.
/// The built `minicon`, for gates that must ask the product rather than read a
/// source file. Same resolution the black-box suites use.
fn minicon_binary() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("MINICON_TEST_BINARY") {
        let path = std::path::PathBuf::from(path);
        assert!(
            path.is_file(),
            "MINICON_TEST_BINARY is missing at {}",
            path.display()
        );
        return path;
    }
    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();
    path.pop();
    path.push(format!("minicon{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.is_file(),
        "minicon is missing at {}; build it with `cargo build --bin minicon`",
        path.display()
    );
    path
}

#[cfg(not(feature = "native-auth-court"))]
#[test]
fn default_product_refuses_the_native_authorization_court_entry_point() {
    let output = Command::new(minicon_binary())
        .args(["--native-auth-court", "unused-native-court-manifest.json"])
        .output()
        .expect("ask default product to reject the native court argument");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown argument '--native-auth-court'")
    );
}

#[test]
fn every_selftest_script_is_listed_in_the_selftest_entrypoint() {
    let root = repo_root();
    let entrypoint =
        fs::read_to_string(root.join("scripts/selftest.sh")).expect("read scripts/selftest.sh");
    let mut missing = BTreeSet::new();
    for name in tracked_files(&root) {
        if !name.contains("selftest") || name == "scripts/selftest.sh" {
            continue;
        }
        if !name.ends_with(".sh") && !name.ends_with(".py") {
            continue;
        }
        if !entrypoint.contains(&name) {
            missing.insert(name);
        }
    }
    assert!(
        missing.is_empty(),
        "self-tests that nothing runs: {missing:?}; add them to scripts/selftest.sh"
    );
}

#[test]
fn the_manifest_never_ships_a_local_path_override() {
    let root = repo_root();
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    for (index, line) in manifest.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        assert!(
            !(trimmed.contains("path = \"..") || trimmed.contains("path = \"/")),
            "Cargo.toml line {} points a dependency outside this clone: {trimmed}",
            index + 1
        );
    }
    assert!(
        !manifest.contains("[patch.\"https://github.com/partnernetsoftware/agenterm\"]"),
        "Cargo.toml still patches the agenterm git source; that is a local-only override"
    );
}

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
    // The README is split by language: `README.md` is the shared entry, and the
    // two language files carry the prose whose links must still resolve.
    sources.push(root.join("README.md"));
    sources.push(root.join("README.en.md"));
    sources.push(root.join("README.zh.md"));
    // `AGENTS.md` names the same scripts and paths and is not prose the
    // referenced-path gate used to read; include it so its commands stay real.
    sources.push(root.join("AGENTS.md"));
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
    // The product's own memory palace was not covered, and a reference there
    // rots the same way: an owning PRD module pointed at a plan for releases
    // after that plan was archived, and nothing caught it. These are the
    // documents an agent is told to start from, so a dead link in them costs
    // more than one in prose, not less.
    //
    // `archive/` is deliberately excluded. A release history records what was
    // true at the time — including scripts since renamed — and rewriting that
    // to keep a link alive would falsify the record. Live documents are the
    // ones that must still resolve.
    sources.push(root.join("PRD.md"));
    for directory in ["prd", "plan"] {
        for entry in walk_markdown(&root.join(directory)) {
            if entry.components().any(|part| part.as_os_str() == "archive") {
                continue;
            }
            sources.push(entry);
        }
    }

    let prefixes = ["scripts/", "tools/", "examples/", "packaging/", "research/"];
    let suffixes = [
        ".ps1", ".sh", ".py", ".md", ".js", ".qjs", ".cmd", ".bat", ".c", ".rs",
    ];
    let mut missing = BTreeSet::new();
    let tracked = tracked_files(&root);
    let mut mismatched = BTreeSet::new();
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
                // `./scripts/build.sh` is the same claim as `scripts/build.sh`,
                // and the prefix test below would otherwise skip it.
                let token = token.strip_prefix("./").unwrap_or(token);
                // A markdown link is a claim about a repository file whether or
                // not it sits under one of the scanned directories: `PRD.md` and
                // `plan/*.md` are as breakable as `scripts/*.sh`. A `~`-relative
                // path points outside this clone and is not a claim about it.
                let markdown = token.ends_with(".md") && token != ".md" && !token.starts_with('~');
                if !markdown && !prefixes.iter().any(|prefix| token.starts_with(prefix)) {
                    continue;
                }
                if !suffixes.iter().any(|suffix| token.ends_with(suffix)) {
                    continue;
                }
                // A generated output under a `dist/` tree is not in a clone.
                if token.contains("/dist/") || token.contains("dist/") {
                    continue;
                }
                // `crates/minicon-platform/.../windows/runtime.rs` elides the
                // middle on purpose; only a path that names one file is a claim.
                if token.contains("...") {
                    continue;
                }
                // `scripts/*-utm-runner.sh` names a family of files, not one
                // path; a pattern is not a claim that a literal file exists.
                if token.contains('*') {
                    continue;
                }
                // A link may be written relative to the repository root or to
                // the file that contains it; accept either, so a document's own
                // sibling links are not reported as dead.
                let from_root = root.join(token).exists();
                let from_file = source
                    .parent()
                    .map(|dir| dir.join(token).exists())
                    .unwrap_or(false);
                if !from_root && !from_file {
                    missing.insert(format!("{}: {}", source.display(), token));
                } else if token.contains('/') {
                    // A token with a separator is a path claim worth a case
                    // check; a bare filename mentioned in prose with no
                    // separator already had to resolve on this filesystem
                    // above (`from_root`/`from_file`), on Linux where that
                    // means matching case, so it needs no second check here.
                    // Resolve the token the way the OS would, then require
                    // the result to be spelled as git tracks it, because
                    // this filesystem may be case-insensitive while Linux CI
                    // is not.
                    let resolved = source.parent().map(|dir| normalize_path(&dir.join(token)));
                    let as_root = normalize_path(&root.join(token));
                    let candidate = resolved.filter(|path| path.exists()).unwrap_or(as_root);
                    if candidate.exists()
                        && !tracked
                            .iter()
                            .any(|entry| normalize_path(&root.join(entry)) == candidate)
                    {
                        mismatched.insert(format!("{}: {}", source.display(), token));
                    }
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "documentation or tests name paths that do not exist:\n{}",
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
    assert!(
        mismatched.is_empty(),
        "documentation or tests name paths whose spelling differs from git, \
         which resolves here but not on a case-sensitive checkout:\n{}",
        mismatched.into_iter().collect::<Vec<_>>().join("\n")
    );
}

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
    let tracked = tracked_files(&root);
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
            // The link resolves, but this filesystem may be case-insensitive:
            // a wrong-case `href` would still 404 on a case-sensitive host. The
            // pages are served from a checkout, so a file's spelling must match
            // what git records. A directory target (`href="./"`) is tracked as
            // its files, not as an entry of its own, so it is exempt.
            if candidate.is_file()
                && !tracked
                    .iter()
                    .any(|entry| normalize_path(&root.join(entry)) == normalize_path(&candidate))
            {
                broken.insert(format!(
                    "{page_name}: {target} is spelled differently from the tracked file"
                ));
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

/// Only the landing page carries locale tables and the binding script; the
/// reference pages are English by design. A `data-i18n` hook pasted into a
/// page without that machinery would render untranslated and never be caught,
/// so require each page to be wholly one or the other: no hooks at all, or the
/// full machinery that applies them.
#[test]
fn every_docs_page_is_wholly_localized_or_wholly_english() {
    let docs = repo_root().join("docs");
    let mut pages: Vec<PathBuf> = fs::read_dir(&docs)
        .expect("read docs")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "html"))
        .collect();
    pages.sort();
    assert!(!pages.is_empty(), "no docs pages found");

    for page in pages {
        let name = page.file_name().unwrap().to_string_lossy().into_owned();
        let html = fs::read_to_string(&page).expect("read page");
        let hooks = html.matches("data-i18n").count();
        let has_tables = html.contains("data-locale=") || html.contains("\"zh-CN\":");
        let has_binding = html.contains("document.querySelectorAll(`[${attribute}]`)")
            || html.contains("data-i18n-html");
        if hooks == 0 {
            assert!(
                !has_tables,
                "{name} has locale tables but no data-i18n hooks to apply them"
            );
            continue;
        }
        assert!(
            has_tables,
            "{name} has {hooks} data-i18n hooks but no locale tables to resolve them"
        );
        assert!(
            has_binding,
            "{name} has {hooks} data-i18n hooks but no script that applies them"
        );
    }
}

/// The shortcut list appears twice: in `minicon --help` and as the README's key
/// table. They are prose, so one can gain a chord the other lacks and no
/// behaviour test notices — which is how the README came to document `Up`/`Down`
/// recall that the help text never listed. Compare the chord names as sets after
/// stripping the formatting each uses.
#[test]
fn the_help_and_readme_list_the_same_shortcuts() {
    let root = repo_root();
    // Read the help from the built binary, not from a source file.
    //
    // This gate used to grep `src/main.rs` for sentinel strings, so moving the
    // usage text into `src/cli.rs` — a pure code move with no behaviour change —
    // failed it. A gate on what the product *says* should ask the product; one
    // that asks a particular file is a gate on where the code happens to live,
    // and it will break again on the next move.
    let binary = minicon_binary();
    let output = std::process::Command::new(&binary)
        .arg("--help")
        .output()
        .expect("run minicon --help");
    assert!(output.status.success(), "minicon --help failed");
    let source = String::from_utf8(output.stdout).expect("help output is UTF-8");
    let readme = fs::read_to_string(root.join("README.en.md")).expect("read README");

    // The help block: lines shaped `<chord>  <description>` between the
    // "Keys use names such as" paragraph and the trailing click sentence.
    let start = source
        .find("Keys use names such as Enter")
        .expect("the help documents key naming");
    let block = &source[start..];
    let end = block
        .find("Click a tab to select it")
        .expect("the help block ends with the click sentence");
    let block = &block[..end];
    let mut help: BTreeSet<String> = BTreeSet::new();
    for line in block.lines() {
        let Some(chord) = line.split_whitespace().next() else {
            continue;
        };
        // A chord is either a modifier form or a bare named key that the help
        // uses as a shortcut (the navigation keys are named without modifiers).
        if chord.contains('+') || matches!(chord, "Enter" | "Escape" | "Tab" | "Up") {
            help.insert(chord.to_owned());
        }
    }

    // The README table: first cell of each row, minus the backticks and with
    // the combined forms kept as they are written.
    let table = readme
        .split_once("| Key |")
        .expect("the README has a key table")
        .1;
    let table = table.split("\n\n").next().unwrap_or(table);
    let mut documented: BTreeSet<String> = BTreeSet::new();
    for line in table.lines() {
        let Some(cell) = line.strip_prefix('|') else {
            continue;
        };
        let Some((cell, _)) = cell.split_once('|') else {
            continue;
        };
        let cell = cell.trim().replace('`', "");
        if cell == "Key" || cell.starts_with('-') || cell.is_empty() {
            continue;
        }
        // A combined row such as `Ctrl+Shift+[` / `]` describes two chords of
        // one family. Key the row by its first chord so both spellings of the
        // bracket pair reduce to the same token.
        let first = cell.split_whitespace().next().unwrap_or(&cell);
        documented.insert(first.to_owned());
    }

    assert!(!help.is_empty(), "no chords were parsed from the help text");
    assert!(
        !documented.is_empty(),
        "no chords were parsed from the README table"
    );
    // `Up` / `Down` is one README row for two keys; the help lists it the same
    // way, so compare after collapsing whitespace around the slash.
    let collapse = |set: &BTreeSet<String>| -> BTreeSet<String> {
        set.iter()
            .map(|chord| chord.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    };
    let help = collapse(&help);
    let documented = collapse(&documented);
    let only_help: Vec<&String> = help.difference(&documented).collect();
    let only_readme: Vec<&String> = documented.difference(&help).collect();
    assert!(
        only_help.is_empty(),
        "--help lists shortcuts the README does not: {only_help:?}"
    );
    assert!(
        only_readme.is_empty(),
        "the README lists shortcuts --help does not: {only_readme:?}"
    );
}

/// The landing page's scripting section names CLI commands in two structured
/// places: `minicon cli … <verb>` inside a code block, and the `<dt>` lists that
/// follow it. Neither is checked by anything — the existing reference gate reads
/// `docs/control-cli.html` only — so a renamed command would leave the landing
/// page advertising something the build no longer accepts. Take the command set
/// from the catalog the binary publishes and require every command named in
/// those structured places to be a member.
#[test]
fn the_landing_page_names_only_real_cli_commands() {
    let root = repo_root();
    let source = fs::read_to_string(root.join("src/control.rs")).expect("read control.rs");
    let raw = source
        .split_once("CLI_COMMAND_CATALOG: &str = \"")
        .expect("the command catalog exists")
        .1;
    let raw = raw.split_once("\";").expect("the catalog ends").0;
    let mut catalog: BTreeSet<&str> = raw.split(r"\n").filter(|line| !line.is_empty()).collect();
    // The catalog is the dispatch list plus `list-commands`, which is handled
    // before dispatch; both are public spellings.
    catalog.insert("list-commands");

    let page = fs::read_to_string(root.join("docs/index.html")).expect("read the landing page");
    let mut named: Vec<String> = Vec::new();

    // `minicon cli --control <endpoint> <verb> …`, allowing markup between.
    let mut rest = page.as_str();
    while let Some(index) = rest.find("minicon cli ") {
        let line = &rest[index..];
        let line = &line[..line.find('\n').unwrap_or(line.len())];
        // The verb follows the endpoint argument.
        if let Some(command_index) = line.find("control.sock ") {
            let tail = &line[command_index + "control.sock ".len()..];
            let verb: String = tail
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                .collect();
            if !verb.is_empty() {
                named.push(verb);
            }
        }
        rest = &rest[index + "minicon cli ".len()..];
    }

    // `<dt>a · b · c</dt>` command lists.
    let mut rest = page.as_str();
    while let Some(index) = rest.find("<dt>") {
        let after = &rest[index + "<dt>".len()..];
        let Some(end) = after.find("</dt>") else {
            break;
        };
        for token in after[..end].split('·') {
            let token = token.trim();
            if !token.is_empty() {
                named.push(token.to_owned());
            }
        }
        rest = &after[end..];
    }

    assert!(
        named.len() >= 10,
        "the landing page should name several commands, found {named:?}"
    );
    let unknown: Vec<&String> = named
        .iter()
        .filter(|name| !catalog.contains(name.as_str()))
        .collect();
    assert!(
        unknown.is_empty(),
        "docs/index.html names commands the build does not accept: {unknown:?}"
    );
}

/// `AGENTS.md` tells contributors to run the gate through `./scripts/build.sh
/// test` because that path denies the lints which let a test run nothing. Keep
/// the instruction and the mechanism tied: if the script stops denying them, or
/// the document stops naming them, the rule has become folklore.
/// Lexically normalise a path: drop `.` components and fold `..` against the
/// preceding component. It does not touch the filesystem, so it works for a
/// path whose spelling may not match the disk.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalised = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalised.pop();
            }
            other => normalised.push(other.as_os_str()),
        }
    }
    normalised
}

/// Every path git tracks, with `/` separators. These are the names a
/// case-sensitive checkout sees, so a link must be spelled as they are: this
/// host's filesystem may resolve a wrong case that Linux CI would not.
fn tracked_files(root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .arg("ls-files")
        .output()
        .expect("run git ls-files");
    let tracked: Vec<String> = String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(!tracked.is_empty(), "git ls-files returned nothing");
    tracked
}

/// Every manifest `path = "…"` and every asset `build.rs` reads must exist with
/// the spelling git records. These are build inputs: a wrong case works on this
/// filesystem and fails a case-sensitive checkout, and a missing one fails only
/// on the platform that needs it (the Linux vendor libraries, the macOS plist).
/// A manifest's paths are relative to that manifest's own directory, not the
/// repository root.
#[test]
fn manifest_and_build_asset_paths_exist_with_the_tracked_spelling() {
    let root = repo_root();
    let tracked = tracked_files(&root);
    let mut broken = BTreeSet::new();

    let check = |base: &Path, token: &str, broken: &mut BTreeSet<String>| {
        let candidate = normalize_path(&base.join(token));
        if !candidate.exists() {
            broken.insert(format!("{}: {token} does not exist", base.display()));
            return;
        }
        if candidate.is_file()
            && !tracked
                .iter()
                .any(|entry| normalize_path(&root.join(entry)) == candidate)
        {
            broken.insert(format!(
                "{}: {token} is spelled differently from the tracked file",
                base.display()
            ));
        }
    };

    // Manifests: `path = "…"` entries, each relative to the manifest itself.
    let mut manifests = vec![root.join("Cargo.toml")];
    if let Ok(entries) = fs::read_dir(root.join("crates")) {
        for entry in entries.flatten() {
            let manifest = entry.path().join("Cargo.toml");
            if manifest.exists() {
                manifests.push(manifest);
            }
        }
    }
    assert!(manifests.len() >= 2, "expected more than one manifest");
    for manifest in manifests {
        let text = fs::read_to_string(&manifest).expect("read manifest");
        let base = manifest.parent().expect("a manifest has a directory");
        // A leaf manifest legitimately declares no `path = "…"` entries — the
        // vendored platform crates have no in-repo path dependencies — so an
        // empty scan is a pass, not a failure. Only a manifest that failed to
        // read would leave nothing to check.
        assert!(
            !text.trim().is_empty(),
            "{} is empty; the scan found nothing to check",
            manifest.display()
        );
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("path = \"") else {
                continue;
            };
            let Some((token, _)) = rest.split_once('"') else {
                continue;
            };
            check(base, token, &mut broken);
        }
    }

    // Build assets: the quoted relative paths `build.rs` reads.
    let build = fs::read_to_string(root.join("build.rs")).expect("read build.rs");
    let mut assets = 0usize;
    for token in ["assets/minicon.ico", "assets/macos-info.plist"] {
        if build.contains(token) {
            assets += 1;
            check(&root, token, &mut broken);
        }
    }
    assert!(assets >= 2, "build.rs must name its embedded assets");
    // The Linux vendor libraries are composed from an arch directory.
    for (arch, soname) in [
        ("x86_64", "libxkbcommon-x11.so.0"),
        ("x86_64", "libxcb-xkb.so.1"),
        ("aarch64", "libxkbcommon-x11.so.0"),
        ("aarch64", "libxcb-xkb.so.1"),
    ] {
        check(&root, &format!("vendor/linux/{arch}/{soname}"), &mut broken);
    }

    assert!(
        broken.is_empty(),
        "build inputs are missing or wrongly spelled:\n{}",
        broken.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// The workflows are a blind spot: a broken reference or an unpinned action is
/// only discovered after a push, on someone else's machine. Require every
/// `uses:` to be pinned to a full 40-character commit SHA — a mutable tag or a
/// truncated hash can change under the build — and every repository path a
/// workflow names to exist, excluding `dist/`, which is a build output.
#[test]
fn workflow_references_are_pinned_and_exist() {
    let root = repo_root();
    let tracked = tracked_files(&root);
    let workflows = root.join(".github/workflows");
    let mut broken = BTreeSet::new();
    let mut pins = 0usize;

    let mut files: Vec<PathBuf> = fs::read_dir(&workflows)
        .expect("read workflows")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yml" || extension == "yaml")
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no workflows found");

    for workflow in files {
        let name = workflow
            .file_name()
            .expect("a workflow has a name")
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(&workflow).expect("read workflow");
        for line in text.lines() {
            let line = line.trim();

            // Action pins: `uses: owner/repo@ref`, or a local `./path`.
            if let Some(rest) = line
                .strip_prefix("- uses: ")
                .or_else(|| line.strip_prefix("uses: "))
            {
                let reference = rest.split_whitespace().next().unwrap_or("");
                if reference.starts_with("./") {
                    // A local composite action is a path in this repository.
                    let candidate = normalize_path(&root.join(reference.trim_start_matches("./")));
                    if !candidate.exists() {
                        broken.insert(format!("{name}: local action {reference} does not exist"));
                    }
                } else {
                    pins += 1;
                    let Some((action, reference)) = reference.rsplit_once('@') else {
                        broken.insert(format!("{name}: {reference} is not pinned to a revision"));
                        continue;
                    };
                    if reference.len() != 40 || !reference.bytes().all(|b| b.is_ascii_hexdigit()) {
                        broken.insert(format!(
                            "{name}: {action}@{reference} is not a full 40-character commit SHA"
                        ));
                    }
                }
            }

            // Repository paths named anywhere in the file.
            for token in text_paths(line) {
                if token.contains("/dist/") || token.contains("dist/") {
                    continue;
                }
                if !root.join(&token).exists() {
                    broken.insert(format!("{name}: {token} does not exist"));
                }
            }
        }
    }

    assert!(pins >= 5, "expected several action pins, found {pins}");
    assert!(
        broken.is_empty(),
        "workflow references are unpinned or missing:\n{}",
        broken.into_iter().collect::<Vec<_>>().join("\n")
    );
    let _ = tracked;
}

/// Repository-relative path tokens in one line of a workflow.
fn text_paths(line: &str) -> Vec<String> {
    let prefixes = [
        "scripts/",
        "tests/",
        "src/",
        "crates/",
        "research/",
        "prd/",
        "plan/",
        "docs/",
        "assets/",
        "vendor/",
    ];
    let suffixes = [
        ".rs", ".md", ".html", ".json", ".toml", ".sh", ".ps1", ".py", ".c",
    ];
    line.split(|c: char| c.is_whitespace() || "\"'`()<>|:,".contains(c))
        .filter(|token| {
            prefixes.iter().any(|prefix| token.starts_with(prefix))
                && suffixes.iter().any(|suffix| token.ends_with(suffix))
                && !token.contains('*')
                && !token.contains("...")
        })
        .map(str::to_owned)
        .collect()
}

/// A release workflow that names a path nobody has fails only when the release
/// runs -- the one place a broken path is most expensive to find. Every
/// repository path a workflow names must exist, except the directories a build
/// creates.
#[test]
fn every_path_a_workflow_names_exists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Build outputs the workflows create and then read; not in the tree.
    let generated = [
        "dist",
        "payload-build",
        "target",
        "target-six",
        "candidate",
        "evidence",
    ];
    let path = regex_lite_paths();
    let mut missing = Vec::new();
    for entry in fs::read_dir(root.join(".github/workflows")).expect("workflows") {
        let file = entry.expect("entry").path();
        if file.extension().is_none_or(|ext| ext != "yml") {
            continue;
        }
        let text = fs::read_to_string(&file).expect("workflow text");
        for candidate in path(&text) {
            let trimmed = candidate.trim_end_matches(['.', ',', ':', ')', '"', '\'']);
            if trimmed
                .split('/')
                .any(|part| generated.contains(&part) || part.contains('$') || part.contains('*'))
            {
                continue;
            }
            if !root.join(trimmed).exists() {
                missing.push(format!("{}: {trimmed}", file.display()));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "workflows name paths that do not exist:\n{}",
        missing.join("\n")
    );
}

/// Tokens that look like repository paths under a top-level directory the
/// workflows actually use, without pulling in a regex dependency.
fn regex_lite_paths() -> impl Fn(&str) -> Vec<String> {
    const ROOTS: [&str; 10] = [
        "target/",
        "research/",
        "loader/",
        "release/",
        "scripts/",
        "tests/",
        "src/",
        "ui/",
        "assets/",
        "crates/",
    ];
    move |text: &str| {
        let mut found = Vec::new();
        for token in text.split(|c: char| {
            c.is_whitespace() || matches!(c, '`' | '(' | '"' | '\'' | '=' | ',' | ';')
        }) {
            if let Some(start) = ROOTS.iter().filter_map(|root| token.find(root)).min() {
                let tail = &token[start..];
                let end = tail
                    .find(|c: char| {
                        !(c.is_ascii_alphanumeric()
                            || matches!(c, '/' | '.' | '_' | '-' | '$' | '*' | '{' | '}'))
                    })
                    .unwrap_or(tail.len());
                let path = &tail[..end];
                // A root reached only through a longer word (e.g. `mysrc/`) is not one.
                let preceded = start > 0 && token.as_bytes()[start - 1].is_ascii_alphanumeric();
                if !preceded && path.len() > path.find('/').unwrap_or(0) + 1 {
                    found.push(path.to_owned());
                }
            }
        }
        found
    }
}

/// The `minicon-platform` features MiniCon enables, by dependency block.
///
/// Cargo merges features per target, so a default-feature check proves nothing
/// about this product's build; pinning each block here keeps a block from
/// silently gaining or losing a feature. `minicon-platform` now lives in this
/// repository, so a change here is a change to the crate's own consumers —
/// keep the vendored crate's feature set and this matrix in step.
const MINICON_PLATFORM_FEATURES: &[(&str, &[&str])] = &[
    (
        "dependencies",
        &[
            "clipboard",
            "console-interrupt",
            "contained-process-spawn",
            "entropy",
            "filesystem",
            "filesystem-read",
            "font",
            "host-open",
            "ime",
            "input",
            "ipc",
            "network-http",
            "network-http-stream",
            "network-loopback",
            "parent-console",
            "pty",
            "runtime",
            "screenshot",
            "window",
        ],
    ),
    ("dev-dependencies", &["input-inject"]),
    (
        "target.'cfg(windows)'.dependencies",
        &["native-pixel-window"],
    ),
    ("target.'cfg(windows)'.dev-dependencies", &["runtime"]),
    (
        "target.'cfg(unix)'.dependencies",
        &["portable-pixel-window"],
    ),
];

#[test]
fn minicon_platform_features_match_the_consumer_matrix() {
    let manifest = fs::read_to_string(repo_root().join("Cargo.toml")).unwrap();
    let mut section = String::new();
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_string();
            continue;
        }
        if !line.starts_with("minicon-platform ") {
            continue;
        }
        assert!(
            line.contains("path = \"crates/minicon-platform\""),
            "[{section}] minicon-platform must be the in-repo path dependency: {line}"
        );
        assert!(
            line.contains("default-features = false"),
            "[{section}] minicon-platform must opt out of default features: {line}"
        );
        let features = line
            .split("features = [")
            .nth(1)
            .and_then(|f| f.split(']').next())
            .unwrap_or("")
            .split(',')
            .map(|f| f.trim().trim_matches('"').to_string())
            .filter(|f| !f.is_empty())
            .collect();
        found.push((section.clone(), features));
    }
    let expected: Vec<(String, Vec<String>)> = MINICON_PLATFORM_FEATURES
        .iter()
        .map(|(s, f)| (s.to_string(), f.iter().map(|f| f.to_string()).collect()))
        .collect();
    let effective = |target: &str| -> BTreeSet<&str> {
        MINICON_PLATFORM_FEATURES
            .iter()
            .filter(|(s, _)| !s.starts_with("target.") || s.contains(target))
            .flat_map(|(_, f)| f.iter().copied())
            .collect()
    };
    assert_eq!(
        found,
        expected,
        "minicon-platform features changed. Update MINICON_PLATFORM_FEATURES so the \
         crate's feature set and this matrix stay in step. Pinned effective sets, \
         dev included: windows {:?}, unix {:?}",
        effective("windows"),
        effective("unix"),
    );
    // The terminal stack is owned by this repository now: the renames live in
    // `crates/` as path dependencies and the vendored vt100/softbuffer forks in
    // `third_party/`. Nothing may quietly drift back to the external AgenTerm
    // git dependency, and the forks must stay resolved by path, not by rev.
    assert!(
        !manifest.contains("github.com/partnernetsoftware/agenterm"),
        "Cargo.toml still depends on the external AgenTerm repository; the \
         terminal stack is meant to be owned in-repo"
    );
    for fork in ["vte", "vt100", "softbuffer"] {
        assert!(
            manifest.contains(&format!("{fork} = {{ path = \"third_party/{fork}\" }}")),
            "the {fork} fork must be patched by path into third_party/"
        );
    }
}

/// The platform-boundary invariant (owner directive 2026-10-16: "platform
/// packages the architecture so the upper layers call one unified,
/// abstract seam").
///
/// `minicon-platform` exists so the product layer never needs to know which
/// OS it talks to: every raw OS reach (`std::os::*`, bare FFI) belongs inside
/// that crate's adapters. Two leaks were found and closed on 2026-10-16
/// (update.rs's `apply_staged_mode` and cli.rs's `install_cli` symlink) —
/// this test keeps the next one from landing. Product modules may keep
/// `#[cfg]` *selectors* for behaviour differences (docs text, key chords),
/// but not raw OS calls outside two sanctioned residents: the loader
/// entries (`startup.rs`, `linux_startup.rs`), which run before the platform
/// is initialized at all, and `#[cfg(test)] mod tests` fixture blocks.
#[test]
fn the_product_layer_never_touches_the_operating_system_directly() {
    let root = repo_root();
    // Loader entries live before the platform exists; their FFI is the
    // platform's own bootstrap problem, not a product choice.
    let residents = ["src/startup.rs", "src/linux_startup.rs"];
    let mut offenders = BTreeSet::new();
    for path in tracked_files(&root) {
        if !path.starts_with("src/") || !path.ends_with(".rs") || residents.contains(&path.as_str())
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(root.join(&path)) else {
            continue;
        };
        // Strip `#[cfg(test)] mod tests { ... }` blocks: fixtures may build
        // symlinks or set modes on any host to prove the seam works.
        let production = strip_test_modules(&text);
        for (index, line) in production.lines().enumerate() {
            if line.contains("std::os::")
                || line.contains("unsafe extern")
                || line.contains("extern \"system\"")
            {
                offenders.insert(format!("{path}:{}: {}", index + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "raw OS reach outside minicon-platform (move it behind a platform \
         seam; see filesystem_publish::copy_executable_mode for the pattern):\
         \n{offenders:#?}"
    );
}

/// Removes `#[cfg(test)] mod tests { .. }` blocks (including nested braces)
/// so architecture guards judge production code only.
fn strip_test_modules(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("#[cfg(test)]") {
        output.push_str(&rest[..start]);
        let after_marker = &rest[start + "#[cfg(test)]".len()..];
        let Some(open) = after_marker.find('{') else {
            // Attribute without a brace block on the same text; keep scanning
            // from just after this occurrence to avoid an infinite loop.
            output.push_str("#[cfg(test)]");
            rest = after_marker;
            continue;
        };
        let mut depth = 0usize;
        let mut close = None;
        for (offset, character) in after_marker[open..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match close {
            Some(end) => rest = &after_marker[end..],
            None => {
                output.push_str(after_marker);
                return output;
            }
        }
    }
    output.push_str(rest);
    output
}

/// The injected-input parity invariant (plan v0.2.12, scope item 3).
///
/// The invisible-rename defect chain (0.2.10 → 0.2.11 → the rename fix) had
/// one repeating shape: injected UI input carried a private copy of the focus
/// routing that the physical pipeline did not know about, so a new focus
/// owner wired into one path stayed invisible to the other. e986344 unified
/// every `send-ui-*` verb onto `ConApp::event`; this test locks that in, so
/// the next focus owner (notes editor, settings search) cannot be wired into
/// one path and forgotten in the other without turning this red.
#[test]
fn injected_ui_input_rides_the_physical_event_pipeline() {
    let root = repo_root();
    let control = fs::read_to_string(root.join("src/control.rs")).expect("read src/control.rs");
    let dispatch =
        fs::read_to_string(root.join("src/control_dispatch.rs")).expect("read control_dispatch");
    let main_source = fs::read_to_string(root.join("src/main.rs")).expect("read src/main.rs");
    let rename_source = fs::read_to_string(root.join("src/rename.rs")).expect("read src/rename.rs");
    let notes_source = fs::read_to_string(root.join("src/notes.rs")).expect("read src/notes.rs");

    // 1. Every `SendUi*` command variant exists, and its dispatch arm delivers
    //    every event through `ConApp::event` — the same entry a physical
    //    keystroke takes.
    let variants: Vec<String> = control
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let rest = trimmed.strip_prefix("SendUi")?;
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
            (!name.is_empty()).then(|| format!("SendUi{name}"))
        })
        .collect();
    assert!(
        variants.contains(&"SendUiKeys".to_owned())
            && variants.contains(&"SendUiIme".to_owned())
            && variants.contains(&"SendUiMouse".to_owned()),
        "expected the three known send-ui verbs in control.rs, found {variants:?}"
    );
    // Arm boundaries: each `CliCommand::…` arm runs until the next one.
    let mut arm_starts: Vec<usize> = dispatch
        .match_indices("CliCommand::")
        .map(|(index, _)| index)
        .collect();
    arm_starts.push(dispatch.len());
    for variant in &variants {
        let needle = format!("CliCommand::{variant}");
        let start = dispatch
            .find(&needle)
            .unwrap_or_else(|| panic!("{needle} has no dispatch arm"));
        let end = *arm_starts
            .iter()
            .find(|&&index| index > start + needle.len())
            .expect("arm terminator");
        let arm = &dispatch[start..end];
        assert!(
            arm.contains("self.event(window"),
            "{variant} must deliver through ConApp::event (the physical \
             pipeline); a private routing copy here is the defect shape that \
             shipped the invisible rename editor"
        );
        // 2. No private copy of the focus routing inside the arm. The event
        //    pipeline dispatches to these; an injection arm calling them
        //    directly has bypassed it.
        for banned in [
            "handle_workspace_shortcut",
            "handle_composer_key",
            "handle_composer_ime",
            "handle_rename_key",
            "handle_rename_ime",
            "handle_notes_key",
            "handle_notes_ime",
            "forward_keys_to_terminal",
            "forward_key_checked",
        ] {
            assert!(
                !arm.contains(banned),
                "{variant} calls {banned} directly; injected input must ride \
                 ConApp::event so the routing cannot drift from the physical path"
            );
        }
    }

    // 3. The physical pipeline owns text-input routing through exactly the two
    //    funnels, called from `event` before anything else consumes keys.
    assert!(
        main_source.contains("if self.route_rename_input(window, &event).is_some()"),
        "event() must route keyboard/IME through the rename funnel"
    );
    assert!(
        main_source.contains("if self.route_notes_input(window, &event).is_some()"),
        "event() must route keyboard/IME through the notes funnel"
    );
    assert!(
        main_source.contains("if self.route_composer_input(window, &event).is_some()"),
        "event() must route keyboard/IME through the composer funnel"
    );
    assert!(
        rename_source.contains("fn route_rename_input("),
        "route_rename_input must live with the rename state machine in rename.rs"
    );
    assert!(
        notes_source.contains("fn route_notes_input("),
        "route_notes_input must live with the notes editor in notes.rs"
    );
    assert!(
        main_source.contains("fn route_composer_input("),
        "route_composer_input must stay with the composer state in main.rs"
    );

    // 4. A funnel that claims `Changed` must have marked its damage region on
    //    the way: the pairing of claim and damage is the whole point of the
    //    funnel boundary.
    let rename_funnel = rename_source[rename_source
        .find("fn route_rename_input(")
        .expect("funnel present")
        ..rename_source
            .find("fn update_rename_ime_anchor")
            .expect("funnel end")]
        .to_owned();
    assert!(
        rename_funnel.contains("mark_tree_dirty()")
            && rename_funnel.contains("request_dirty_redraw(window)"),
        "the rename funnel must pair its Changed claim with the tree damage mark"
    );
    let notes_funnel = notes_source[notes_source
        .find("fn route_notes_input(")
        .expect("funnel present")
        ..notes_source
            .find("fn update_notes_ime_anchor")
            .expect("funnel end")]
        .to_owned();
    assert!(
        notes_funnel.contains("mark_host_ui_full()")
            && notes_funnel.contains("request_dirty_redraw(window)"),
        "the notes funnel must pair its Changed claim with the panel damage mark"
    );
}
