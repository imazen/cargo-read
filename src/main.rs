#![forbid(unsafe_code)]

mod api_scan;
mod args;
mod rustdoc_render;

use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;

use semver::{Version, VersionReq};
use serde::Serialize;

use args::{CrateSpec, parse_crate_spec};

const CRATES_API_ROOT: &str = "https://crates.io/api/v1/crates";
const USER_AGENT: &str = "cargo-read (https://github.com/imazen/cargo-read)";

/// Crate metadata fetched from crates.io.
#[derive(Clone, Default, Serialize)]
struct CrateMeta {
    name: String,
    version: String,
    description: Option<String>,
    license: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    documentation: Option<String>,
    rust_version: Option<String>,
    edition: Option<String>,
    crate_size: Option<u64>,
    downloads: Option<u64>,
    categories: Vec<String>,
    keywords: Vec<String>,
    features: Vec<String>,
}

/// JSON output format.
#[derive(Serialize)]
struct JsonOutput {
    #[serde(flatten)]
    meta: CrateMeta,
    path: String,
    readme: Option<String>,
    files: Vec<String>,
}

/// JSON output for --api --json.
#[derive(Serialize)]
struct ApiJsonOutput {
    #[serde(flatten)]
    meta: CrateMeta,
    path: String,
    items: Vec<api_scan::ApiItem>,
}

fn main() {
    let args = args::parse();
    let spec = parse_crate_spec(&args.crate_spec);

    // Resolve version + metadata from crates.io (always, even if cached)
    let (version, meta) = match resolve_version_and_meta(&spec, args.verbose) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "error: failed to resolve version for `{}`: {}",
                spec.name, e
            );
            process::exit(1);
        }
    };

    // Determine cache directory
    let cache_root = args.cache_dir.unwrap_or_else(default_cache_dir);
    let crate_dir = cache_root.join(format!("{}-{}", spec.name, version));

    // Download and extract if not cached (or --force)
    if args.force || !crate_dir.exists() {
        if args.verbose {
            eprintln!("downloading {}=={}", spec.name, version);
        }
        match download_and_extract(&spec.name, &version, &cache_root) {
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "error: failed to download `{}=={}`: {}",
                    spec.name, version, e
                );
                process::exit(1);
            }
        }
    } else if args.verbose {
        eprintln!("using cached {}=={}", spec.name, version);
    }

    if !crate_dir.exists() {
        eprintln!(
            "error: expected directory not found: {}",
            crate_dir.display()
        );
        process::exit(1);
    }

    let readme = find_readme(&crate_dir);
    let files = list_files(&crate_dir);

    // Output
    if args.path_only {
        println!("{}", crate_dir.display());
    } else if args.readme_only {
        match readme {
            Some(content) => print!("{}", content),
            None => {
                eprintln!("no README found in {}", crate_dir.display());
                process::exit(1);
            }
        }
    } else if args.render_docs {
        match rustdoc_render::render_docs(&crate_dir, &spec.name) {
            Ok(md) => print!("{}", md),
            Err(e) => {
                eprintln!("error: {e}");
                eprintln!("Falling back to source-level docs...");
                eprintln!();
                let api_items = api_scan::scan_public_api(&crate_dir, &spec.name);
                print!(
                    "{}",
                    api_scan::format_docs(&spec.name, &crate_dir, &api_items)
                );
            }
        }
    } else if args.api || args.docs {
        let api_items = api_scan::scan_public_api(&crate_dir, &spec.name);
        if args.json {
            let output = ApiJsonOutput {
                meta,
                path: crate_dir.display().to_string(),
                items: api_items,
            };
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        } else if args.docs {
            print!(
                "{}",
                api_scan::format_docs(&spec.name, &crate_dir, &api_items)
            );
        } else {
            print!(
                "{}",
                api_scan::format_api(&spec.name, &crate_dir, &api_items)
            );
        }
    } else if args.json {
        let output = JsonOutput {
            meta,
            path: crate_dir.display().to_string(),
            readme,
            files,
        };
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        print!(
            "{}",
            format_natural(&meta, &crate_dir, readme.as_deref(), &files, &spec.name)
        );
    }
}

