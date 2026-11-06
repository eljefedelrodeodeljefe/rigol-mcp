# Changelog

## [0.4.0](https://github.com/eljefedelrodeodeljefe/rigol-mcp/compare/v0.3.1...v0.4.0) (2026-11-06)


### ⚠ BREAKING CHANGES

* Dp800::session() and Dho900::session() are removed. GeneratorLimits and Rating fields are private; read them with max_vpp()/max_peak_volts() and volts()/amps().
* rigol::Rating::new now returns Result<Rating>.

### Bug Fixes

* close the library paths around the safety ceilings ([ffae194](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/ffae1944ba8c6ad0bb2d8531c26768b66847d62e))
* hold an operation lock across generator and PSU write sequences ([92f2efc](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/92f2efc9652c908b4f6d7ee0af78d2666de4f515))
* log discarded results instead of dropping them silently ([0ce0faf](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/0ce0faf3a626b26792688969ccc6a86f1c59f0a5))
* log one info line for every instrument write ([d82b00d](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/d82b00deff6cd02dba938e298bf0d32f1bfcf044))
* **mcp:** log at info unless RUST_LOG says otherwise ([e0c43a9](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/e0c43a9107000f1554b1a30f23c1c843d1459b79))
* **mcp:** parse boolean environment variables strictly ([9c39b1d](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/9c39b1d15d2cb4f8c1b163e9b0272935229bade7))
* re-check PSU setpoints against the ceilings before energising ([280f8f1](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/280f8f13a73caa8b6d8bd43ab71d265ba6860293))
* refuse non-finite and non-positive safety ceilings ([f3deb94](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/f3deb94639cf4fec87d000c25fedd6207a673528))
* **scpi:** poison a session whose exchange is dropped mid-way ([4aeb29b](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/4aeb29b0b743620175a7da3dead17fc55743acc2))


### Refactors

* **mcp:** read configuration through a lookup function ([2fe95c5](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/2fe95c50587d9ce2e22671c71b886592d45de449))


### Documentation

* **agents:** lint escapes via #[expect], and the new safety rules ([e3a4ecb](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/e3a4ecb1f42c45875446f0a51d54cff82a825c3d))
* **readme:** the PSU re-checks its setpoints at output-on too ([161efcf](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/161efcf01de345ffedd9a31e85f8e86253674132))
* sweep README and AGENTS prose against the de-AI style rules ([91c9e8e](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/91c9e8eb234cd83e18776331ced0b72ccaa67a20))
* sweep README and AGENTS prose against the de-AI style rules ([a3164cb](https://github.com/eljefedelrodeodeljefe/rigol-mcp/commit/a3164cb6d81b1076ef807a39e827eb8580fb6aee))

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
