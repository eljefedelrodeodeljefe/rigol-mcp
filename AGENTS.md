# AGENTS.md: conventions for `rigol-mcp`

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
  src/scpi.rs          transport: framing, timeouts, blocks, poison-and-reconnect, error queue
  src/idn.rs           *IDN? identity and the instrument Family it implies
  src/dp800.rs         DP800 power supply model
  src/dho900.rs        DHO800/DHO900 oscilloscope model
  src/limits.rs        PSU safety ceilings
  src/generator.rs     scope signal-generator ceilings and safe command ordering
  src/error.rs         typed errors
  examples/probe.rs    read-only, family-aware probe of a live instrument
crates/rigol-mcp/      binary
  src/server.rs        MCP adapter — #[tool] stubs only, no logic; one router per family
  src/tools/           pure logic per family, returns anyhow::Result<Value>
  src/params/          Deserialize + JsonSchema structs per family
  src/error.rs         maps failures onto MCP error codes
crates/rigol-fake/     test-only scripted fake instrument (unpublished)
```

One process drives one instrument family, chosen by `RIGOL_FAMILY`. The PSU and scope
tool sets live on separate named routers and never share a process; tool names may repeat
across them (`get_identity`) where the meaning is the same.

The seam between `rigol` and `rigol-mcp` is load-bearing: the library must stay usable
without an MCP runtime, because it publishes separately.

## Adding a tool

Add a tool in this order:

1. A parameter struct in `crates/rigol-mcp/src/params/{domain}.rs`, deriving `Deserialize`
   and `JsonSchema`, with `#[schemars(description = "…")]` on each field.
2. Pure logic in `crates/rigol-mcp/src/tools/{domain}.rs` returning `anyhow::Result<Value>`.
   No MCP types here.
3. A `#[tool(description = "…")]` stub in `server.rs`, on the router for its family
   (`psu_router` or `scope_router`), that calls `self.guard_write()?` first if it mutates
   anything.

Tool descriptions mirror the **RIGOL DP800** and **DHO800/DHO900 Programming Guide**
terminology, the way `mikrotik-mcp` mirrors the MikroTik wiki. Name the SCPI command in the
description.

New SCPI commands come from the Programming Guide and are checked one at a time on a real
instrument, with `examples/probe.rs`, before any tool depends on them. Instrument parameters
are closed enums, because on DHO firmware an unknown query gets no reply at all.
Tests run against `rigol-fake` with replies captured from the instrument.

## Safety rules specific to this server

This drives a power supply and a signal generator into real hardware. Every change keeps
these rules:

- Ceilings are checked only in the library (`rigol::limits`, `rigol::generator`), below the
  tool layer. Do not add a parameter that relaxes them.
- Generator ceilings bound each value and the peak, `|offset| + amplitude/2`. Keep
  `generator::plan` ordering commands so every intermediate state stays inside them, and
  keep its exhaustive test.
- Never make a setpoint or generator configuration write energise an output as a side
  effect.
- Every write logs one info line with its operation, channel and values; every `output_on`
  and `generator_output_on` logs the settings it energises.
- `output_on` re-checks the channel's voltage and current setpoints against the ceilings, and
  `generator_output_on` re-checks the generator's settings, before turning on. A setpoint
  can change on the front panel.
- Ceilings are finite and above zero. A NaN ceiling compares false against every request, so
  every constructor validates and every check refuses a non-finite request.
- A generator or PSU write holds the instrument's operation lock from its check to its last
  command. rmcp runs requests concurrently, and two writes planned from the same state can
  each fit the ceilings and exceed them together.
- Send each SCPI command and its newline in one write. DHO firmware drops the reply to a
  command split across TCP segments.
- Never add anything that sweeps ports or brute-forces command spellings against an
  instrument. One wedged this unit for ninety seconds.
- Read state before proposing a write, and confirm with the user before energising.

## Gates

All commits must pass `prek run --all-files`:

- `cargo fmt`
- `cargo clippy --all-targets -- -D warnings`: fix the lint where you can. Where a lint is
  wrong for one site, put `#[expect(clippy::lint, reason = "…")]` on that site; it errors
  once the lint stops firing. **A bare `#[allow]` is refused.** A crate- or workspace-wide
  allow goes in `[workspace.lints]` with a comment giving the reason.
- `cargo test --all`
- `typos`, TOML/YAML validation, markdownlint

## Security scanning

`sbom-scan.yml` builds a CycloneDX and SPDX SBOM from `Cargo.lock` with Syft and scans it
with Grype, both installed from pinned, checksum-verified release tarballs.

- At release time it runs **informational**: findings are reported and the SBOMs are
  attached to the release, but nothing blocks a release that is already published.
- The weekly schedule runs **gating**: it fails on CRITICAL or HIGH findings that have a fix
  available. Fix those by bumping the dependency in `Cargo.lock`. A finding with no fix stays
  unsuppressed and is noted in the PR that triages it.

## Runtime

stdout is the MCP protocol channel. All diagnostics go to stderr via `tracing`. Anything
printed to stdout corrupts the JSON-RPC stream. The workspace denies `print_stdout`,
`print_stderr` and `dbg_macro`, so `println!` fails clippy everywhere, examples included.
