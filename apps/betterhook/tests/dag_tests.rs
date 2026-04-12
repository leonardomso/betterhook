#![allow(clippy::cast_sign_loss)]
mod common;

use std::collections::BTreeMap;

use betterhook::config::Job;
use betterhook::runner::dag::{build_dag, DagError};

/// Build a [`Job`] with the fields the DAG resolver cares about.
/// Everything else gets sensible defaults.
fn job(
    name: &str,
    reads: &[&str],
    writes: &[&str],
    priority: u32,
    network: bool,
    concurrent_safe: bool,
) -> Job {
    Job {
        name: name.to_owned(),
        run: "true".to_owned(),
        fix: None,
        glob: Vec::new(),
        exclude: Vec::new(),
        tags: Vec::new(),
        skip: None,
        only: None,
        env: BTreeMap::new(),
        root: None,
        stage_fixed: false,
        isolate: None,
        timeout: None,
        interactive: false,
        fail_text: None,
        priority,
        reads: reads.iter().map(|s| (*s).to_owned()).collect(),
        writes: writes.iter().map(|s| (*s).to_owned()).collect(),
        network,
        concurrent_safe,
        builtin: None,
    }
}

// ── Topology tests ──────────────────────────────────────────────────

#[test]
fn empty_jobs_empty_graph() {
    let dag = build_dag(&[]).unwrap();
    assert_eq!(dag.nodes.len(), 0);
    assert_eq!(dag.edge_count(), 0);
}

#[test]
fn single_job_is_root() {
    let dag = build_dag(&[job("only", &[], &["*.ts"], 0, false, false)]).unwrap();
    assert_eq!(dag.nodes.len(), 1);
    assert_eq!(dag.roots(), vec![0]);
    assert_eq!(dag.edge_count(), 0);
}

