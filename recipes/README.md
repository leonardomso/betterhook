# Recipes

Drop-in `betterhook.toml` configurations for common stacks. Each recipe is a
working starting point — copy it into your repo, edit the globs and tools,
and `betterhook install`.

## How to use a recipe

**Option A: copy the file.** Pick the closest match, copy to your repo as
`betterhook.toml`, edit to taste.

```sh
cp recipes/typescript.toml /path/to/your-repo/betterhook.toml
```

**Option B: extend it.** Keep recipes versioned in your org and inherit:

```toml
extends = ["./.betterhook/typescript.toml"]

[hooks.pre-commit.jobs.lint]
glob = ["src/**/*.ts"]    # override just the bits you need
```

Cross-format extends works (a TOML file can extend a YAML file).

## Available recipes

| Recipe | Stack | Highlights |
|---|---|---|
| [`typescript.toml`](typescript.toml) | TypeScript / JS monorepo | Prettier + ESLint + tsc, parallel, isolated `eslint`, sharded `tsc` |
| [`rust.toml`](rust.toml) | Rust workspace | rustfmt + clippy + cargo test, per-worktree `CARGO_TARGET_DIR` |
| [`python.toml`](python.toml) | Python project | Ruff (lint + format) + mypy, fail-fast off |
| [`go.toml`](go.toml) | Go module | gofmt + govet + go test, builtins where possible |
| [`polyglot.toml`](polyglot.toml) | Mixed-language monorepo | All four stacks above in one config, glob-routed |

## Validating a recipe locally

```sh
betterhook explain --hook pre-commit --worktree /path/to/repo
```

Prints the resolved DAG without executing. A non-zero exit means the config
didn't parse — exit code 2 with a `miette` diagnostic pointing at the line.
