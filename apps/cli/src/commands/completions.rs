use clap_complete::Shell;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Shell to generate completions for.
    #[arg(value_enum)]
    pub shell: Shell,
}

pub fn run(args: &Args) {
    let mut cmd = crate::cli();
    clap_complete::generate(args.shell, &mut cmd, "betterhook", &mut std::io::stdout());
}
