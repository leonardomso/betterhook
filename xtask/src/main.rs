fn main() {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("bench") => println!("xtask bench — placeholder (phase 18)"),
        Some("stress") => println!("xtask stress — placeholder (phase 13)"),
        Some("compat") => println!("xtask compat — placeholder (nightly lefthook-compat suite)"),
        _ => {
            eprintln!("usage: xtask <bench|stress|compat>");
            std::process::exit(64);
        }
    }
}
