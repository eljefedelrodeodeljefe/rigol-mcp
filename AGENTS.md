# AGENTS.md — conventions for `rigol-mcp`

Ported from `mikrotik-mcp` so muscle memory transfers between the two.

## Branching and commits

`main` is the sole long-lived branch. There is no `develop`. Feature work branches off
`main` and returns via pull request.

Branch prefixes, kebab-case: `feat/`, `fix/`, `bugfix/`, `docs/`, `chore/`, `refactor/`.

Conventional commits drive release-please. `feat:` bumps minor, `fix:` and `perf:` bump
patch, `feat!:` or a `BREAKING CHANGE:` footer bumps major. `chore:`, `ci:`, `test:`,
`docs:` and `refactor:` do not affect the version. One logical change per commit.

## Layout

```text
crates/rigol/          library — no MCP types cross into here
  src/scpi.rs          transport: framing, timeouts, serialization, error queue
  src/dp800.rs         typed device model
  src/limits.rs        safety ceilings
  src/error.rs         typed errors
crates/rigol-mcp/      binary
  src/server.rs        MCP adapter — #[tool] stubs only, no logic
  src/tools/           pure logic, returns anyhow::Result<Value>
  src/params/          Deserialize + JsonSchema structs
  src/error.rs         maps failures onto MCP error codes
```

The seam between `rigol` and `rigol-mcp` is load-bearing: the library must stay usable
without an MCP runtime, because it publishes separately.

## Adding a tool

Three steps, in order:

1. A parameter struct in `crates/rigol-mcp/src/params/{domain}.rs`, deriving `Deserialize`
   and `JsonSchema`, with `#[schemars(description = "…")]` on each field.
2. Pure logic in `crates/rigol-mcp/src/tools/{domain}.rs` returning `anyhow::Result<Value>`.
   No MCP types here.
3. A `#[tool(description = "…")]` stub in `server.rs` that calls `self.guard_write()?`
   first if it mutates anything.

Tool descriptions mirror **RIGOL DP800 Programming Guide** terminology, the way
`mikrotik-mcp` mirrors the MikroTik wiki. Name the SCPI command in the description.

## Safety rules specific to this server

This drives a power supply. These are not negotiable:

- Ceilings are checked in `rigol::limits`, never in the tool layer. Do not add a parameter
  that relaxes them.
- Never make a setpoint write energise an output as a side effect.
- Every `output_on` logs channel and setpoints at info level.
- Never add anything that sweeps ports or brute-forces command spellings against an
  instrument. One wedged this unit for ninety seconds.
- Read state before proposing a write, and confirm with the user before energising.

## Gates

All commits must pass `prek run --all-files`:

- `cargo fmt`
- `cargo clippy --all-targets -- -D warnings` — **`#[allow]` escapes are refused**; fix the
  lint instead
- `cargo test --all`
- `typos`, TOML/YAML validation, markdownlint

## Runtime

stdout is the MCP protocol channel. All diagnostics go to stderr via `tracing`. Anything
printed to stdout corrupts the JSON-RPC stream.
