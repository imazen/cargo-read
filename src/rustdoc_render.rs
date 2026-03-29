//! Render rustdoc JSON to markdown.
//!
//! Runs `cargo +nightly rustdoc --output-format json` on a cached crate,
//! then walks the item tree to produce markdown documentation.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use rustdoc_types::{Crate, GenericArgs, Id, Item, ItemEnum, Type, Visibility};

/// Try to generate markdown docs from rustdoc JSON.
pub fn render_docs(crate_dir: &Path, crate_name: &str) -> Result<String, String> {
    let json_path = build_rustdoc_json(crate_dir, crate_name)?;

    let json_text = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("Failed to read {}: {e}", json_path.display()))?;

    let krate: Crate = serde_json::from_str(&json_text)
        .map_err(|e| format!("Failed to parse rustdoc JSON: {e}"))?;

    Ok(render_crate(&krate, crate_name, crate_dir))
}

fn build_rustdoc_json(crate_dir: &Path, crate_name: &str) -> Result<std::path::PathBuf, String> {
    let lib_name = crate_name.replace('-', "_");
    let json_path = crate_dir
        .join("target/doc")
        .join(format!("{lib_name}.json"));

    if json_path.exists() {
        return Ok(json_path);
    }

    let nightly_check = Command::new("cargo")
        .args(["+nightly", "--version"])
        .output()
        .map_err(|e| format!("Failed to run cargo: {e}"))?;

    if !nightly_check.status.success() {
        return Err(
            "Nightly Rust required for --render-docs. Install with: rustup toolchain install nightly"
                .into(),
        );
    }

    eprintln!("Building rustdoc JSON for {crate_name} (nightly)...");

    let output = Command::new("cargo")
        .args([
            "+nightly",
            "rustdoc",
            "--manifest-path",
            &crate_dir.join("Cargo.toml").display().to_string(),
            "--",
            "--output-format",
            "json",
            "-Z",
            "unstable-options",
        ])
        .output()
        .map_err(|e| format!("Failed to run cargo rustdoc: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("cargo rustdoc failed:\n{stderr}"));
    }

    if json_path.exists() {
        Ok(json_path)
    } else {
        Err(format!(
            "Expected JSON at {} but not found",
            json_path.display()
        ))
    }
}

fn render_crate(krate: &Crate, crate_name: &str, crate_dir: &Path) -> String {
    let mut out = String::new();

    let root = &krate.index[&krate.root];

    if let Some(docs) = &root.docs {
        if !docs.is_empty() {
            writeln!(out, "{docs}").unwrap();
            writeln!(out, "\n---\n").unwrap();
        }
    }

    if let ItemEnum::Module(m) = &root.inner {
        render_module_items(krate, &m.items, crate_name, crate_dir, &mut out, 0);
    }

    writeln!(out).unwrap();
    writeln!(
        out,
        "Hint: Use `cargo read {crate_name}` for README, `cargo read --api {crate_name}` for quick API overview."
    )
    .unwrap();

    out
}

fn render_module_items(
    krate: &Crate,
    items: &[Id],
    parent_path: &str,
    crate_dir: &Path,
    out: &mut String,
    depth: usize,
) {
    for id in items {
        let Some(item) = krate.index.get(id) else {
            continue;
        };

        if !matches!(item.visibility, Visibility::Public) {
            continue;
        }

        let Some(name) = &item.name else {
            if let ItemEnum::Impl(imp) = &item.inner {
                if imp.trait_.is_some() && !imp.items.is_empty() {
                    render_impl(krate, item, parent_path, out);
                }
            }
            continue;
        };

        let item_path = format!("{parent_path}::{name}");
        let heading = "#".repeat((depth + 2).min(6));

        match &item.inner {
            ItemEnum::Module(m) => {
                writeln!(out, "{heading} mod `{name}`\n").unwrap();
                render_docs_text(item, out);
                render_module_items(krate, &m.items, &item_path, crate_dir, out, depth + 1);
            }
            ItemEnum::Function(f) => {
                let sig = render_fn_sig(name, f);
                writeln!(out, "{heading} `{sig}`").unwrap();
                render_source_link(item, crate_dir, out);
                render_docs_text(item, out);
            }
            ItemEnum::Struct(s) => {
                let generics = render_generics(&s.generics);
                writeln!(out, "{heading} struct `{name}{generics}`").unwrap();
                render_source_link(item, crate_dir, out);
                render_docs_text(item, out);
                render_struct_methods(krate, s, &item_path, out, depth);
            }
            ItemEnum::Enum(e) => {
                let generics = render_generics(&e.generics);
                writeln!(out, "{heading} enum `{name}{generics}`").unwrap();
                render_source_link(item, crate_dir, out);
                render_docs_text(item, out);
                for variant_id in &e.variants {
                    if let Some(variant) = krate.index.get(variant_id) {
                        if let Some(vname) = &variant.name {
                            write!(out, "- `{vname}`").unwrap();
                            if let Some(docs) = &variant.docs {
                                let first = docs.lines().next().unwrap_or("").trim();
                                if !first.is_empty() {
                                    write!(out, " — {first}").unwrap();
                                }
                            }
                            writeln!(out).unwrap();
                        }
                    }
                }
                writeln!(out).unwrap();
            }
            ItemEnum::Trait(t) => {
                let generics = render_generics(&t.generics);
                writeln!(out, "{heading} trait `{name}{generics}`").unwrap();
                render_source_link(item, crate_dir, out);
                render_docs_text(item, out);
                for method_id in &t.items {
                    if let Some(method) = krate.index.get(method_id) {
                        if let (Some(mname), ItemEnum::Function(f)) = (&method.name, &method.inner)
                        {
                            let sig = render_fn_sig(mname, f);
                            write!(out, "- `{sig}`").unwrap();
                            if let Some(docs) = &method.docs {
                                let first = docs.lines().next().unwrap_or("").trim();
                                if !first.is_empty() {
                                    write!(out, " — {first}").unwrap();
                                }
                            }
                            writeln!(out).unwrap();
                        }
                    }
                }
                writeln!(out).unwrap();
            }
            ItemEnum::TypeAlias(t) => {
                let generics = render_generics(&t.generics);
                let rhs = render_type_brief(&t.type_);
                writeln!(out, "{heading} type `{name}{generics} = {rhs}`").unwrap();
                render_source_link(item, crate_dir, out);
                render_docs_text(item, out);
            }
            ItemEnum::Constant { type_: _, const_ } => {
                let val = const_
                    .value
                    .as_deref()
                    .map(|v| format!(" = {v}"))
                    .unwrap_or_default();
                writeln!(out, "{heading} const `{name}{val}`").unwrap();
                render_source_link(item, crate_dir, out);
                render_docs_text(item, out);
            }
            ItemEnum::Macro(_) => {
                writeln!(out, "{heading} macro `{name}!`").unwrap();
                render_source_link(item, crate_dir, out);
                render_docs_text(item, out);
            }
            ItemEnum::Use(u) => {
                if u.is_glob {
                    writeln!(out, "- pub use {}::*", u.source).unwrap();
                } else {
                    writeln!(out, "- pub use {}", u.source).unwrap();
                }
            }
            _ => {}
        }
    }
}

fn render_impl(krate: &Crate, item: &Item, parent_path: &str, out: &mut String) {
    let ItemEnum::Impl(imp) = &item.inner else {
        return;
    };

    let Some(trait_path) = &imp.trait_ else {
        return;
    };
    let trait_name = &trait_path.path;

    writeln!(out, "#### impl `{trait_name}` for `{parent_path}`").unwrap();
    if let Some(docs) = &item.docs {
        if !docs.is_empty() {
            writeln!(out, "\n{docs}").unwrap();
        }
    }

    for method_id in &imp.items {
        if let Some(method) = krate.index.get(method_id) {
            if matches!(method.visibility, Visibility::Public) {
                if let (Some(mname), ItemEnum::Function(f)) = (&method.name, &method.inner) {
                    let sig = render_fn_sig(mname, f);
                    write!(out, "- `{sig}`").unwrap();
                    if let Some(docs) = &method.docs {
                        let first = docs.lines().next().unwrap_or("").trim();
                        if !first.is_empty() {
                            write!(out, " — {first}").unwrap();
                        }
                    }
                    writeln!(out).unwrap();
                }
            }
        }
    }
    writeln!(out).unwrap();
}

fn render_struct_methods(
    krate: &Crate,
    s: &rustdoc_types::Struct,
    _parent_path: &str,
    out: &mut String,
    depth: usize,
) {
    let heading = "#".repeat((depth + 3).min(6));

    for impl_id in &s.impls {
        let Some(imp_item) = krate.index.get(impl_id) else {
            continue;
        };
        let ItemEnum::Impl(imp) = &imp_item.inner else {
            continue;
        };

        if imp.trait_.is_some() {
            continue;
        }

        let methods: Vec<_> = imp
            .items
            .iter()
            .filter_map(|id| krate.index.get(id))
            .filter(|m| matches!(m.visibility, Visibility::Public))
            .filter(|m| matches!(m.inner, ItemEnum::Function(_)))
            .collect();

        if methods.is_empty() {
            continue;
        }

        writeln!(out, "{heading} Methods\n").unwrap();
        for method in &methods {
            if let (Some(mname), ItemEnum::Function(f)) = (&method.name, &method.inner) {
                let sig = render_fn_sig(mname, f);
                writeln!(out, "- `{sig}`").unwrap();
                if let Some(docs) = &method.docs {
                    if !docs.is_empty() {
                        writeln!(out).unwrap();
                        // Indent doc text under the bullet
                        for line in docs.lines() {
                            writeln!(out, "  {line}").unwrap();
                        }
                        writeln!(out).unwrap();
                    }
                }
            }
        }
    }
}

fn render_docs_text(item: &Item, out: &mut String) {
    if let Some(docs) = &item.docs {
        if !docs.is_empty() {
            writeln!(out, "\n{docs}\n").unwrap();
        }
    } else {
        writeln!(out).unwrap();
    }
}

fn render_source_link(item: &Item, crate_dir: &Path, out: &mut String) {
    if let Some(span) = &item.span {
        let abs = crate_dir.join(&span.filename);
        writeln!(out, "<sub>{}:{}</sub>", abs.display(), span.begin.0).unwrap();
    }
}

fn render_fn_sig(name: &str, f: &rustdoc_types::Function) -> String {
    let generics = render_generics(&f.generics);
    let inputs: Vec<String> = f
        .sig
        .inputs
        .iter()
        .map(|(pname, ty)| {
            let ty_str = render_type_brief(ty);
            if pname == "self" {
                ty_str
            } else {
                format!("{pname}: {ty_str}")
            }
        })
        .collect();

    let ret = f
        .sig
        .output
        .as_ref()
        .map(|t| format!(" -> {}", render_type_brief(t)))
        .unwrap_or_default();

    format!("fn {name}{generics}({}){ret}", inputs.join(", "))
}

fn render_generics(g: &rustdoc_types::Generics) -> String {
    if g.params.is_empty() {
        return String::new();
    }
    let params: Vec<String> = g.params.iter().map(|p| p.name.clone()).collect();
    format!("<{}>", params.join(", "))
}

fn render_type_brief(ty: &Type) -> String {
    match ty {
        Type::ResolvedPath(p) => {
            let args = p
                .args
                .as_ref()
                .map(|a| render_generic_args(a))
                .unwrap_or_default();
            format!("{}{args}", p.path)
        }
        Type::Generic(g) => g.clone(),
        Type::Primitive(p) => p.clone(),
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            let lt = lifetime
                .as_ref()
                .map(|l| format!("{l} "))
                .unwrap_or_default();
            let m = if *is_mutable { "mut " } else { "" };
            format!("&{lt}{m}{}", render_type_brief(type_))
        }
        Type::Slice(inner) => format!("[{}]", render_type_brief(inner)),
        Type::Array { type_, len } => format!("[{}; {len}]", render_type_brief(type_)),
        Type::Tuple(types) => {
            let inner: Vec<String> = types.iter().map(render_type_brief).collect();
            format!("({})", inner.join(", "))
        }
        Type::RawPointer {
            is_mutable, type_, ..
        } => {
            let m = if *is_mutable { "mut" } else { "const" };
            format!("*{m} {}", render_type_brief(type_))
        }
        Type::ImplTrait(bounds) => {
            let b: Vec<String> = bounds
                .iter()
                .filter_map(|b| {
                    if let rustdoc_types::GenericBound::TraitBound { trait_, .. } = b {
                        Some(trait_.path.clone())
                    } else {
                        None
                    }
                })
                .collect();
            format!("impl {}", b.join(" + "))
        }
        Type::QualifiedPath {
            name,
            self_type,
            trait_,
            ..
        } => {
            let self_ty = render_type_brief(self_type);
            let tr = trait_
                .as_ref()
                .map(|t| format!(" as {}", t.path))
                .unwrap_or_default();
            format!("<{self_ty}{tr}>::{name}")
        }
        Type::DynTrait(dt) => {
            let traits: Vec<String> = dt.traits.iter().map(|t| t.trait_.path.clone()).collect();
            format!("dyn {}", traits.join(" + "))
        }
        Type::FunctionPointer(_) => "fn(...)".into(),
        Type::Infer => "_".into(),
        Type::Pat { type_, .. } => render_type_brief(type_),
    }
}

fn render_generic_args(args: &GenericArgs) -> String {
    match args {
        GenericArgs::AngleBracketed { args, .. } => {
            if args.is_empty() {
                return String::new();
            }
            let inner: Vec<String> = args
                .iter()
                .filter_map(|a| match a {
                    rustdoc_types::GenericArg::Type(t) => Some(render_type_brief(t)),
                    rustdoc_types::GenericArg::Lifetime(l) => Some(l.clone()),
                    rustdoc_types::GenericArg::Const(c) => {
                        Some(c.value.clone().unwrap_or_else(|| c.expr.clone()))
                    }
                    _ => None,
                })
                .collect();
            format!("<{}>", inner.join(", "))
        }
        GenericArgs::Parenthesized { inputs, output } => {
            let ins: Vec<String> = inputs.iter().map(render_type_brief).collect();
            let ret = output
                .as_ref()
                .map(|t| format!(" -> {}", render_type_brief(t)))
                .unwrap_or_default();
            format!("({}){ret}", ins.join(", "))
        }
        _ => String::new(),
    }
}
