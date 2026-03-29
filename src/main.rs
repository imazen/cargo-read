mod args;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;

use semver::{Version, VersionReq};
use serde::Serialize;

use args::{CrateSpec, parse_crate_spec};

const CRATES_API_ROOT: &str = "https://crates.io/api/v1/crates";

/// JSON output format — designed for LLM consumption.
#[derive(Serialize)]
struct Output {
    /// Crate name
    name: String,
    /// Resolved version string
    version: String,
    /// Absolute path to the extracted source directory
    path: String,
    /// README content, if found
    readme: Option<String>,
    /// Relative paths of .rs and .md files in the crate
    files: Vec<String>,
}

fn main() {
    let args = args::parse();
    let spec = parse_crate_spec(&args.crate_spec);

    // Resolve version from crates.io (always, even if cached)
    let version = match resolve_version(&spec, args.verbose) {
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
    } else {
        let output = Output {
            name: spec.name,
            version: version.to_string(),
            path: crate_dir.display().to_string(),
            readme,
            files,
        };
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }
}

fn default_cache_dir() -> PathBuf {
    dirs_cache().join("cargo-download")
}

/// Platform-appropriate cache directory.
fn dirs_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache");
    }
    // Fallback
    PathBuf::from("/tmp")
}

/// Query crates.io for the latest version matching the spec.
fn resolve_version(spec: &CrateSpec, verbose: bool) -> Result<Version, Box<dyn std::error::Error>> {
    let versions_url = format!("{}/{}/versions", CRATES_API_ROOT, spec.name);
    if verbose {
        eprintln!("fetching versions from {}", versions_url);
    }

    let body: String = ureq::get(&versions_url)
        .header(
            "User-Agent",
            "cargo-download (https://github.com/imazen/cargo-download)",
        )
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

    // Apply version requirement filter
    let version_req = match &spec.version_req {
        Some(req) => VersionReq::parse(req)?,
        None => VersionReq::STAR,
    };

    versions.sort_by(|a, b| b.cmp(a));

    versions
        .into_iter()
        .find(|v| version_req.matches(v))
        .ok_or_else(|| {
            format!(
                "no version of `{}` matches requirement `{}`",
                spec.name, version_req
            )
            .into()
        })
}

/// Download and extract a crate to the cache directory.
fn download_and_extract(
    name: &str,
    version: &Version,
    cache_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let download_url = format!("{}/{}/{}/download", CRATES_API_ROOT, name, version);

    let response = ureq::get(&download_url)
        .header(
            "User-Agent",
            "cargo-download (https://github.com/imazen/cargo-download)",
        )
        .call()?;

    let mut bytes = Vec::new();
    response.into_body().as_reader().read_to_end(&mut bytes)?;

    // Ensure cache directory exists
    fs::create_dir_all(cache_root)?;

    // Extract tar.gz — crates.io archives contain a single top-level dir named {crate}-{version}
    let gz = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(cache_root)?;

    Ok(())
}

/// Find and read the README file in a crate directory.
fn find_readme(dir: &Path) -> Option<String> {
    // Check Cargo.toml for a custom readme field first
    let cargo_toml = dir.join("Cargo.toml");
    if let Ok(content) = fs::read_to_string(&cargo_toml) {
        // Simple TOML parsing — look for readme = "..."
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

    // Common README filenames, in priority order
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
                    // Normalize to forward slashes for consistent cross-platform output
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
}
