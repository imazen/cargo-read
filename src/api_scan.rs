//! Source-level public API scanner.
//!
//! Walks .rs files in a crate directory, extracts `pub` items, and builds
//! a module tree with fully-qualified paths. This is a best-effort heuristic
//! that works without compilation — it won't catch macro-generated items or
//! resolve type aliases, but covers the ~90% of API surface an LLM needs.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// A single public API item found by source scanning.
#[derive(Clone, Debug, Serialize)]
pub struct ApiItem {
    /// Fully qualified path: "serde::de::Deserialize"
    pub path: String,
    /// Item kind
    pub kind: ItemKind,
    /// The signature line(s), cleaned up
    pub signature: String,
    /// Source file (relative to crate root)
    pub file: String,
    /// Line number in source (1-based)
    pub line: usize,
    /// cfg gate, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cfg: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Function,
    Struct,
    Enum,
    Trait,
    Type,
    Const,
    Static,
    Module,
    Macro,
    ReExport,
}

/// Scan a crate directory for public API items.
pub fn scan_public_api(crate_dir: &Path, crate_name: &str) -> Vec<ApiItem> {
    let mut items = Vec::new();

    // Find the crate root
    let lib_rs = crate_dir.join("src/lib.rs");
    let main_rs = crate_dir.join("src/main.rs");
    let root = if lib_rs.exists() {
        lib_rs
    } else if main_rs.exists() {
        main_rs
    } else {
        return items;
    };

    let crate_mod = crate_name.replace('-', "_");
    scan_module(crate_dir, &root, &crate_mod, &mut items);
    items
}

/// Format scanned API items into a human/LLM-readable string.
pub fn format_api(crate_name: &str, crate_dir: &Path, items: &[ApiItem]) -> String {
    let mut out = String::new();

    // Group by module (the parent path)
    let mut current_module = String::new();
    for item in items {
        let module = item_module(&item.path);
        if module != current_module {
            if !current_module.is_empty() {
                writeln!(out).unwrap();
            }
            current_module = module.clone();
            writeln!(out, "# {module}").unwrap();
            writeln!(out).unwrap();
        }

        let abs_path = crate_dir.join(&item.file);
        let loc = format!("{}:{}", abs_path.display(), item.line);

        if let Some(ref cfg) = item.cfg {
            writeln!(out, "  #[cfg({cfg})]").unwrap();
        }
        writeln!(out, "  {}  // {loc}", item.signature).unwrap();
    }

    if !items.is_empty() {
        writeln!(out).unwrap();
        writeln!(
            out,
            "Hint: Read any file above for full details. Use `cargo read {crate_name}` for README."
        )
        .unwrap();
    }

    out
}

fn item_module(path: &str) -> String {
    match path.rsplit_once("::") {
        Some((module, _)) => module.to_string(),
        None => path.to_string(),
    }
}

