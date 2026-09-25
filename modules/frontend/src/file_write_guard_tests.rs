//! Architecture guard: Editor code does not write files outside the Editor
//! pool (`docs/plans/stateless-editor.md` section 1).
//!
//! Scans the non-test source of the Editor crates for `std::fs` write APIs.
//! Credentials and install state are written by `artisan_editor_cli`
//! (`modules/cli`), which is outside this scan; calling its APIs is allowed.
//! Test code (`#[cfg(test)]` modules and every file they own) is exempt.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Editor source roots, relative to `modules/`.
const SCANNED_ROOTS: &[&str] = &["frontend/src", "ui/src"];

/// Crate roots whose module trees decide which files are test-only.
const CRATE_ROOTS: &[&str] = &[
    "frontend/src/lib.rs",
    "frontend/src/main.rs",
    "ui/src/lib.rs",
];

/// Source fragments that create, replace, move, or delete files.
const WRITE_APIS: &[&str] = &[
    "fs::write",
    "File::create",
    "OpenOptions",
    "fs::rename",
    "fs::copy",
    "create_dir",
    "remove_file",
    "remove_dir",
    "hard_link",
    "set_permissions",
];

/// Files allowed to write, relative to `modules/`, each with its justification.
const ALLOWED_WRITERS: &[(&str, &str)] = &[
    (
        "frontend/src/editor_settings/storage.rs",
        "the Editor pool: settings.json, its corrupt copy, and legacy-file migration",
    ),
    (
        "frontend/src/native_last_used.rs",
        "temporary: last-used model and project orders are Forge-pool state that step 7 of \
         docs/plans/stateless-editor.md moves to the Forge, deleting this writer",
    ),
    (
        "frontend/src/dev_startup_receipt.rs",
        "opt-in development receipt, written only to the file the dev launcher names in \
         ARTISAN_DEV_STARTUP_RECEIPT",
    ),
    (
        "frontend/src/native_application/frame_capture.rs",
        "opt-in development frame capture report, written only to the file named in \
         ARTISAN_FRAME_CAPTURE",
    ),
];

fn modules_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("modules directory")
        .to_path_buf()
}

fn relative(path: &Path) -> String {
    path.strip_prefix(modules_directory())
        .expect("scanned file under modules/")
        .to_string_lossy()
        .replace('\\', "/")
}

fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("readable source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// One outline `mod name;` declaration.
struct ModuleDeclaration {
    name: String,
    path_attribute: Option<String>,
    test_only: bool,
}

fn attribute(line: &str) -> Option<&str> {
    line.strip_prefix("#[")?.strip_suffix(']')
}

/// Outline module declarations, with their `#[cfg(test)]` and `#[path]`
/// attributes. Inline `mod name { ... }` blocks are handled by
/// [`without_inline_test_modules`].
fn module_declarations(source: &str) -> Vec<ModuleDeclaration> {
    let mut declarations = Vec::new();
    let (mut test_only, mut path_attribute) = (false, None);
    for line in source.lines().map(str::trim) {
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(attribute) = attribute(line) {
            test_only |= attribute == "cfg(test)";
            if let Some(path) = attribute
                .strip_prefix("path = \"")
                .and_then(|rest| rest.strip_suffix('"'))
            {
                path_attribute = Some(path.to_owned());
            }
            continue;
        }
        let item = line
            .strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub(super) "))
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line);
        if let Some(name) = item
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(';'))
        {
            declarations.push(ModuleDeclaration {
                name: name.trim().to_owned(),
                path_attribute: path_attribute.take(),
                test_only,
            });
        }
        (test_only, path_attribute) = (false, None);
    }
    declarations
}

/// Walks the module trees from the crate roots and returns every reachable
/// file with whether it is compiled only for tests. Files loaded through
/// `#[path]` (and crate roots and `mod.rs`) own their directory; other files
/// own the directory named after their stem, as rustc resolves them.
fn module_files() -> BTreeMap<PathBuf, bool> {
    let modules = modules_directory();
    let mut files = BTreeMap::new();
    let mut pending = CRATE_ROOTS
        .iter()
        .map(|root| (modules.join(root), false, true))
        .collect::<Vec<_>>();
    while let Some((file, test_only, owns_directory)) = pending.pop() {
        if files.insert(file.clone(), test_only).is_some() {
            continue;
        }
        let source = std::fs::read_to_string(&file).expect("readable module");
        let directory = file.parent().expect("module directory");
        let child_directory =
            if owns_directory || file.file_stem().is_some_and(|stem| stem == "mod") {
                directory.to_path_buf()
            } else {
                directory.join(file.file_stem().expect("module stem"))
            };
        for declaration in module_declarations(&source) {
            let test_only = test_only || declaration.test_only;
            let (child, owns) = if let Some(path) = &declaration.path_attribute {
                (directory.join(path), true)
            } else {
                let flat = child_directory.join(format!("{}.rs", declaration.name));
                let nested = child_directory.join(&declaration.name).join("mod.rs");
                (if nested.exists() { nested } else { flat }, false)
            };
            if child.exists() {
                pending.push((child, test_only, owns));
            }
        }
    }
    files
}

