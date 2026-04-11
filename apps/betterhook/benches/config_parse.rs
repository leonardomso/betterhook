//! Benchmark the TOML/YAML/JSON parsers and the lowering step.
//!
//! Run with:
//!
//!     cargo bench -p betterhook --bench config_parse

use betterhook::config::{Format, parse_bytes};
use criterion::{Criterion, criterion_group, criterion_main};

const SAMPLE_TOML: &str = r#"
[meta]
version = 1

[hooks.pre-commit]
parallel = true
priority = ["lint", "test", "fmt"]

[hooks.pre-commit.jobs.lint]
run = "eslint --cache --fix {staged_files}"
glob = ["*.ts", "*.tsx"]
exclude = ["**/*.gen.ts"]
stage_fixed = true
isolate = "eslint"
timeout = "60s"

[hooks.pre-commit.jobs.test]
run = "cargo test --quiet"

[hooks.pre-commit.jobs.fmt]
run = "cargo fmt --all -- --check"

[hooks.pre-push.jobs.audit]
run = "cargo audit"
"#;

const SAMPLE_YAML: &str = r#"
meta:
  version: 1
hooks:
  pre-commit:
    parallel: true
    priority: [lint, test, fmt]
    jobs:
      lint:
        run: "eslint --cache --fix {staged_files}"
        glob: ["*.ts", "*.tsx"]
        exclude: ["**/*.gen.ts"]
        stage_fixed: true
        isolate: "eslint"
        timeout: "60s"
      test:
        run: "cargo test --quiet"
      fmt:
        run: "cargo fmt --all -- --check"
  pre-push:
    jobs:
      audit:
        run: "cargo audit"
"#;

fn bench_parsers(c: &mut Criterion) {
    let mut group = c.benchmark_group("config_parse");
    group.bench_function("toml_parse", |b| {
        b.iter(|| {
            let raw = parse_bytes(SAMPLE_TOML, Format::Toml, "betterhook.toml").unwrap();
            std::hint::black_box(raw);
        });
    });
    group.bench_function("yaml_parse", |b| {
        b.iter(|| {
            let raw = parse_bytes(SAMPLE_YAML, Format::Yaml, "betterhook.yml").unwrap();
            std::hint::black_box(raw);
        });
    });
    group.bench_function("toml_parse_lower", |b| {
        b.iter(|| {
            let raw = parse_bytes(SAMPLE_TOML, Format::Toml, "betterhook.toml").unwrap();
            let cfg = raw.lower().unwrap();
            std::hint::black_box(cfg);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_parsers);
criterion_main!(benches);
