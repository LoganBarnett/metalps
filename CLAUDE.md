# metalps — Claude Code instructions

## Development environment

**Always run cargo commands inside the nix develop shell.**

This project uses Nix flakes to provide the Rust toolchain (1.93+), rustfmt,
and other cargo tools.  Outside the shell these are not in PATH.

```sh
# Enter the shell interactively
nix develop

# Or prefix individual commands
nix develop --command cargo build --workspace
nix develop --command cargo test --workspace
nix develop --command cargo clippy --workspace
```

**Git commits must also go through the shell** — the pre-commit hook runs
`rustfmt` and will fail if it isn't in PATH:

```sh
nix develop --command git commit -m "..."
```

## Build & test

```sh
nix develop --command cargo build --workspace
nix develop --command cargo test --workspace
```

## Code style

- 2-space indentation, 80-column line width (enforced by `rustfmt.toml`)
- Semantic error types via `thiserror`; logging to stderr, output to stdout
- Logging with `tracing`; never print diagnostics to stdout
