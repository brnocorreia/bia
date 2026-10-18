# Pinned stable toolchain, MSRV equals the pin

`rust-toolchain.toml` pins an exact stable release (e.g. `1.99.1`) instead of the floating `stable` channel, and the workspace `rust-version` matches that release's minor version. A floating channel lets a new Rust release break CI on an unrelated PR, usually through new clippy lints, and a pin makes local runs and CI use the same compiler. bia ships as a binary rather than as a library other projects build, so supporting older compilers buys nothing. Moving to a newer Rust is a PR that bumps the pin and `rust-version` together and fixes whatever lints the new release adds.

## Considered Options

- Floating `stable`: rejected, because a new release could turn CI red with no change in the repository.
- An MSRV kept below the toolchain, checked by a separate CI job: rejected for now, because nothing downstream compiles bia's crates. Revisit if the crates are published.
