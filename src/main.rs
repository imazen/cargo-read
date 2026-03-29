mod args;

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
#[derive(Default, Serialize)]
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
    } else if args.json {
        let output = JsonOutput {
            meta,
            path: crate_dir.display().to_string(),
            readme,
            files,
        };
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        print_natural(&meta, &crate_dir, readme.as_deref(), &files);
    }
}

/// Print the default human/LLM-readable output.
fn print_natural(meta: &CrateMeta, crate_dir: &Path, readme: Option<&str>, files: &[String]) {
    // Frontmatter
    println!("---");
    println!("crate: {}", meta.name);
    println!("version: {}", meta.version);
    if let Some(ref desc) = meta.description {
        println!("description: {}", desc);
    }
    if let Some(ref license) = meta.license {
        println!("license: {}", license);
    }
    if let Some(ref repo) = meta.repository {
        println!("repository: {}", repo);
    }
    if let Some(ref hp) = meta.homepage {
        if meta.repository.as_deref() != Some(hp) {
            println!("homepage: {}", hp);
        }
    }
    if let Some(ref docs) = meta.documentation {
        println!("documentation: {}", docs);
    }
    if let Some(ref msrv) = meta.rust_version {
        println!("rust-version: {}", msrv);
    }
    if let Some(ref ed) = meta.edition {
        println!("edition: {}", ed);
    }
    if let Some(size) = meta.crate_size {
        println!("crate-size: {}", format_bytes(size));
    }
    if let Some(dl) = meta.downloads {
        println!("downloads: {}", format_number(dl));
    }
    if !meta.keywords.is_empty() {
        println!("keywords: {}", meta.keywords.join(", "));
    }
    if !meta.categories.is_empty() {
        println!("categories: {}", meta.categories.join(", "));
    }
    if !meta.features.is_empty() {
        println!("features: {}", meta.features.join(", "));
    }
    println!("path: {}", crate_dir.display());
    println!("---");

    // README
    if let Some(readme) = readme {
        println!();
        print!("{}", readme);
        if !readme.ends_with('\n') {
            println!();
        }
    }

    // File listing with absolute paths
    println!();
    println!("## Files");
    println!();
    for f in files {
        let abs = crate_dir.join(f);
        println!("{}", abs.display());
    }
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
    // Fetch version list to resolve the best match
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
