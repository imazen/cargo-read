# cargo-read — Project Instructions

Download crate source and show README + metadata. Designed for LLM tool use.

## Quick Commands

```bash
just check        # fmt + clippy + test
just fmt          # format only
just clippy       # clippy only
just test         # unit tests (no network)
just test-all     # all tests including network integration
just smoke        # cargo run -- serde
just outdated     # check dependency versions
```

## CI

GitHub Actions at `.github/workflows/ci.yml`:
- Tests on 6 platforms: ubuntu-latest, windows-latest, macos-latest, macos-26-intel, windows-11-arm, ubuntu-24.04-arm
- Clippy with `-D warnings`, fmt check
- Code coverage via cargo-llvm-cov + codecov
- Smoke test (`cargo run -- --path-only serde`) on every platform

## Design Notes

- Binary crate, no library API — all logic in `src/main.rs`
- `#![forbid(unsafe_code)]`
- Default output is human/LLM-readable frontmatter + README + file listing
- `--json` for structured output
- Always queries crates.io for latest version (2 API calls: `/versions` + `/crates/{name}`)
- Cache at `~/.cache/cargo-read/` (XDG_CACHE_HOME, LOCALAPPDATA on Windows)
- Yanked versions filtered out
- `format_natural()` returns a String for testability (not println directly)

## Known Bugs

(none currently)

## Investigation Notes

(none currently)
