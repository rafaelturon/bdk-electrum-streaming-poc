use anyhow::Result;

/// Internal orchestrator
pub struct Engine {}

impl Engine {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn run(&mut self) -> Result<()> {
        Ok(())
    }
}
