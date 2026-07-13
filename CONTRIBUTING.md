# Contributing

The private Paymos monorepo is the source of truth. Public repository changes may
be overwritten by the automated mirror release process.

Every change must keep `conformance/contract.json` byte-identical to the canonical
contract and pass:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo package --locked
```

Do not add credentials, generated `target/` output, or examples that convert
monetary amounts through floating point.

