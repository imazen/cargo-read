# cargo-read ![CI](https://img.shields.io/github/actions/workflow/status/imazen/cargo-read/ci.yml?style=flat-square&label=CI) ![crates.io](https://img.shields.io/crates/v/cargo-read?style=flat-square) [![lib.rs](https://img.shields.io/crates/v/cargo-read?style=flat-square&label=lib.rs&color=blue)](https://lib.rs/crates/cargo-read) ![docs.rs](https://img.shields.io/docsrs/cargo-read?style=flat-square) ![License](https://img.shields.io/crates/l/cargo-read?style=flat-square)

Download crate source and show README + metadata. Designed for LLM tool use.

## What it does

`cargo read` downloads a crate from crates.io, extracts it to a local cache, and prints:

1. **Frontmatter** — version, license, repository, MSRV, features, downloads, etc.
2. **README** — the crate's README content
3. **File listing** — absolute paths to every `.rs` and `.md` file (clickable in terminals/IDEs)

It always checks crates.io for the latest version, even if a cached copy exists. If the version hasn't changed, it uses the cache.

## Install

```
cargo install cargo-read
```

## Usage

```
cargo read serde
```

Default output:

```
---
crate: serde
version: 1.0.228
description: A generic serialization/deserialization framework
license: MIT OR Apache-2.0
repository: https://github.com/serde-rs/serde
documentation: https://docs.rs/serde
rust-version: 1.56
edition: 2021
crate-size: 81.7 KB
downloads: 894.9M
keywords: no_std, serde, serialization
features: alloc, default, derive, rc, std, unstable
path: /home/user/.cache/cargo-read/serde-1.0.228
---

[README content here]

## Files

/home/user/.cache/cargo-read/serde-1.0.228/README.md
/home/user/.cache/cargo-read/serde-1.0.228/src/lib.rs
...
```

### Version specifiers

```
cargo read serde              # latest
cargo read serde==1.0.200     # exact version
cargo read serde=^1.0         # semver-compatible
cargo read serde=~1.0         # tilde requirement
```

### Flags

| Flag | Description |
|------|-------------|
| `--json` | Output structured JSON with all metadata, readme, and file list |
| `--path-only` | Print only the extracted directory path |
| `--readme-only` | Print only the README content |
| `--force` | Re-download even if the version is already cached |
| `--cache-dir DIR` | Override the cache directory (default: `~/.cache/cargo-read/`) |
| `-v, --verbose` | Show download progress on stderr |

### JSON output

```
cargo read --json serde
```

Returns a single JSON object with all metadata fields at the top level, plus `path`, `readme`, and `files`.

## Cache

Crates are cached in `~/.cache/cargo-read/` (or `$XDG_CACHE_HOME/cargo-read/`, or `%LOCALAPPDATA%\cargo-read\` on Windows). Each version gets its own directory named `{crate}-{version}`.

The latest version is always checked against crates.io. If the cached version matches, no download happens.

## License

MIT (forked from [Xion/cargo-download](https://github.com/Xion/cargo-download))
