mod engine;
mod error;
pub mod runtime;
use error::Result;
use runtime::manager;

#[tokio::main]
async fn main() -> Result<()> {
    manager().await.unwrap();
    Ok(())
}
