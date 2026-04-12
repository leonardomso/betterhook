//! Capability DAG resolver.
//!
//! Turns a list of [`Job`]s with declared `reads`/`writes`/`network`/
//! `concurrent_safe` capabilities into a static dependency graph the
//! scheduler in `runner::executor` walks to decide what can run in
//! parallel and what must serialize.
//!
//! # Edge rule
//!
//! For each pair `(A, B)`, a directed edge `A → B` exists if:
//!
//! 1. `A.writes` could match any file that `B.reads` or `B.writes`
//!    could match (write-read or write-write conflict), OR
//! 2. Both `A` and `B` declare `network = true` and neither is
//!    `concurrent_safe` — network jobs serialize globally so they
//!    don't race on shared remote state.
//!
//! When a conflict exists the edge direction is picked by **priority
//! order**: the job with the lower `priority` value runs first. Ties
//! break on declaration index. This makes the resulting graph a
//! total order over any conflict set and therefore acyclic by
//! construction.
//!
//! # Glob overlap heuristic
//!
//! At DAG build time we don't yet know which files a commit will
//! stage, so glob-set overlap is approximated: for each pattern in
//! one set, we build a "probe filename" by replacing `**` with `a/a`
//! and `*` with `a`, and check if the other globset matches it. The
//! test is symmetric. This is intentionally pessimistic — spurious
//! edges serialize extra jobs but never allow unsafe parallelism.

use globset::GlobSet;
use miette::Diagnostic;
use thiserror::Error;

use crate::config::Job;
use crate::runner::glob_util::build_globset as build_globset_util;

/// Errors raised while building the DAG.
#[derive(Debug, Error, Diagnostic)]
pub enum DagError {
    #[error("invalid capability glob '{pattern}' in job '{job}': {source}")]
    #[diagnostic(code(betterhook::dag::glob))]
    Glob {
        job: String,
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

pub type DagResult<T> = Result<T, DagError>;

/// One node in the DAG — a job plus its resolved parent/child indices.
#[derive(Debug, Clone)]
pub struct DagNode {
    pub index: usize,
    pub job: Job,
    pub parents: Vec<usize>,
    pub children: Vec<usize>,
}

/// Static dependency graph built from a flat job list.
#[derive(Debug, Clone)]
pub struct JobGraph {
    pub nodes: Vec<DagNode>,
}

impl JobGraph {
    /// Indices of nodes that have no parents — the roots the scheduler
    /// spawns first.
    #[must_use]
    pub fn roots(&self) -> Vec<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| if n.parents.is_empty() { Some(i) } else { None })
            .collect()
    }

    /// Number of edges across the whole graph (for `status` / `explain`).
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.nodes.iter().map(|n| n.children.len()).sum()
    }

    /// `(from, to)` pairs — handy for emitting a graphviz digraph in
    /// `betterhook explain`.
    #[must_use]
    pub fn edges(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for node in &self.nodes {
            for child in &node.children {
                out.push((node.index, *child));
            }
        }
        out
    }
}

/// Build a DAG from a list of jobs.
pub fn build_dag(jobs: &[Job]) -> DagResult<JobGraph> {
    let mut nodes: Vec<DagNode> = jobs
        .iter()
        .enumerate()
        .map(|(index, job)| DagNode {
            index,
            job: job.clone(),
            parents: Vec::new(),
            children: Vec::new(),
        })
        .collect();

    // Pre-compile each job's globsets once. Returned vectors are
    // aligned with the `nodes` vector by index.
    let mut writes_sets: Vec<Option<GlobSet>> = Vec::with_capacity(nodes.len());
    let mut reads_or_writes_sets: Vec<Option<GlobSet>> = Vec::with_capacity(nodes.len());
    for node in &nodes {
        writes_sets.push(build_globset(&node.job.name, &node.job.writes)?);
        let mut combined: Vec<String> = node.job.reads.clone();
        combined.extend(node.job.writes.clone());
        reads_or_writes_sets.push(build_globset(&node.job.name, &combined)?);
    }

    for a_idx in 0..nodes.len() {
        for b_idx in (a_idx + 1)..nodes.len() {
            let conflict = pair_conflicts(
                &nodes[a_idx].job,
                &nodes[b_idx].job,
                writes_sets[a_idx].as_ref(),
                writes_sets[b_idx].as_ref(),
                reads_or_writes_sets[a_idx].as_ref(),
                reads_or_writes_sets[b_idx].as_ref(),
            );
            if !conflict {
                continue;
            }
            // Pick direction by priority. Lower priority number
            // (declared earlier in `hook.priority`) runs first.
            let (parent, child) =
                if order_first(&nodes[a_idx].job, a_idx, &nodes[b_idx].job, b_idx) {
                    (a_idx, b_idx)
                } else {
                    (b_idx, a_idx)
                };
            nodes[parent].children.push(child);
            nodes[child].parents.push(parent);
        }
    }

    Ok(JobGraph { nodes })
}

/// Returns `true` when `a` should run before `b`.
fn order_first(a: &Job, a_idx: usize, b: &Job, b_idx: usize) -> bool {
    match a.priority.cmp(&b.priority) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => a_idx < b_idx,
    }
}