/// Format the default human/LLM-readable output.
fn format_natural(
    meta: &CrateMeta,
    crate_dir: &Path,
    readme: Option<&str>,
    files: &[String],
    crate_name: &str,
) -> String {
    let mut out = String::new();

    // Frontmatter
    writeln!(out, "---").unwrap();
    writeln!(out, "crate: {}", meta.name).unwrap();
    writeln!(out, "version: {}", meta.version).unwrap();
    if let Some(ref desc) = meta.description {
        writeln!(out, "description: {desc}").unwrap();
    }
    if let Some(ref license) = meta.license {
        writeln!(out, "license: {license}").unwrap();
    }
    if let Some(ref repo) = meta.repository {
        writeln!(out, "repository: {repo}").unwrap();
    }
    if let Some(ref hp) = meta.homepage {
        if meta.repository.as_deref() != Some(hp) {
            writeln!(out, "homepage: {hp}").unwrap();
        }
    }
    if let Some(ref docs) = meta.documentation {
        writeln!(out, "documentation: {docs}").unwrap();
    }
    if let Some(ref msrv) = meta.rust_version {
        writeln!(out, "rust-version: {msrv}").unwrap();
    }
    if let Some(ref ed) = meta.edition {
        writeln!(out, "edition: {ed}").unwrap();
    }
    if let Some(size) = meta.crate_size {
        writeln!(out, "crate-size: {}", format_bytes(size)).unwrap();
    }
    if let Some(dl) = meta.downloads {
        writeln!(out, "downloads: {}", format_number(dl)).unwrap();
    }
    if !meta.keywords.is_empty() {
        writeln!(out, "keywords: {}", meta.keywords.join(", ")).unwrap();
    }
    if !meta.categories.is_empty() {
        writeln!(out, "categories: {}", meta.categories.join(", ")).unwrap();
    }
    if !meta.features.is_empty() {
        writeln!(out, "features: {}", meta.features.join(", ")).unwrap();
    }
    writeln!(out, "path: {}", crate_dir.display()).unwrap();
    writeln!(out, "---").unwrap();

    // README
    if let Some(readme) = readme {
        writeln!(out).unwrap();
        out.push_str(readme);
        if !readme.ends_with('\n') {
            writeln!(out).unwrap();
        }
    }

    // File listing with absolute paths
    writeln!(out).unwrap();
    writeln!(out, "## Files").unwrap();
    writeln!(out).unwrap();
    for f in files {
        let abs = crate_dir.join(f);
        writeln!(out, "{}", abs.display()).unwrap();
    }

    writeln!(out).unwrap();
    writeln!(
        out,
        "Hint: Run `cargo read --api {crate_name}` for API structure, `cargo read --docs {crate_name}` for API docs"
    )
    .unwrap();

    out
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} bytes", bytes)
    }
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn default_cache_dir() -> PathBuf {
    dirs_cache().join("cargo-read")
}

/// Platform-appropriate cache directory.
fn dirs_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache");
    }
    // Windows fallback
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local);
    }
    PathBuf::from("/tmp")
}

/// Query crates.io for the latest version matching the spec, plus metadata.
fn resolve_version_and_meta(
    spec: &CrateSpec,
    verbose: bool,
) -> Result<(Version, CrateMeta), Box<dyn std::error::Error>> {
    let versions_url = format!("{}/{}/versions", CRATES_API_ROOT, spec.name);
    if verbose {
        eprintln!("fetching versions from {}", versions_url);
    }

    let body: String = ureq::get(&versions_url)
        .header("User-Agent", USER_AGENT)
        .call()?
        .into_body()
        .read_to_string()?;

    let json: serde_json::Value = serde_json::from_str(&body)?;

    let versions_array = json
        .pointer("/versions")
        .and_then(|v| v.as_array())
        .ok_or("malformed response: missing /versions array")?;

    // Parse all valid, non-yanked versions
    let mut versions: Vec<Version> = versions_array
        .iter()
        .filter(|v| {
            v.as_object()
                .and_then(|o| o.get("yanked"))
                .and_then(|y| y.as_bool())
                != Some(true)
        })
        .filter_map(|v| {
            v.as_object()
                .and_then(|o| o.get("num"))
                .and_then(|n| n.as_str())
        })
        .filter_map(|v| Version::parse(v).ok())
        .collect();

    if versions.is_empty() {
        return Err(format!("no versions found for crate `{}`", spec.name).into());
    }

    let version_req = match &spec.version_req {
        Some(req) => VersionReq::parse(req)?,
        None => VersionReq::STAR,
    };

    versions.sort_by(|a, b| b.cmp(a));

    let version = versions
        .into_iter()
        .find(|v| version_req.matches(v))
        .ok_or_else(|| -> Box<dyn std::error::Error> {
            format!(
                "no version of `{}` matches requirement `{}`",
                spec.name, version_req
            )
            .into()
        })?;

    // Fetch version-specific metadata
    let meta = fetch_version_meta(&spec.name, &version, &json, verbose)?;

    Ok((version, meta))
}

