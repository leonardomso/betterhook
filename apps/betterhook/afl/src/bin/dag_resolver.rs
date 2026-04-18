//! afl.rs harness for the capability DAG resolver.

fn main() {
    afl::fuzz!(|data: &[u8]| {
        betterhook::fuzz_harnesses::run_dag_resolver(data);
    });
}