fn build_globset(job_name: &str, patterns: &[String]) -> DagResult<Option<GlobSet>> {
    build_globset_util(patterns).map_err(|source| DagError::Glob {
        job: job_name.to_owned(),
        pattern: patterns.first().cloned().unwrap_or_else(|| "<set>".to_owned()),
        source,
    })
}

fn pair_conflicts(
    a: &Job,
    b: &Job,
    a_writes: Option<&GlobSet>,
    b_writes: Option<&GlobSet>,
    a_reads_or_writes: Option<&GlobSet>,
    b_reads_or_writes: Option<&GlobSet>,
) -> bool {
    // Network rule: two non-concurrent-safe network jobs always
    // serialize regardless of their file patterns.
    if a.network && b.network && !(a.concurrent_safe && b.concurrent_safe) {
        return true;
    }

    // Capability glob overlap (either direction).
    let fwd = globsets_overlap(&a.writes, a_writes, &b.reads, &b.writes, b_reads_or_writes);
    let bwd = globsets_overlap(&b.writes, b_writes, &a.reads, &a.writes, a_reads_or_writes);
    fwd || bwd
}

/// True if `writer_patterns` could match any file that the other job's
/// `reader` or `writer` glob sets also match.
fn globsets_overlap(
    writer_patterns: &[String],
    writer_set: Option<&GlobSet>,
    reader_patterns: &[String],
    other_writer_patterns: &[String],
    other_combined_set: Option<&GlobSet>,
) -> bool {
    let Some(w) = writer_set else {
        return false;
    };
    // Probe using the reader/writer patterns as candidate filenames.
    for pat in reader_patterns.iter().chain(other_writer_patterns.iter()) {
        let probe = probe_filename(pat);
        if w.is_match(&probe) {
            return true;
        }
    }
    // And the symmetric check: does the other side's combined globset
    // match any of the writer's patterns?
    let Some(other) = other_combined_set else {
        return false;
    };
    for pat in writer_patterns {
        let probe = probe_filename(pat);
        if other.is_match(&probe) {
            return true;
        }
    }
    false
}

/// Turn a glob pattern into a "representative" filename by replacing
/// wildcards with `a`. `**/*.ts` becomes `a/a/a.ts`.
fn probe_filename(pattern: &str) -> String {
    pattern.replace("**", "a/a").replace('*', "a")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn job(name: &str, reads: &[&str], writes: &[&str], priority: u32) -> Job {
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
            network: false,
            concurrent_safe: false,
            builtin: None,
        }
    }

    #[test]
    fn disjoint_jobs_have_no_edges() {
        let jobs = vec![
            job("rust", &["**/*.rs"], &[], 0),
            job("ts", &["**/*.ts"], &[], 1),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.edge_count(), 0);
        assert_eq!(dag.roots(), vec![0, 1]);
    }

    #[test]
    fn read_after_write_forces_ordering() {
        // `format` writes ts files, `lint` reads ts files — lint must
        // wait for format.
        let jobs = vec![
            job("format", &[], &["**/*.ts"], 0),
            job("lint", &["**/*.ts"], &[], 1),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.edge_count(), 1);
        assert_eq!(dag.nodes[0].children, vec![1]);
        assert_eq!(dag.nodes[1].parents, vec![0]);
    }

    #[test]
    fn write_write_conflict_serializes_by_priority() {
        let jobs = vec![
            job("fmt1", &[], &["**/*.ts"], 1),
            job("fmt2", &[], &["**/*.ts"], 0),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.edge_count(), 1);
        // fmt2 has lower priority -> runs first -> is the parent.
        assert_eq!(dag.nodes[1].children, vec![0]);
        assert_eq!(dag.nodes[0].parents, vec![1]);
    }

    #[test]
    fn priority_tie_breaks_on_declaration_order() {
        let jobs = vec![
            job("first", &[], &["a.txt"], 0),
            job("second", &[], &["a.txt"], 0),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.nodes[0].children, vec![1]);
    }

    #[test]
    fn disjoint_writes_run_in_parallel() {
        let jobs = vec![
            job("a", &[], &["src/a.rs"], 0),
            job("b", &[], &["src/b.rs"], 0),
        ];
        let dag = build_dag(&jobs).unwrap();
        assert_eq!(dag.edge_count(), 0);
    }

    #[test]
    fn network_jobs_serialize() {
        let mut a = job("a", &[], &[], 0);
        a.network = true;
        let mut b = job("b", &[], &[], 1);
        b.network = true;
        let dag = build_dag(&[a, b]).unwrap();
        assert_eq!(dag.edge_count(), 1);
    }

    #[test]
    fn concurrent_safe_network_jobs_do_not_serialize() {
        let mut a = job("a", &[], &[], 0);
        a.network = true;
        a.concurrent_safe = true;
        let mut b = job("b", &[], &[], 1);
        b.network = true;
        b.concurrent_safe = true;
        let dag = build_dag(&[a, b]).unwrap();
        assert_eq!(dag.edge_count(), 0);
    }

    #[test]
    fn invalid_glob_returns_error() {
        let j = job("bad", &[], &["src/[invalid"], 0);
        let err = build_dag(&[j]).unwrap_err();
        assert!(matches!(err, DagError::Glob { .. }));
    }
}
