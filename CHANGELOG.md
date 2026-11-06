# Changelog

## [0.3.1](https://github.com/eljefedelrodeodeljefe/rigol-mcp/compare/v0.3.0...v0.3.1) (2026-11-06)


### Bug Fixes

* **deps:** upgrade rmcp to 3.9 ([cba8b7b](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/cba8b7b66a922d710b8f5858519183efb0a4df3e))
* **deps:** upgrade rmcp to 3.9 ([63d4900](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/63d4900fb34033c63789e507625c07ff87295e26))

## [0.3.0](https://github.com/eljefedelrodeodeljefe/rigol-mcp/compare/v0.2.0...v0.3.0) (2026-11-05)


### Features

* digital channels D0-D15 as the edge trigger source ([9864526](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/9864526576ea1ab1a91bd95a3850b27595e76fb9))
* logic analyser state, channel display and pod thresholds ([de2acca](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/de2acca39e5394d2a800888de4a6b599c1568bee))
* measurement statistics and their reset for the scope ([9344586](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/934458635bd6b8485f18751ea6201b8aa5493230))
* scope measurement statistics, digital edge trigger, logic analyser basics ([14b5fe2](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/14b5fe236080d790cb7be492366faa18144f4128))


### Refactors

* log through tracing in the probe, deny print macros ([212494e](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/212494e0db90312f49a99abe3cc7ff399f827063))
* log through tracing in the probe, deny print macros ([9f3a78e](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/9f3a78e3660ddea0206f54e535cb3a70f33a58fc))

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
