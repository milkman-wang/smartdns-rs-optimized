# Contributing to SmartDNS-rs Optimized

This fork maintains one long-lived branch, `main`. Base changes on
[milkman-wang/smartdns-rs-optimized](https://github.com/milkman-wang/smartdns-rs-optimized),
not the original project's main branch.

## Workflow

Maintainer changes are reviewed and integrated on `main`; this repository does
not keep separate feature or release branches. External contributors can use a
short-lived branch in their own fork and open a pull request against this
repository's `main`.

Describe the problem, the resulting behavior and how you verified it. Keep
unrelated changes separate. Use Conventional Commits, for example:

```text
fix(cache): preserve negative response TTL
perf(rules): share domain rule storage
docs: update router installation instructions
```

## Validation

Use the checks relevant to the change:

- Rust code: formatting, Clippy and the affected tests; run the full suite for
  changes shared by multiple query paths.
- OpenWrt integration: the contract check and relevant tests under
  `contrib/openwrt/tests`.
- Documentation: check links, commands and the accuracy of referenced versions.
- Performance claims: record the exact binaries, hardware, configuration and
  repeated measurements. Keep raw data and a reproducible runner.

Common commands are `just fmt`, `just clippy` and `just test`. Build variants and
platform requirements are documented in [docs/BUILD_VARIANTS.md](docs/BUILD_VARIANTS.md).

## Code and tests

Keep changes small and follow existing patterns. Put unit tests at the end of
the source file, after public code. Verify actual returned values and behavior,
including relevant error paths; do not add tests that merely repeat the
implementation.

Configuration support is documented in
[the compatibility guide](docs/C_FEATURE_COMPATIBILITY.md) and
[the LuCI interface matrix](contrib/openwrt/INTERFACE_MATRIX.md).
Performance results are indexed in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

Bug reports should include the version, platform, a minimal configuration and
relevant logs, with credentials and private data removed.
