use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "betterhook",
    version,
    about = "Worktree-native git hooks manager built for the AI agent era"
)]
struct Cli {}

fn main() {
    let _ = Cli::parse();
    println!("betterhook {}", betterhook::VERSION);
}