/// Fetch metadata for a specific version from crates.io.
fn fetch_version_meta(
    name: &str,
    version: &Version,
    versions_json: &serde_json::Value,
    verbose: bool,
) -> Result<CrateMeta, Box<dyn std::error::Error>> {
    let version_str = version.to_string();

    // Try to extract from the already-fetched versions array first
    let version_obj = versions_json
        .pointer("/versions")
        .and_then(|vs| vs.as_array())
        .and_then(|vs| {
            vs.iter().find(|v| {
                v.as_object()
                    .and_then(|o| o.get("num"))
                    .and_then(|n| n.as_str())
                    == Some(&version_str)
            })
        });

    // The /versions endpoint doesn't include crate-level keywords/categories,
    // so fetch the crate info endpoint separately
    let crate_url = format!("{}/{}", CRATES_API_ROOT, name);
    if verbose {
        eprintln!("fetching crate metadata from {}", crate_url);
    }
    let crate_body: String = ureq::get(&crate_url)
        .header("User-Agent", USER_AGENT)
        .call()?
        .into_body()
        .read_to_string()?;
    let crate_json: serde_json::Value = serde_json::from_str(&crate_body)?;
    let crate_obj = crate_json.pointer("/crate");

    let str_field = |obj: Option<&serde_json::Value>, field: &str| -> Option<String> {
        obj.and_then(|o| o.get(field))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    let u64_field = |obj: Option<&serde_json::Value>, field: &str| -> Option<u64> {
        obj.and_then(|o| o.get(field)).and_then(|v| v.as_u64())
    };

    let keywords: Vec<String> = crate_json
        .pointer("/crate/keywords")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let categories: Vec<String> = crate_json
        .get("categories")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_object()
                        .and_then(|o| o.get("category"))
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();

    // Extract features from version object
    let features: Vec<String> = version_obj
        .and_then(|v| v.get("features"))
        .and_then(|f| f.as_object())
        .map(|obj| {
            let mut keys: Vec<String> = obj.keys().cloned().collect();
            keys.sort();
            keys
        })
        .unwrap_or_default();

    Ok(CrateMeta {
        name: name.to_string(),
        version: version_str,
        description: str_field(version_obj, "description")
            .or_else(|| str_field(crate_obj, "description")),
        license: str_field(version_obj, "license"),
        repository: str_field(version_obj, "repository")
            .or_else(|| str_field(crate_obj, "repository")),
        homepage: str_field(version_obj, "homepage").or_else(|| str_field(crate_obj, "homepage")),
        documentation: str_field(version_obj, "documentation")
            .or_else(|| str_field(crate_obj, "documentation")),
        rust_version: str_field(version_obj, "rust_version"),
        edition: str_field(version_obj, "edition"),
        crate_size: u64_field(version_obj, "crate_size"),
        downloads: u64_field(crate_obj, "downloads"),
        categories,
        keywords,
        features,
    })
}

/// Download given crate and extract to cache directory.
fn download_and_extract(
    name: &str,
    version: &Version,
    cache_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let download_url = format!("{}/{}/{}/download", CRATES_API_ROOT, name, version);

    let response = ureq::get(&download_url)
        .header("User-Agent", USER_AGENT)
        .call()?;

    let mut bytes = Vec::new();
    response.into_body().as_reader().read_to_end(&mut bytes)?;

    fs::create_dir_all(cache_root)?;

    let gz = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(cache_root)?;

    Ok(())
}

/// Find and read the README file in a crate directory.
fn find_readme(dir: &Path) -> Option<String> {
    let cargo_toml = dir.join("Cargo.toml");
    if let Ok(content) = fs::read_to_string(&cargo_toml) {
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("readme") {
                let rest = rest.trim();
                if let Some(rest) = rest.strip_prefix('=') {
                    let rest = rest.trim().trim_matches('"').trim_matches('\'');
                    if !rest.is_empty() {
                        let readme_path = dir.join(rest);
                        if let Ok(readme) = fs::read_to_string(&readme_path) {
                            return Some(readme);
                        }
                    }
                }
            }
        }
    }

    let candidates = [
        "README.md",
        "readme.md",
        "Readme.md",
        "README",
        "readme",
        "README.txt",
        "readme.txt",
        "README.rst",
        "readme.rst",
    ];

    for candidate in &candidates {
        let path = dir.join(candidate);
        if let Ok(content) = fs::read_to_string(&path) {
            return Some(content);
        }
    }

    None
}

