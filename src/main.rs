mod infra;

use infra::Config;

#[tokio::main]
async fn main() {
    infra::init();
    tracing::info!("starting up");

    let _config = Config::load();
}
