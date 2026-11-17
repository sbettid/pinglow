fn main() {
    if let Err(error) = pinglow_executor::run_sandbox_launcher() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