/// Drops inline `#[cfg(test)] mod name { ... }` blocks and line comments.
fn without_inline_test_modules(source: &str) -> String {
    let mut kept = String::new();
    let (mut test_attribute, mut skip_depth) = (false, None::<i64>);
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(depth) = skip_depth.as_mut() {
            *depth += braces(trimmed);
            if *depth <= 0 {
                skip_depth = None;
            }
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        if let Some(attribute) = attribute(trimmed) {
            test_attribute |= attribute == "cfg(test)";
        } else if test_attribute && trimmed.contains("mod ") && trimmed.ends_with('{') {
            skip_depth = Some(braces(trimmed));
            test_attribute = false;
            continue;
        } else if !trimmed.is_empty() {
            test_attribute = false;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

fn braces(line: &str) -> i64 {
    line.chars().fold(0, |depth, character| match character {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

/// Production files that call a file-writing API, with the offending lines.
fn writers() -> BTreeMap<String, Vec<String>> {
    let module_files = module_files();
    let mut files = Vec::new();
    for root in SCANNED_ROOTS {
        rust_files(&modules_directory().join(root), &mut files);
    }
    let mut writers = BTreeMap::new();
    for file in files {
        // Unreachable files are scanned too: only proven test code is exempt.
        if module_files.get(&file).copied().unwrap_or(false) {
            continue;
        }
        let source = std::fs::read_to_string(&file).expect("readable source");
        let lines = without_inline_test_modules(&source)
            .lines()
            .filter(|line| WRITE_APIS.iter().any(|api| line.contains(api)))
            .map(|line| line.trim().to_owned())
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            writers.insert(relative(&file), lines);
        }
    }
    writers
}

#[test]
fn editor_code_writes_files_only_through_allowed_modules() {
    let writers = writers();
    let allowed = ALLOWED_WRITERS
        .iter()
        .map(|(path, _)| *path)
        .collect::<BTreeSet<_>>();
    let unexpected = writers
        .iter()
        .filter(|(path, _)| !allowed.contains(path.as_str()))
        .collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "Editor code must not write files outside the Editor pool \
         (docs/plans/stateless-editor.md section 1). Persist Editor-only settings through \
         crate::editor_settings, send domain state to the Forge, or justify a new entry in \
         ALLOWED_WRITERS: {unexpected:#?}"
    );
    let stale = allowed
        .iter()
        .filter(|path| !writers.contains_key(**path))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "remove allowlist entries that no longer write files: {stale:?}"
    );
}

#[test]
fn guard_detects_writes_and_exempts_only_test_code() {
    let source = "\
use std::fs;
// fs::write(\"comment\", b\"\");
fn production() { fs::write(\"a\", b\"\").ok(); }
#[cfg(test)]
mod tests {
    fn helper() { if true { std::fs::remove_file(\"b\").ok(); } }
}
fn after() { std::fs::File::create(\"c\").ok(); }
";
    let scanned = without_inline_test_modules(source);
    assert!(scanned.contains("fs::write(\"a\""));
    assert!(!scanned.contains("comment"));
    assert!(!scanned.contains("remove_file"));
    assert!(scanned.contains("File::create"));
    let declarations = module_declarations(
        "#[cfg(test)]\n#[path = \"x/tests.rs\"]\nmod tests;\npub(crate) mod storage;\n",
    );
    assert_eq!(declarations.len(), 2);
    assert!(declarations[0].test_only);
    assert_eq!(
        declarations[0].path_attribute.as_deref(),
        Some("x/tests.rs")
    );
    assert!(!declarations[1].test_only);
    assert_eq!(declarations[1].name, "storage");
    let files = module_files();
    let modules = modules_directory();
    assert_eq!(
        files.get(&modules.join("frontend/src/editor_settings/storage.rs")),
        Some(&false)
    );
    assert_eq!(
        files.get(&modules.join("frontend/src/editor_settings/tests.rs")),
        Some(&true)
    );
    assert_eq!(
        files.get(&modules.join("frontend/src/native_application/tests/sends.rs")),
        Some(&true)
    );
}
