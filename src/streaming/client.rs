use anyhow::Result;
use bdk_wallet::Update;

/// Public streaming sync adapter (the only API main.rs should use)
pub struct StreamingSync {
    // engine: Engine (will be added)
}

impl StreamingSync {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn start(&mut self) -> Result<()> {
        // start background tasks
        Ok(())
    }

    pub async fn next_update(&mut self) -> Option<Update> {
        // receive from channel
        None
    }
}
