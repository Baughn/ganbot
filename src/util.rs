//! Misc utility functions

use std::{future::Future, time::Duration};

use anyhow::{Context as _, Result};
use tokio::time::sleep;

/// Retries an arbitrary async call with default backoff.
/// Primarily intended for glitchy Redis connections.
pub async fn retry<F, Fut, T>(mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    for backoff in [0.2, 5.0, 30.0] {
        if let Ok(x) = f().await {
            return Ok(x);
        }
        sleep(Duration::from_secs_f64(backoff)).await;
    }
    f().await.context("after retries")
}
