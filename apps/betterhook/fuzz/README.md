# betterhook fuzz targets

Adversarial inputs for the config parser and the dispatch path.

These targets build with [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
on a nightly toolchain. The fuzz crate is deliberately outside the
workspace so a normal `cargo build --workspace` doesn't need nightly.

## Install

```sh
cargo install cargo-fuzz
rustup toolchain install nightly
```

## Running

```sh
cd apps/betterhook/fuzz
cargo +nightly fuzz run config_parse
cargo +nightly fuzz run wrapper_paths
```

Corpora live under `fuzz/corpus/<target>/`; seed with real
`betterhook.toml` / `lefthook.yml` files for best coverage.

## Targets

| Target             | What it fuzzes                                           |
|--------------------|----------------------------------------------------------|
| `config_parse`     | Multi-format parser chain (TOML + YAML + JSON → RawConfig) |
| `wrapper_paths`    | `dispatch::find_config` against synthetic path strings   |

A panic in any target is a bug — file an issue with the minimized
reproducer from `fuzz/artifacts/<target>/`.
