---
id: SEED-001
status: dormant
planted: 2026-03-24
planted_during: code cleanup / deduplication work
trigger_when: code quality or observability milestone
scope: Small
---

# SEED-001: Switch logging from eprintln!/println! to tracing (or log crate)

## Why This Matters

The codebase uses raw `eprintln!` and `println!` for all diagnostics — warnings, recovery messages, connection status, errors. This means:
- No log levels (can't filter warnings vs info vs debug)
- No structured output (harder to parse in production)
- No runtime control over verbosity
- The `LogOnce` utility we just extracted still writes to stderr with no level distinction

A proper logging framework (even just the `log` crate with `env_logger`) would give level filtering, timestamps, and module-path context for free.

## When to Surface

**Trigger:** When a milestone focuses on code quality, observability, or maintainability.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Code quality or maintainability focus
- Observability, logging, or debugging improvements
- Production hardening or operational tooling

## Scope Estimate

**Small** — There are ~40 `eprintln!`/`println!` call sites. The work is mostly:
1. Add `log` or `tracing` dependency
2. Replace `eprintln!("Warning: ...")` with `warn!(...)`, `eprintln!("...")` with `info!(...)`, etc.
3. Update `LogOnce` in `src/rate_limit.rs` to use `warn!`/`info!` instead of `eprintln!`
4. Initialize a subscriber/logger in `main()`

## Breadcrumbs

Key files with `eprintln!`/`println!` usage:
- `src/main.rs` — session detection, dispatch errors
- `src/rate_limit.rs` — `LogOnce` struct (recently extracted)
- `src/config.rs` — config parse warnings, reload status
- `src/backlight.rs` — brightness control warnings
- `src/volume.rs` — PulseAudio connection status
- `src/reconnect.rs` — inotify errors
- `src/workspace/niri.rs` — niri socket connection/disconnection
- `src/widgets/mod.rs` — widget fd registration, icon lookup
- `src/iced_renderer.rs` — screenshot errors

## Notes

The `tracing` crate is heavier than needed for this embedded touchbar daemon. The `log` crate with `env_logger` is likely the right fit — minimal overhead, `RUST_LOG` env var for filtering.
