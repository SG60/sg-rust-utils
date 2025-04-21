# sg-rust-utils
Various Rust utility libraries

## Release workflow

Using [cargo-release](https://github.com/crate-ci/cargo-release) for release workflow.

Set the new version using `cargo release version [minor, major etc.]`. Then use `cargo release` when ready to do the actual release (this also replaces the “Unreleased” section in `CHANGELOG.md`).