/// Scan a single module file and recurse into submodules.
fn scan_module(crate_dir: &Path, file: &Path, module_path: &str, items: &mut Vec<ApiItem>) {
    let content = match fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return,
    };

    let rel_file = file
        .strip_prefix(crate_dir)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    let mut brace_depth: i32 = 0;
    let mut impl_stack: Vec<(i32, String)> = Vec::new(); // (depth, type_name)
    let mut skip_until_depth: Vec<i32> = Vec::new(); // brace depths to skip past (macro_rules!, non-pub mod)
    let mut pending_cfg: Option<String> = None;
    let mut doc_hidden = false;
    let mut in_block_comment = false;

    while i < lines.len() {
        let raw_line = lines[i];
        let line = raw_line.trim();

        // Track block comments
        if in_block_comment {
            if line.contains("*/") {
                in_block_comment = false;
            }
            i += 1;
            continue;
        }
        if line.starts_with("/*") && !line.contains("*/") {
            in_block_comment = true;
            i += 1;
            continue;
        }

        // Skip line comments
        if line.starts_with("//") {
            i += 1;
            continue;
        }

        // Track #[cfg(...)]
        if (line.starts_with("#[cfg(") && !line.starts_with("#[cfg(test)"))
            || line.starts_with("#[cfg_attr(")
        {
            if let Some(cfg) = extract_cfg(line) {
                pending_cfg = Some(cfg);
            }
            i += 1;
            continue;
        }

        // Track #[doc(hidden)]
        if line.contains("#[doc(hidden)]") {
            doc_hidden = true;
            i += 1;
            continue;
        }

        // Track brace depth for impl blocks and skip scopes
        let depth_change = count_braces(line);
        let old_depth = brace_depth;
        brace_depth += depth_change;

        // Pop skip scopes that have closed
        while let Some(&skip_depth) = skip_until_depth.last() {
            if brace_depth <= skip_depth {
                skip_until_depth.pop();
            } else {
                break;
            }
        }

        // Detect macro_rules! blocks — skip their entire body
        if line.starts_with("macro_rules!") || line.contains("macro_rules!") {
            if depth_change > 0 {
                skip_until_depth.push(old_depth);
            }
            i += 1;
            continue;
        }

        // Detect non-pub inline mod blocks — skip their body
        // (items pub within a private mod are not part of the crate's public API)
        if line.starts_with("mod ") && !line.starts_with("mod test") && line.contains('{') {
            skip_until_depth.push(old_depth);
            i += 1;
            continue;
        }

        // If we're inside a skip scope, don't emit items
        if !skip_until_depth.is_empty() {
            i += 1;
            continue;
        }

        // Detect impl blocks at the current brace level
        if line.starts_with("impl") || line.starts_with("pub") && line.contains(" impl ") {
            if let Some(impl_name) = parse_impl_header(line) {
                impl_stack.push((old_depth, impl_name));
            }
        }

        // Pop impl blocks that have closed
        while let Some(&(depth, _)) = impl_stack.last() {
            if brace_depth <= depth {
                impl_stack.pop();
            } else {
                break;
            }
        }

        // Only look for pub items
        if !line.starts_with("pub ") && !line.starts_with("pub(") {
            // Clear pending attributes if we hit a non-attribute, non-blank line
            if !line.is_empty() && !line.starts_with('#') {
                pending_cfg = None;
                doc_hidden = false;
            }
            i += 1;
            continue;
        }

        // Skip restricted visibility: pub(crate), pub(super), pub(self), pub(in ...)
        if line.starts_with("pub(") {
            i += 1;
            pending_cfg = None;
            doc_hidden = false;
            continue;
        }

        // Skip doc(hidden) items
        if doc_hidden {
            doc_hidden = false;
            pending_cfg = None;
            i += 1;
            continue;
        }

        let line_num = i + 1;
        let cfg = pending_cfg.take();

        // Parse the pub item
        if let Some(item) = parse_pub_item(line, &lines, &mut i, module_path, &impl_stack) {
            items.push(ApiItem {
                path: item.0,
                kind: item.1,
                signature: item.2,
                file: rel_file.clone(),
                line: line_num,
                cfg,
            });

            // If it's a pub mod, recurse into the submodule
            if item.1 == ItemKind::Module {
                let mod_name = &item.3;
                if !mod_name.is_empty() {
                    let sub_path = format!("{}::{}", module_path, mod_name);
                    if let Some(sub_file) = find_module_file(file, mod_name) {
                        scan_module(crate_dir, &sub_file, &sub_path, items);
                    }
                }
            }
        }

        doc_hidden = false;
        i += 1;
    }
}

