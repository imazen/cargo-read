use std::path::PathBuf;

use clap::Parser;

/// Download and cache crate source code.
///
/// Extracts to a cache directory and returns the path + README.
/// Always checks crates.io for the latest version, even if cached.
/// Designed for LLM tool use — outputs JSON by default.
#[derive(Parser, Debug)]
#[command(name = "cargo-download", bin_name = "cargo download", version)]
pub struct Args {
    /// Crate to download, optionally with version: CRATE or CRATE=VERSION
    ///
    /// VERSION uses Cargo.toml semver syntax:
    ///   serde          — latest version
    ///   serde==1.0.200 — exact version
    ///   serde=^1.0     — semver-compatible
    ///   serde=~1.0     — tilde requirement
    pub crate_spec: String,

    /// Override the cache directory (default: ~/.cache/cargo-download/)
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Output only the extracted directory path (no JSON, no README)
    #[arg(long)]
    pub path_only: bool,

    /// Output only the README content (no JSON, no path)
    #[arg(long, conflicts_with = "path_only")]
    pub readme_only: bool,

    /// Force re-download even if the version is already cached
    #[arg(long)]
    pub force: bool,

    /// Verbose output on stderr
    #[arg(short, long)]
    pub verbose: bool,
}

/// Parsed crate specification.
pub struct CrateSpec {
    pub name: String,
    pub version_req: Option<String>,
}

pub fn parse_crate_spec(spec: &str) -> CrateSpec {
    if let Some((name, version)) = spec.split_once('=') {
        let name = name.trim().to_string();
        // Preserve the version string as-is after the first '='.
        // "serde==1.0.200" splits to name="serde", version="=1.0.200"
        // The leading '=' is the semver exact-version operator, not a separator.
        let version = version.trim().to_string();
        CrateSpec {
            name,
            version_req: if version.is_empty() {
                None
            } else {
                Some(version)
            },
        }
    } else {
        CrateSpec {
            name: spec.trim().to_string(),
            version_req: None,
        }
    }
}

pub fn parse() -> Args {
    // Handle `cargo download` invocation where cargo passes "download" as first arg
    let mut raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() >= 2 && raw_args[1] == "download" {
        raw_args.remove(1);
    }
    Args::parse_from(raw_args)
}