#[test]
fn two_disjoint_jobs_both_roots() {
    let jobs = vec![
        job("rust", &["**/*.rs"], &[], 0, false, false),
        job("ts", &["**/*.ts"], &[], 1, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 0);
    assert_eq!(dag.roots(), vec![0, 1]);
}

#[test]
fn read_after_write_creates_edge() {
    let jobs = vec![
        job("format", &[], &["**/*.ts"], 0, false, false),
        job("lint", &["**/*.ts"], &[], 1, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 1);
    assert_eq!(dag.nodes[0].children, vec![1]);
    assert_eq!(dag.nodes[1].parents, vec![0]);
}

#[test]
fn write_write_conflict_creates_edge() {
    let jobs = vec![
        job("fmt1", &[], &["**/*.ts"], 0, false, false),
        job("fmt2", &[], &["**/*.ts"], 1, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 1);
}

#[test]
fn bidirectional_conflict_uses_priority() {
    // Both write *.ts — lower priority number runs first.
    let jobs = vec![
        job("a", &[], &["**/*.ts"], 0, false, false),
        job("b", &[], &["**/*.ts"], 1, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 1);
    // a (pri=0) is parent, b (pri=1) is child
    assert_eq!(dag.nodes[0].children, vec![1]);
    assert_eq!(dag.nodes[1].parents, vec![0]);
}

#[test]
fn diamond_dependency() {
    // A writes *.ts, B reads *.ts + writes *.js,
    // C reads *.ts, D reads *.js (from B)
    let jobs = vec![
        job("a", &[], &["**/*.ts"], 0, false, false),
        job("b", &["**/*.ts"], &["**/*.js"], 1, false, false),
        job("c", &["**/*.ts"], &[], 2, false, false),
        job("d", &["**/*.js"], &[], 3, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();

    // A -> B (A writes ts, B reads ts)
    assert!(dag.nodes[0].children.contains(&1));
    // A -> C (A writes ts, C reads ts)
    assert!(dag.nodes[0].children.contains(&2));
    // B -> D (B writes js, D reads js)
    assert!(dag.nodes[1].children.contains(&3));
    // A is the sole root
    assert!(dag.nodes[0].parents.is_empty());
    assert!(dag.edge_count() >= 3);
}

#[test]
fn fan_out_one_writer_many_readers() {
    let mut jobs = vec![job("writer", &[], &["**/*.ts"], 0, false, false)];
    for i in 0..10 {
        jobs.push(job(
            &format!("reader-{i}"),
            &["**/*.ts"],
            &[],
            (i + 1) as u32,
            false,
            false,
        ));
    }
    let dag = build_dag(&jobs).unwrap();
    // The single writer fans out to all 10 readers.
    assert_eq!(dag.nodes[0].children.len(), 10);
    assert_eq!(dag.edge_count(), 10);
}

#[test]
fn fan_in_many_writers_one_reader() {
    let mut jobs: Vec<Job> = (0..10)
        .map(|i| {
            job(
                &format!("writer-{i}"),
                &[],
                &["**/*.ts"],
                i as u32,
                false,
                false,
            )
        })
        .collect();
    jobs.push(job("reader", &["**/*.ts"], &[], 100, false, false));
    let dag = build_dag(&jobs).unwrap();

    // The reader should have parents (writers that must finish first).
    let reader_idx = jobs.len() - 1;
    assert!(!dag.nodes[reader_idx].parents.is_empty());
    // All writers also conflict with each other (write-write on *.ts),
    // so total edges > 10.
    assert!(dag.edge_count() >= 1);
}

#[test]
fn chain_of_three() {
    // A writes *.ts, B reads *.ts + writes *.js, C reads *.js
    let jobs = vec![
        job("a", &[], &["**/*.ts"], 0, false, false),
        job("b", &["**/*.ts"], &["**/*.js"], 1, false, false),
        job("c", &["**/*.js"], &[], 2, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert!(dag.nodes[0].children.contains(&1)); // A -> B
    assert!(dag.nodes[1].children.contains(&2)); // B -> C
    // Only A is a root.
    assert_eq!(dag.roots(), vec![0]);
}

#[test]
fn fully_disjoint_50_jobs() {
    let jobs: Vec<Job> = (0..50)
        .map(|i| {
            job(
                &format!("job-{i}"),
                &[],
                &[&format!("unique-dir-{i}/*.txt")],
                i as u32,
                false,
                false,
            )
        })
        .collect();
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.roots().len(), 50);
    assert_eq!(dag.edge_count(), 0);
}

#[test]
fn all_shared_writers_form_total_order() {
    // 5 jobs all writing *.ts — the DAG must serialize them into a
    // chain of 4 edges (total order by priority).
    let jobs: Vec<Job> = (0..5)
        .map(|i| {
            job(
                &format!("fmt-{i}"),
                &[],
                &["**/*.ts"],
                i as u32,
                false,
                false,
            )
        })
        .collect();
    let dag = build_dag(&jobs).unwrap();
    // n*(n-1)/2 = 10 edges for a fully connected tournament of 5 nodes,
    // because every pair has a write-write conflict.
    assert_eq!(dag.edge_count(), 10);
    // Only the first (lowest priority) is a root.
    assert_eq!(dag.roots(), vec![0]);
}

// ── Network tests ───────────────────────────────────────────────────

#[test]
fn network_jobs_serialize() {
    let jobs = vec![
        job("a", &[], &[], 0, true, false),
        job("b", &[], &[], 1, true, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 1);
}

#[test]
fn concurrent_safe_network_jobs_parallel() {
    let jobs = vec![
        job("a", &[], &[], 0, true, true),
        job("b", &[], &[], 1, true, true),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 0);
}

#[test]
fn one_network_one_safe_serializes() {
    // The network rule fires when NOT both are concurrent_safe.
    let jobs = vec![
        job("a", &[], &[], 0, true, false),
        job("b", &[], &[], 1, true, true),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 1);
}

// ── Priority tests ──────────────────────────────────────────────────

#[test]
fn priority_determines_edge_direction() {
    // A has priority 5, B has priority 1 — B should run first.
    let jobs = vec![
        job("a", &[], &["**/*.ts"], 5, false, false),
        job("b", &[], &["**/*.ts"], 1, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 1);
    // B (index 1, pri=1) is parent, A (index 0, pri=5) is child.
    assert!(dag.nodes[1].children.contains(&0));
    assert!(dag.nodes[0].parents.contains(&1));
}

#[test]
fn equal_priority_breaks_on_declaration_order() {
    let jobs = vec![
        job("first", &[], &["a.txt"], 0, false, false),
        job("second", &[], &["a.txt"], 0, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    // Earlier declaration (index 0) runs first.
    assert_eq!(dag.nodes[0].children, vec![1]);
}

// ── Error tests ─────────────────────────────────────────────────────

#[test]
fn invalid_glob_in_reads_errors() {
    let j = job("bad", &["[unclosed"], &[], 0, false, false);
    let err = build_dag(&[j]).unwrap_err();
    assert!(matches!(err, DagError::Glob { .. }));
}

#[test]
fn invalid_glob_in_writes_errors() {
    let j = job("bad", &[], &["[unclosed"], 0, false, false);
    let err = build_dag(&[j]).unwrap_err();
    assert!(matches!(err, DagError::Glob { .. }));
}

#[test]
fn empty_glob_pattern_is_fine() {
    // An empty string pattern should not panic. Whether it errors or
    // succeeds is implementation-defined; we just assert no panic.
    let j = job("empty", &[""], &[], 0, false, false);
    let _ = build_dag(&[j]);
}

// ── Stress tests ────────────────────────────────────────────────────

#[test]
fn hundred_job_dag_builds_without_panic() {
    let jobs: Vec<Job> = (0..100)
        .map(|i| {
            // Even-indexed jobs write to a shared glob, odd ones to a unique
            // glob. This produces a mix of conflicts and parallelism.
            let write_pat = if i % 2 == 0 {
                "src/**/*.ts".to_owned()
            } else {
                format!("pkg-{i}/**/*.rs")
            };
            job(
                &format!("job-{i}"),
                &[],
                &[&write_pat],
                i as u32,
                false,
                false,
            )
        })
        .collect();
    let dag = build_dag(&jobs).unwrap();
    // Must have 100 nodes and at least some edges from the shared writers.
    assert_eq!(dag.nodes.len(), 100);
    assert!(dag.edge_count() > 0);
}

#[test]
fn glob_with_double_star_matches_deeply() {
    // writes=["**/*.rs"] should conflict with reads=["src/**/*.rs"]
    // because the probe heuristic turns both into paths that match.
    let jobs = vec![
        job("writer", &[], &["**/*.rs"], 0, false, false),
        job("reader", &["src/**/*.rs"], &[], 1, false, false),
    ];
    let dag = build_dag(&jobs).unwrap();
    assert_eq!(dag.edge_count(), 1);
    assert!(dag.nodes[0].children.contains(&1));
}
