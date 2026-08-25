# rigol-mcp

Rust SDK and MCP server for RIGOL test instruments. First target: the **DP800-series**
programmable bench power supplies (verified against a **DP832A**, firmware `00.01.19`).

Two crates:

| Crate | What it is |
|-------|------------|
| [`rigol`](crates/rigol) | The library. SCPI over a raw TCP socket, a typed DP800 device model, and per-channel safety ceilings. No MCP dependency — usable on its own. |
| [`rigol-mcp`](crates/rigol-mcp) | The MCP server. Exposes the library as tools over stdio, built on [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk). |

## Safety

**This drives real power into real hardware.** A wrong write does not cost you
connectivity, it costs you a board. Three things follow from that:

1. **Per-channel ceilings are enforced in the library**, below the MCP layer. No tool
   argument, prompt, or future caller can bypass them — callers never see the check, they
   just get `Err(LimitExceeded)`.
2. **A ceiling is never unlimited.** If you do not configure one, it defaults to the
   channel's own rating as reported by the instrument (30V/3A, 30V/3A, 5V/3A on a DP832A).
3. **Setting a voltage and energising an output are separate operations.** Nothing turns an
   output on as a side effect of changing a setpoint, and every energisation is logged.

`RIGOL_ALLOW_WRITES` gates mutating tools, but understand what it is: **a convenience
guard, not a security boundary.** Anyone who can reach tcp/5555 can drive the instrument
directly, with or without this server. It protects against an agent making a mistake, not
against an attacker.

The ceilings likewise constrain *this client only*. The front panel and any other LAN
client are unaffected. This is not a hardware interlock.

## Configuration

| Variable | Default | Purpose |
|----------|---------|---------|
| `RIGOL_HOST` | *(required)* | Instrument address |
| `RIGOL_PORT` | `5555` | Raw SCPI socket |
| `RIGOL_TIMEOUT_MS` | `5000` | Read timeout |
| `RIGOL_ALLOW_WRITES` | `false` | Must be `true` for any mutating tool |
| `RIGOL_CH<n>_MAX_VOLTS` | channel rating | Safety ceiling, `n` = 1–3 |
| `RIGOL_CH<n>_MAX_AMPS` | channel rating | Safety ceiling, `n` = 1–3 |

`MAX_VOLTS` and `MAX_AMPS` must be set together for a given channel.

## Tools

Reads: `get_identity`, `get_channel`, `list_channels`, `measure_channel`,
`get_output_state`, `get_protection`, `get_safety_limits`, `get_lan_config`,
`get_instrument_errors`.

Writes (all behind `RIGOL_ALLOW_WRITES`): `set_voltage`, `set_current`, `set_protection`,
`output_on`, `output_off`.

A refused setpoint comes back as an MCP *invalid params* error, not an internal error — so
a model understands it should lower the value rather than retry the same call.

## Probing a live instrument

```sh
cargo run -p rigol --example probe -- <instrument-ip>
```

Read-only: no setpoint is written and no output is energised.

## Firmware quirks worth knowing

Measured against a DP832A on `00.01.19`. Several of these contradict the DP800 Programming
Guide, and cost real time to rediscover:

- **The raw SCPI port is 5555.** tcp/5025 — the LXI-conventional one — is closed. tcp/111
  serves VXI-11 via ONC-RPC portmap, but the raw socket has identical capability without
  the XDR machinery, so this crate does not implement VXI-11.
- **Abbreviated command forms win.** `:OUTP:OVP:ALAR?` is accepted; the guide's long-form
  `:OUTP:OVP:ALARm?` is rejected with `-113 Undefined header`.
- **The alarm queries answer `YES`/`NO`**, not `ON`/`OFF` like the state queries.
- **Several documented LAN queries do not exist here.** `:SYST:COMM:LAN:SMAS?`, `:AIP?`,
  `:MANU?` and `:VISA?` all return empty and push `-113`. The accepted spellings are
  `:MASK?`, `:AUTO?`, `:MAN?` — and even `:MASK?` returns an empty string, so "supported"
  and "populated" are separate facts. `get_lan_config` probes rather than assumes.
- **`:APPL? CHn` returns the channel rating *and* its setpoints** in one reply:
  `CH1:30V/3A,0.000,3.000`. That rating is what seeds an unconfigured safety ceiling.
- **The MAC comes back hyphen-separated** (`00-19-AF-…`); the library normalises it to colons.
- **The instrument is fragile under scanning.** A full `nmap -p-` sweep wedged it for about
  ninety seconds — it stopped answering entirely. Probe sequentially. Nothing in this crate
  sweeps.
- **It advertises no mDNS/LXI record**, so there is no name to discover it by. Address it
  by IP, or find it by its Rigol OUI `00:19:AF` in an ARP table.
- Latency is comfortable — around 10–110 ms per query on a quiet LAN. The 5 s default
  timeout is generous on purpose, not because the instrument is slow.

## Licence

MIT.
