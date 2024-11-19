mod engine;
mod error;
pub mod runtime;
use error::Result;
use runtime::manager;
pub use truinlag::{Colour, Jpeg};

#[tokio::main]
async fn main() -> Result<()> {
    manager().await.unwrap();
    Ok(())
}