/// Parse a `pub` item declaration. Returns (qualified_path, kind, signature, item_name).
fn parse_pub_item(
    first_line: &str,
    lines: &[&str],
    i: &mut usize,
    module_path: &str,
    impl_stack: &[(i32, String)],
) -> Option<(String, ItemKind, String, String)> {
    let rest = first_line.strip_prefix("pub ")?;

    // pub use — re-exports
    if let Some(use_path) = rest.strip_prefix("use ") {
        return parse_pub_use(use_path, module_path);
    }

    // pub mod name
    if let Some(after_mod) = rest.strip_prefix("mod ") {
        let name = after_mod
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .next()
            .unwrap_or("");
        if name.is_empty() {
            return None;
        }
        let sig = format!("pub mod {name}");
        return Some((
            format!("{module_path}::{name}"),
            ItemKind::Module,
            sig,
            name.to_string(),
        ));
    }

    // Determine kind and extract name
    let (kind, after_keyword) = if let Some(r) = rest.strip_prefix("fn ") {
        (ItemKind::Function, r)
    } else if let Some(r) = rest.strip_prefix("async fn ") {
        (ItemKind::Function, r)
    } else if let Some(r) = rest.strip_prefix("unsafe fn ") {
        (ItemKind::Function, r)
    } else if let Some(r) = rest.strip_prefix("const fn ") {
        (ItemKind::Function, r)
    } else if let Some(r) = rest.strip_prefix("struct ") {
        (ItemKind::Struct, r)
    } else if let Some(r) = rest.strip_prefix("enum ") {
        (ItemKind::Enum, r)
    } else if let Some(r) = rest.strip_prefix("trait ") {
        (ItemKind::Trait, r)
    } else if let Some(r) = rest.strip_prefix("type ") {
        (ItemKind::Type, r)
    } else if let Some(r) = rest.strip_prefix("const ") {
        (ItemKind::Const, r)
    } else if let Some(r) = rest.strip_prefix("static ") {
        (ItemKind::Static, r)
    } else if let Some(r) = rest.strip_prefix("macro ") {
        (ItemKind::Macro, r)
    } else if rest.starts_with("macro_rules!") {
        // pub macro_rules! name { ... }
        let r = rest.strip_prefix("macro_rules! ")?;
        (ItemKind::Macro, r)
    } else {
        return None;
    };

    // Extract the item name
    let name: String = after_keyword
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }

    // Build the qualified path
    let qpath = if let Some((_, impl_type)) = impl_stack.last() {
        format!("{module_path}::{impl_type}::{name}")
    } else {
        format!("{module_path}::{name}")
    };

    // Build the signature (may be multi-line)
    let sig = collect_signature(first_line, lines, i);

    Some((qpath, kind, sig, name))
}

/// Collect a potentially multi-line signature up to `{` or `;`.
fn collect_signature(first_line: &str, lines: &[&str], i: &mut usize) -> String {
    let mut sig = first_line.trim().to_string();

    // Check if the first line already ends the signature
    if sig_is_complete(&sig) {
        return truncate_sig(sig);
    }

    // Read continuation lines
    let start = *i;
    while *i + 1 < lines.len() && (*i - start) < 10 {
        *i += 1;
        let next = lines[*i].trim();
        sig.push(' ');
        sig.push_str(next);
        if sig_is_complete(&sig) {
            break;
        }
    }

    truncate_sig(sig)
}

fn sig_is_complete(sig: &str) -> bool {
    sig.contains('{') || sig.ends_with(';')
}

/// Truncate a signature at `{` or body content, keeping just the declaration.
fn truncate_sig(mut sig: String) -> String {
    // Remove everything from the first `{` onward
    if let Some(brace_pos) = sig.find('{') {
        sig.truncate(brace_pos);
    }
    // Normalize whitespace
    let sig = sig.split_whitespace().collect::<Vec<_>>().join(" ");
    sig.trim_end_matches(';').trim().to_string()
}

/// Parse `pub use` statements into re-export items.
fn parse_pub_use(use_path: &str, module_path: &str) -> Option<(String, ItemKind, String, String)> {
    let use_path = use_path.trim().trim_end_matches(';').trim();

    // Handle `pub use foo::{A, B, C}` — just show the whole use statement
    // Handle `pub use foo::Bar` — show it
    // Handle `pub use foo::*` — show it

    // Extract the last segment for the qualified name
    let last_segment = use_path
        .rsplit("::")
        .next()
        .unwrap_or(use_path)
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '*' && c != '{');

    let sig = format!("pub use {use_path}");
    let name = if last_segment.contains('{') || last_segment == "*" {
        // Group import or glob — use the full path
        String::new()
    } else {
        last_segment.to_string()
    };

    let qpath = if name.is_empty() {
        format!("{module_path}::{{re-exports}}")
    } else {
        format!("{module_path}::{name}")
    };

    Some((qpath, ItemKind::ReExport, sig, String::new()))
}

