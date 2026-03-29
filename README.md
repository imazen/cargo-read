# cargo-read ![CI](https://img.shields.io/github/actions/workflow/status/imazen/cargo-read/ci.yml?style=flat-square&label=CI) ![crates.io](https://img.shields.io/crates/v/cargo-read?style=flat-square) [![lib.rs](https://img.shields.io/crates/v/cargo-read?style=flat-square&label=lib.rs&color=blue)](https://lib.rs/crates/cargo-read) ![docs.rs](https://img.shields.io/docsrs/cargo-read?style=flat-square) ![License](https://img.shields.io/crates/l/cargo-read?style=flat-square)

Download crate source and show README, metadata, API structure, and docs. Designed for LLM tool use.

## Install

```
cargo install cargo-read
```

## Progressive disclosure

Each command tells you what to run next for more detail.

```
cargo read serde                  # metadata + README + file listing
cargo read --api serde            # public API skeleton with signatures
cargo read --docs serde           # API with doc comments from source
cargo read --render-docs serde    # full compiled rustdoc → markdown (nightly)
```

## Default output

```
cargo read serde
```

```
---
crate: serde
version: 1.0.228
description: A generic serialization/deserialization framework
license: MIT OR Apache-2.0
repository: https://github.com/serde-rs/serde
rust-version: 1.56
edition: 2021
crate-size: 81.7 KB
downloads: 894.9M
features: alloc, default, derive, rc, std, unstable
path: /home/user/.cache/cargo-read/serde-1.0.228
---

[README content]

## Files

/home/user/.cache/cargo-read/serde-1.0.228/src/lib.rs
...

Hint: Run `cargo read --api serde` for API structure, `cargo read --docs serde` for API docs
```

## API skeleton

```
cargo read --api ureq
```

Shows every public type, function, trait, and re-export with fully-qualified module paths and clickable source locations. Extracted from source — no compilation needed.

## API docs

```
cargo read --docs anyhow
```

Same as `--api` but includes `///` doc comments and `//!` module-level docs.

## Full rendered docs (nightly)

```
cargo read --render-docs anyhow
```

Runs `cargo +nightly rustdoc --output-format json` and renders the compiled documentation to markdown. Includes resolved types, trait impls, method listings, enum variants — everything rustdoc knows. Requires nightly Rust; falls back to `--docs` if unavailable. Results are cached.

## Version specifiers

```
cargo read serde              # latest
cargo read serde==1.0.200     # exact version
cargo read serde=^1.0         # semver-compatible
cargo read serde=~1.0         # tilde requirement
```

## All flags

| Flag | Description |
|------|-------------|
| `--api` | Public API skeleton (source-level scan, no compilation) |
| `--docs` | API with doc comments (source-level extraction) |
| `--render-docs` | Full compiled rustdoc → markdown (requires nightly) |
| `--json` | Structured JSON output (combinable with `--api`) |
| `--path-only` | Print only the extracted directory path |
| `--readme-only` | Print only the README content |
| `--force` | Re-download even if the version is already cached |
| `--cache-dir DIR` | Override cache directory (default: `~/.cache/cargo-read/`) |
| `-v, --verbose` | Show progress on stderr |

## Cache

Crates are cached in `~/.cache/cargo-read/` (or `$XDG_CACHE_HOME/cargo-read/`, or `%LOCALAPPDATA%\cargo-read\` on Windows). Each version gets its own directory. The latest version is always checked against crates.io — if it matches the cache, no download happens.

## License

MIT (forked from [Xion/cargo-download](https://github.com/Xion/cargo-download))