/// List .rs and .md files in a crate directory, returning relative paths sorted.
fn list_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files);
    files.sort();
    files
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out);
        } else if let Some(ext) = path.extension() {
            if ext == "rs" || ext == "md" {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    // ── parse_crate_spec ──────────────────────────────────────────

    #[test]
    fn parse_spec_name_only() {
        let spec = parse_crate_spec("serde");
        assert_eq!(spec.name, "serde");
        assert!(spec.version_req.is_none());
    }

    #[test]
    fn parse_spec_with_spaces() {
        let spec = parse_crate_spec("  serde  ");
        assert_eq!(spec.name, "serde");
        assert!(spec.version_req.is_none());
    }

    #[test]
    fn parse_spec_exact_version() {
        let spec = parse_crate_spec("serde==1.0.200");
        assert_eq!(spec.name, "serde");
        assert_eq!(spec.version_req.as_deref(), Some("=1.0.200"));
    }

    #[test]
    fn parse_spec_caret_version() {
        let spec = parse_crate_spec("serde=^1.0");
        assert_eq!(spec.name, "serde");
        assert_eq!(spec.version_req.as_deref(), Some("^1.0"));
    }

    #[test]
    fn parse_spec_tilde_version() {
        let spec = parse_crate_spec("serde=~1.0");
        assert_eq!(spec.name, "serde");
        assert_eq!(spec.version_req.as_deref(), Some("~1.0"));
    }

    #[test]
    fn parse_spec_bare_version() {
        let spec = parse_crate_spec("serde=1.0.200");
        assert_eq!(spec.name, "serde");
        assert_eq!(spec.version_req.as_deref(), Some("1.0.200"));
    }

    #[test]
    fn parse_spec_empty_version() {
        let spec = parse_crate_spec("serde=");
        assert_eq!(spec.name, "serde");
        assert!(spec.version_req.is_none());
    }

    #[test]
    fn parse_spec_hyphenated_name() {
        let spec = parse_crate_spec("my-crate=^0.1");
        assert_eq!(spec.name, "my-crate");
        assert_eq!(spec.version_req.as_deref(), Some("^0.1"));
    }

    #[test]
    fn parse_spec_underscore_name() {
        let spec = parse_crate_spec("my_crate");
        assert_eq!(spec.name, "my_crate");
        assert!(spec.version_req.is_none());
    }

    // ── format_bytes ──────────────────────────────────────────────

    #[test]
    fn format_bytes_small() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(1023), "1023 bytes");
    }

    #[test]
    fn format_bytes_kb() {
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(81_715), "79.8 KB");
    }

    #[test]
    fn format_bytes_mb() {
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(5_242_880), "5.0 MB");
    }

    // ── format_number ─────────────────────────────────────────────

    #[test]
    fn format_number_small() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
    }

    #[test]
    fn format_number_thousands() {
        assert_eq!(format_number(1_000), "1.0K");
        assert_eq!(format_number(1_500), "1.5K");
        assert_eq!(format_number(70_525), "70.5K");
    }

    #[test]
    fn format_number_millions() {
        assert_eq!(format_number(1_000_000), "1.0M");
        assert_eq!(format_number(894_900_000), "894.9M");
    }

    // ── find_readme ───────────────────────────────────────────────

    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn make_temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("cargo-read-test-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn find_readme_standard_md() {
        let dir = make_temp_dir();
        fs::write(dir.join("README.md"), "# Hello").unwrap();
        assert_eq!(find_readme(&dir).as_deref(), Some("# Hello"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_readme_lowercase() {
        let dir = make_temp_dir();
        fs::write(dir.join("readme.md"), "lower").unwrap();
        assert_eq!(find_readme(&dir).as_deref(), Some("lower"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_readme_plain_text() {
        let dir = make_temp_dir();
        fs::write(dir.join("README"), "plain").unwrap();
        assert_eq!(find_readme(&dir).as_deref(), Some("plain"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_readme_from_cargo_toml() {
        let dir = make_temp_dir();
        fs::write(dir.join("Cargo.toml"), "readme = \"docs/ABOUT.md\"\n").unwrap();
        fs::create_dir_all(dir.join("docs")).unwrap();
        fs::write(dir.join("docs/ABOUT.md"), "custom readme").unwrap();
        assert_eq!(find_readme(&dir).as_deref(), Some("custom readme"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_readme_cargo_toml_missing_file_falls_back() {
        let dir = make_temp_dir();
        fs::write(dir.join("Cargo.toml"), "readme = \"MISSING.md\"\n").unwrap();
        fs::write(dir.join("README.md"), "fallback").unwrap();
        assert_eq!(find_readme(&dir).as_deref(), Some("fallback"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_readme_none() {
        let dir = make_temp_dir();
        assert!(find_readme(&dir).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_readme_priority_md_over_plain() {
        let dir = make_temp_dir();
        fs::write(dir.join("README.md"), "markdown").unwrap();
        fs::write(dir.join("README"), "plain").unwrap();
        assert_eq!(find_readme(&dir).as_deref(), Some("markdown"));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ── list_files / collect_files ────────────────────────────────

    #[test]
    fn list_files_basic() {
        let dir = make_temp_dir();
        fs::write(dir.join("lib.rs"), "").unwrap();
        fs::write(dir.join("README.md"), "").unwrap();
        fs::write(dir.join("data.json"), "").unwrap(); // should be excluded
        let files = list_files(&dir);
        assert_eq!(files, vec!["README.md", "lib.rs"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_files_nested() {
        let dir = make_temp_dir();
        fs::create_dir_all(dir.join("src/sub")).unwrap();
        fs::write(dir.join("src/main.rs"), "").unwrap();
        fs::write(dir.join("src/sub/helper.rs"), "").unwrap();
        fs::write(dir.join("CHANGELOG.md"), "").unwrap();
        let files = list_files(&dir);
        assert_eq!(
            files,
            vec!["CHANGELOG.md", "src/main.rs", "src/sub/helper.rs"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_files_empty_dir() {
        let dir = make_temp_dir();
        let files = list_files(&dir);
        assert!(files.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_files_excludes_non_rs_md() {
        let dir = make_temp_dir();
        fs::write(dir.join("Cargo.toml"), "").unwrap();
        fs::write(dir.join("build.py"), "").unwrap();
        fs::write(dir.join("data.csv"), "").unwrap();
        let files = list_files(&dir);
        assert!(files.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ── format_natural ────────────────────────────────────────────

    #[test]
    fn format_natural_minimal_meta() {
        let meta = CrateMeta {
            name: "foo".into(),
            version: "1.0.0".into(),
            ..Default::default()
        };
        let dir = PathBuf::from("/tmp/foo-1.0.0");
        let output = format_natural(&meta, &dir, None, &[], "foo");

        assert!(output.starts_with("---\n"));
        assert!(output.contains("crate: foo\n"));
        assert!(output.contains("version: 1.0.0\n"));
        assert!(output.contains("path: /tmp/foo-1.0.0\n"));
        assert!(output.contains("---\n"));
        assert!(output.contains("## Files\n"));
        assert!(output.contains("cargo read --api foo"));
        assert!(output.contains("cargo read --docs foo"));
        // No description/license/etc lines
        assert!(!output.contains("description:"));
        assert!(!output.contains("license:"));
    }

    #[test]
    fn format_natural_full_meta() {
        let meta = CrateMeta {
            name: "bar".into(),
            version: "2.3.4".into(),
            description: Some("A test crate".into()),
            license: Some("MIT".into()),
            repository: Some("https://github.com/test/bar".into()),
            homepage: Some("https://bar.dev".into()),
            documentation: Some("https://docs.rs/bar".into()),
            rust_version: Some("1.70".into()),
            edition: Some("2021".into()),
            crate_size: Some(51_200),
            downloads: Some(1_500_000),
            keywords: vec!["test".into(), "bar".into()],
            categories: vec!["Testing".into()],
            features: vec!["default".into(), "serde".into()],
        };
        let dir = PathBuf::from("/cache/bar-2.3.4");
        let readme = Some("# Bar\nA crate.\n");
        let files = vec!["README.md".into(), "src/lib.rs".into()];
        let output = format_natural(&meta, &dir, readme, &files, "bar");

        assert!(output.contains("description: A test crate\n"));
        assert!(output.contains("license: MIT\n"));
        assert!(output.contains("repository: https://github.com/test/bar\n"));
        assert!(output.contains("homepage: https://bar.dev\n"));
        assert!(output.contains("documentation: https://docs.rs/bar\n"));
        assert!(output.contains("rust-version: 1.70\n"));
        assert!(output.contains("edition: 2021\n"));
        assert!(output.contains("crate-size: 50.0 KB\n"));
        assert!(output.contains("downloads: 1.5M\n"));
        assert!(output.contains("keywords: test, bar\n"));
        assert!(output.contains("categories: Testing\n"));
        assert!(output.contains("features: default, serde\n"));
        assert!(output.contains("# Bar\nA crate.\n"));
        // Path separators vary by OS — check the filename portion
        assert!(output.contains("README.md\n"));
        assert!(output.contains("lib.rs\n"));
    }

    #[test]
    fn format_natural_homepage_same_as_repo_suppressed() {
        let meta = CrateMeta {
            name: "x".into(),
            version: "0.1.0".into(),
            repository: Some("https://github.com/a/b".into()),
            homepage: Some("https://github.com/a/b".into()),
            ..Default::default()
        };
        let dir = PathBuf::from("/tmp/x-0.1.0");
        let output = format_natural(&meta, &dir, None, &[], "x");
        assert!(output.contains("repository:"));
        assert!(!output.contains("homepage:"));
    }

    #[test]
    fn format_natural_readme_without_trailing_newline() {
        let meta = CrateMeta {
            name: "x".into(),
            version: "0.1.0".into(),
            ..Default::default()
        };
        let dir = PathBuf::from("/tmp/x-0.1.0");
        let output = format_natural(&meta, &dir, Some("no newline"), &[], "x");
        // Should still have a newline after the readme
        assert!(output.contains("no newline\n"));
    }

    // ── JSON output structure ─────────────────────────────────────

    #[test]
    fn json_output_has_flattened_meta() {
        let output = JsonOutput {
            meta: CrateMeta {
                name: "test".into(),
                version: "1.0.0".into(),
                license: Some("MIT".into()),
                ..Default::default()
            },
            path: "/tmp/test-1.0.0".into(),
            readme: Some("hello".into()),
            files: vec!["src/lib.rs".into()],
        };
        let json_str = serde_json::to_string(&output).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        // Flattened — no nested "meta" object
        assert_eq!(v["name"], "test");
        assert_eq!(v["version"], "1.0.0");
        assert_eq!(v["license"], "MIT");
        assert_eq!(v["path"], "/tmp/test-1.0.0");
        assert_eq!(v["readme"], "hello");
        assert_eq!(v["files"][0], "src/lib.rs");
        assert!(v.get("meta").is_none());
    }

    // ── Integration test (network) ────────────────────────────────

    #[test]
    #[ignore] // requires network — run with: cargo test -- --ignored
    fn integration_download_and_read() {
        let dir = std::env::temp_dir().join("cargo-read-integration-test");
        let _ = fs::remove_dir_all(&dir);

        let spec = CrateSpec {
            name: "whereat".into(),
            version_req: Some("=0.1.4".into()),
        };
        let (version, meta) = resolve_version_and_meta(&spec, false).unwrap();
        assert_eq!(version.to_string(), "0.1.4");
        assert_eq!(meta.license.as_deref(), Some("MIT OR Apache-2.0"));

        download_and_extract("whereat", &version, &dir).unwrap();
        let crate_dir = dir.join("whereat-0.1.4");
        assert!(crate_dir.exists());

        let readme = find_readme(&crate_dir);
        assert!(readme.is_some());
        assert!(readme.unwrap().contains("whereat"));

        let files = list_files(&crate_dir);
        assert!(files.contains(&"src/lib.rs".to_string()));
        assert!(files.iter().any(|f| f.ends_with(".md")));

        fs::remove_dir_all(&dir).unwrap();
    }
}