/// Parse an impl block header to extract the type name.
fn parse_impl_header(line: &str) -> Option<String> {
    // Match patterns like:
    //   impl Foo { ... }
    //   impl<T> Foo<T> { ... }
    //   impl Trait for Foo { ... }
    //   impl<T: Bound> Trait for Foo<T> { ... }
    let line = line.trim();
    let rest = line.strip_prefix("impl")?;
    let rest = rest.trim_start();

    // Skip generic params
    let rest = skip_generics(rest);
    let rest = rest.trim_start();

    // Check for "Trait for Type" pattern
    if let Some(for_pos) = rest.find(" for ") {
        let after_for = rest[for_pos + 5..].trim_start();
        let type_name = extract_type_name(after_for);
        if !type_name.is_empty() {
            let trait_name = extract_type_name(rest);
            return Some(format!("{type_name} (impl {trait_name})"));
        }
    }

    // Direct impl on type
    let type_name = extract_type_name(rest);
    if !type_name.is_empty() {
        return Some(type_name);
    }

    None
}

fn skip_generics(s: &str) -> &str {
    if !s.starts_with('<') {
        return s;
    }
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return &s[i + 1..];
                }
            }
            _ => {}
        }
    }
    s
}

fn extract_type_name(s: &str) -> String {
    s.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Count net brace changes in a line (ignoring strings and comments).
fn count_braces(line: &str) -> i32 {
    let mut count = 0i32;
    let mut in_string = false;
    let mut in_char = false;
    let mut escape = false;

    for c in line.chars() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' && (in_string || in_char) {
            escape = true;
            continue;
        }
        if c == '"' && !in_char {
            in_string = !in_string;
            continue;
        }
        if c == '\'' && !in_string {
            in_char = !in_char;
            continue;
        }
        if !in_string && !in_char {
            match c {
                '{' => count += 1,
                '}' => count -= 1,
                _ => {}
            }
        }
    }
    count
}

