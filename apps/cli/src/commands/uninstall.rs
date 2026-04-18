use betterhook::install::uninstall;

#[derive(Debug, clap::Args)]
pub struct Args {}

pub async fn run(_args: Args) -> miette::Result<()> {
    let report = uninstall(None).await?;
    println!("removed {} wrappers", report.removed.len());
    for name in &report.removed {
        println!("  {name}");
    }
    if !report.skipped.is_empty() {
        eprintln!("skipped {} hooks:", report.skipped.len());
        for (name, why) in &report.skipped {
            eprintln!("  {name}: {why}");
        }
    }
    Ok(())
}
