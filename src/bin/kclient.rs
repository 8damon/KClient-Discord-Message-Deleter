#[tokio::main]
async fn main() -> anyhow::Result<()> {
    kcordclient::entry().await
}
