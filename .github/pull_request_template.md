## Changes

<!-- Describe the user value delivered or the architectural risk eliminated. -->

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `python3 -m unittest discover -s tests/python -v`
- [ ] `python3 tools/check_dependency_provenance.py`
- [ ] Relevant Godot integration tests

## ABI and Supply Chain

- [ ] No Rust ABI is exposed across a dynamic-library boundary
- [ ] Godot-facing code and generated inputs have documented official provenance
- [ ] Changes to official API inputs include updated provenance, hashes, and snapshots
- [ ] Changes that affect product behavior are documented in `CHANGELOG.md`
