mod app;
mod args;
mod checkpoint;
mod discord;
mod fetch;
mod models;
mod paths;
mod proxy;
mod report;
mod secure;
mod state;
mod storage;
mod terminal;
mod timeframe;
mod token;

pub async fn entry() -> anyhow::Result<()> {
    app::run().await
}
