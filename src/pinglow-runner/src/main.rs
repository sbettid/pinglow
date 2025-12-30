use env_logger::Builder;

mod config;
mod executor;
mod queue;
mod runner;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize the logger
    Builder::from_env(env_logger::Env::default().default_filter_or("info,wasmtime_wasi::p1=warn"))
        .init();

    runner::run().await
}