/// Extract a cfg predicate from a #[cfg(...)] attribute line.
fn extract_cfg(line: &str) -> Option<String> {
    let start = line.find("cfg(")?;
    let rest = &line[start + 4..];
    // Find the matching closing paren
    let mut depth = 1;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(rest[..i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the file for a submodule given the parent module's file.
fn find_module_file(parent_file: &Path, mod_name: &str) -> Option<PathBuf> {
    let parent_dir = parent_file.parent()?;

    // If parent is lib.rs or mod.rs, submodules are siblings or in subdirectory
    let parent_stem = parent_file.file_stem()?.to_str()?;

    if parent_stem == "lib" || parent_stem == "mod" || parent_stem == "main" {
        // Check parent_dir/mod_name.rs
        let sibling = parent_dir.join(format!("{mod_name}.rs"));
        if sibling.exists() {
            return Some(sibling);
        }
        // Check parent_dir/mod_name/mod.rs
        let nested = parent_dir.join(mod_name).join("mod.rs");
        if nested.exists() {
            return Some(nested);
        }
    } else {
        // Parent is foo.rs, submodules are in foo/
        let parent_name = parent_stem;
        let sub_dir = parent_dir.join(parent_name);
        let sibling = sub_dir.join(format!("{mod_name}.rs"));
        if sibling.exists() {
            return Some(sibling);
        }
        let nested = sub_dir.join(mod_name).join("mod.rs");
        if nested.exists() {
            return Some(nested);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_cfg() {
        assert_eq!(
            extract_cfg("#[cfg(feature = \"std\")]"),
            Some("feature = \"std\"".into())
        );
        assert_eq!(
            extract_cfg("#[cfg(not(feature = \"no_std\"))]"),
            Some("not(feature = \"no_std\")".into())
        );
        assert_eq!(extract_cfg("#[derive(Debug)]"), None);
    }

    #[test]
    fn test_count_braces() {
        assert_eq!(count_braces("fn foo() {"), 1);
        assert_eq!(count_braces("}"), -1);
        assert_eq!(count_braces("let s = \"}\";"), 0);
        assert_eq!(count_braces("{ { } }"), 0);
        assert_eq!(count_braces("pub struct Foo {"), 1);
    }

    #[test]
    fn test_extract_type_name() {
        assert_eq!(extract_type_name("Foo<T>"), "Foo");
        assert_eq!(extract_type_name("Bar {"), "Bar");
        assert_eq!(extract_type_name("MyType"), "MyType");
    }

    #[test]
    fn test_parse_impl_header() {
        assert_eq!(parse_impl_header("impl Foo {"), Some("Foo".into()));
        assert_eq!(parse_impl_header("impl<T> Foo<T> {"), Some("Foo".into()));
        assert_eq!(
            parse_impl_header("impl Display for Foo {"),
            Some("Foo (impl Display)".into())
        );
        assert_eq!(
            parse_impl_header("impl<T: Clone> Iterator for MyIter<T> {"),
            Some("MyIter (impl Iterator)".into())
        );
    }

    #[test]
    fn test_truncate_sig() {
        assert_eq!(truncate_sig("pub fn foo()".into()), "pub fn foo()");
        assert_eq!(truncate_sig("pub fn foo() {".into()), "pub fn foo()");
        assert_eq!(truncate_sig("pub struct Foo {".into()), "pub struct Foo");
        assert_eq!(
            truncate_sig("pub type Alias = Vec<u8>;".into()),
            "pub type Alias = Vec<u8>"
        );
    }

    #[test]
    fn test_skip_generics() {
        assert_eq!(skip_generics("<T>Foo"), "Foo");
        assert_eq!(skip_generics("<T: Clone + Debug>Bar"), "Bar");
        assert_eq!(skip_generics("Baz"), "Baz");
        assert_eq!(skip_generics("<A, B<C>>D"), "D");
    }

    #[test]
    fn test_parse_pub_use() {
        let (path, kind, sig, _) = parse_pub_use("foo::Bar;", "mycrate").unwrap();
        assert_eq!(path, "mycrate::Bar");
        assert_eq!(kind, ItemKind::ReExport);
        assert_eq!(sig, "pub use foo::Bar");
    }

    #[test]
    fn test_parse_pub_use_group() {
        let (path, _, sig, _) = parse_pub_use("foo::{A, B, C};", "mycrate").unwrap();
        assert_eq!(path, "mycrate::{re-exports}");
        assert_eq!(sig, "pub use foo::{A, B, C}");
    }

    #[test]
    fn test_scan_synthetic_crate() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("cargo-read-api-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();

        fs::write(
            dir.join("src/lib.rs"),
            r#"
pub mod utils;

pub use utils::helper;

pub struct Config {
    pub name: String,
}

pub enum Mode {
    Fast,
    Slow,
}

pub trait Process {
    fn run(&self);
}

pub fn init() -> Config {
    todo!()
}

pub(crate) fn internal() {}

#[doc(hidden)]
pub fn hidden_fn() {}

#[cfg(feature = "extra")]
pub fn extra_feature() {}

impl Config {
    pub fn new() -> Self {
        todo!()
    }
}
"#,
        )
        .unwrap();

        fs::write(
            dir.join("src/utils.rs"),
            r#"
pub fn helper() -> bool {
    true
}

pub const VERSION: &str = "1.0";
"#,
        )
        .unwrap();

        let items = scan_public_api(&dir, "testcrate");

        let names: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();

        // Should include public items
        assert!(names.contains(&"testcrate::utils"), "missing pub mod utils");
        assert!(
            names.contains(&"testcrate::helper"),
            "missing re-export helper"
        );
        assert!(names.contains(&"testcrate::Config"), "missing Config");
        assert!(names.contains(&"testcrate::Mode"), "missing Mode");
        assert!(names.contains(&"testcrate::Process"), "missing Process");
        assert!(names.contains(&"testcrate::init"), "missing init");
        assert!(
            names.contains(&"testcrate::extra_feature"),
            "missing cfg-gated item"
        );

        // Should NOT include restricted/hidden items
        assert!(
            !names.iter().any(|n| n.contains("internal")),
            "should exclude pub(crate)"
        );
        assert!(
            !names.iter().any(|n| n.contains("hidden_fn")),
            "should exclude doc(hidden)"
        );

        // Should include method
        assert!(
            names.contains(&"testcrate::Config::new"),
            "missing Config::new method"
        );

        // Should recurse into utils module
        assert!(
            names.contains(&"testcrate::utils::helper"),
            "missing utils::helper"
        );
        assert!(
            names.contains(&"testcrate::utils::VERSION"),
            "missing utils::VERSION"
        );

        // Check cfg annotation
        let extra = items
            .iter()
            .find(|i| i.path == "testcrate::extra_feature")
            .unwrap();
        assert_eq!(extra.cfg.as_deref(), Some("feature = \"extra\""));

        fs::remove_dir_all(&dir).unwrap();
    }
}
