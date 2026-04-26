use std::path::PathBuf;

use betterhook::config;
use betterhook::dispatch::find_config;
use betterhook::runner::build_dag;
use miette::{IntoDiagnostic, miette};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Hook name. Required if --job is scoped to a hook with siblings.
    #[arg(long)]
    pub hook: String,
    /// Job name to explain. If omitted, every job in the hook is listed.
    #[arg(long)]
    pub job: Option<String>,
    /// Path to inspect. Defaults to the current directory.
    #[arg(long)]
    pub worktree: Option<PathBuf>,
    /// Output format: `json` (default), `dot` (graphviz digraph), or
    /// `svg` (pipes through `dot -Tsvg`; requires graphviz on PATH).
    #[arg(long, default_value = "json", value_parser = ["json", "dot", "svg"])]
    pub format: String,
}

pub fn run(args: &Args) -> miette::Result<()> {
    let worktree = args.worktree.clone().unwrap_or_else(|| PathBuf::from("."));
    let Some(path) = find_config(&worktree) else {
        return Err(miette!(
            "no betterhook config found in {}",
            worktree.display()
        ));
    };
    let cfg = config::load(&path)?;
    let hook = cfg
        .hooks
        .get(&args.hook)
        .ok_or_else(|| miette!("hook '{}' is not defined in {}", args.hook, path.display()))?;

    match args.format.as_str() {
        "dot" => print_dot(hook),
        "svg" => print_svg(hook),
        _ => print_json(args, hook, &path),
    }
}

fn build_digraph(hook: &betterhook::config::Hook) -> String {
    let Ok(graph) = build_dag(&hook.jobs) else {
        return String::from("digraph betterhook {}\n");
    };
    let mut out = String::from("digraph betterhook {\n");
    out.push_str("  rankdir = LR;\n");
    out.push_str("  node [shape=box, style=rounded, fontname=\"Inter, sans-serif\"];\n");
    for node in &graph.nodes {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("  \"{}\";\n", node.job.name));
    }
    for (a, b) in graph.edges() {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "  \"{}\" -> \"{}\";\n",
                graph.nodes[a].job.name, graph.nodes[b].job.name
            ),
        );
    }
    out.push_str("}\n");
    out
}

fn print_dot(hook: &betterhook::config::Hook) -> miette::Result<()> {
    print!("{}", build_digraph(hook));
    std::io::Write::flush(&mut std::io::stdout()).map_err(|e| miette!("flush failed: {e}"))?;
    Ok(())
}

fn print_svg(hook: &betterhook::config::Hook) -> miette::Result<()> {
    let dot_source = build_digraph(hook);
    let mut child = std::process::Command::new("dot")
        .args(["-Tsvg"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| miette!("failed to run `dot -Tsvg` (is graphviz installed?): {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(dot_source.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|e| miette!("dot process failed: {e}"))?;
    if !output.status.success() {
        return Err(miette!(
            "dot exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::io::Write::write_all(&mut std::io::stdout(), &output.stdout)
        .map_err(|e| miette!("write failed: {e}"))?;
    Ok(())
}

fn print_json(
    args: &Args,
    hook: &betterhook::config::Hook,
    path: &std::path::Path,
) -> miette::Result<()> {
    let mut payload = serde_json::json!({
        "config_path": path.display().to_string(),
        "hook": {
            "name": hook.name,
            "parallel": hook.parallel,
            "fail_fast": hook.fail_fast,
            "stash_untracked": hook.stash_untracked,
        },
    });
    let mut jobs = Vec::new();
    for job in &hook.jobs {
        if let Some(filter) = args.job.as_deref()
            && filter != job.name
        {
            continue;
        }
        jobs.push(serde_json::json!({
            "name": job.name,
            "run": job.run,
            "fix": job.fix,
            "glob": job.glob,
            "exclude": job.exclude,
            "env": job.env,
            "timeout": job.timeout.map(|d| format!("{}ms", d.as_millis())),
            "stage_fixed": job.stage_fixed,
            "interactive": job.interactive,
            "priority": job.priority,
            "isolate": format!("{:?}", job.isolate),
            "reads": job.reads,
            "writes": job.writes,
            "network": job.network,
            "concurrent_safe": job.concurrent_safe,
            "builtin": job.builtin,
        }));
    }
    payload["jobs"] = serde_json::Value::Array(jobs);

    if let Ok(graph) = build_dag(&hook.jobs) {
        let roots: Vec<&str> = graph
            .roots()
            .iter()
            .map(|&i| graph.nodes[i].job.name.as_str())
            .collect();
        let edges: Vec<[&str; 2]> = graph
            .edges()
            .iter()
            .map(|&(a, b)| {
                [
                    graph.nodes[a].job.name.as_str(),
                    graph.nodes[b].job.name.as_str(),
                ]
            })
            .collect();
        payload["dag"] = serde_json::json!({
            "node_count": graph.nodes.len(),
            "edge_count": graph.edge_count(),
            "roots": roots,
            "edges": edges,
            "digraph": build_digraph(hook),
        });
    }

    let pretty = serde_json::to_string_pretty(&payload).into_diagnostic()?;
    println!("{pretty}");
    Ok(())
}
