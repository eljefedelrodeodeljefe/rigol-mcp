# Changelog

## [0.2.0](https://github.com/eljefedelrodeodeljefe/rigol-mcp/compare/v0.1.0...v0.2.0) (2026-10-06)


### Features

* classify instruments by family, refuse the wrong one at connect ([3109288](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/31092889efc29936fdf1fa087e79c282cab1ae30))
* DHO800/DHO900 oscilloscope model, read side (rigol::Dho900) ([5330710](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/5330710bdbae7761d95c32d04e436b12cf38a90b))
* oscilloscope configuration writes in rigol::Dho900 ([79e22db](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/79e22dbc62631e40aa3423912d77128d1ed5fca5))
* oscilloscope read tools for RIGOL_FAMILY=dho900 ([ca69fa0](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/ca69fa01563125d858fdca9b62db79d080a62fd3))
* oscilloscope write tools behind RIGOL_ALLOW_WRITES ([6f983eb](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/6f983eb96244a52235ac2c2fe953de9454bd4ee8))
* pick the tool set per instrument family (RIGOL_FAMILY) ([9e8dc10](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/9e8dc10a6d3c96267545f919661f63bc32799a7a))
* probe detects the family and maps a scope's query set ([b13d922](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/b13d922b5544009345d9aa33562e29e37bcc58b5))
* read IEEE 488.2 binary blocks (Session::query_block) ([dee030f](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/dee030f8e82f6c333187f7e63c56d04aa3205a32))
* Rust SDK and MCP server for RIGOL DP800-series bench PSUs ([f53553f](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/f53553fcf35fab3634cbb23612e7df76128fd50f))
* signal generator tools with RIGOL_GEN_MAX_VPP / RIGOL_GEN_MAX_PEAK_VOLTS ([3a7398f](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/3a7398f8213e2de292cee7303bcd13943fe62b9e))
* signal generator with peak-voltage ceilings (rigol::generator) ([47d7aed](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/47d7aede075f8396d7e1ae55809e85910568e7c8))


### Bug Fixes

* never pair a late reply with the next query ([b9e9307](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/b9e9307a6449d4b3842e98dd55f346bbde95ea2d))
* send each command and its newline in one write ([3109a43](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/3109a4356e1bb54d8c06965efefba3b99fc1d4d4))


### Documentation

* README for people landing on the repo; AGENTS for the scope ([e9bd874](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/e9bd8741cabdf49e05f68958fa874b26d7a9f7ed))
