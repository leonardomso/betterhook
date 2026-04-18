//! Measure the per-line overhead of the `OutputEvent` NDJSON encoder.
//!
//! Run with:
//!
//!     cargo bench -p betterhook --bench output_multiplexer

use betterhook::runner::{OutputEvent, Stream};
use criterion::{Criterion, criterion_group, criterion_main};

fn bench_ndjson_serialize(c: &mut Criterion) {
    let event = OutputEvent::Line {
        job: "lint".to_owned(),
        stream: Stream::Stdout,
        line: "src/foo.ts: 42:10  error  'bar' is unused".to_owned(),
    };
    c.bench_function("output_line_serialize", |b| {
        b.iter(|| {
            let s = serde_json::to_string(&event).unwrap();
            std::hint::black_box(s);
        });
    });
}

criterion_group!(benches, bench_ndjson_serialize);
criterion_main!(benches);
