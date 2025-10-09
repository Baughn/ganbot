//! Misc utility functions

pub mod image_compression;
pub mod kameo;
pub mod token_bucket;

use std::future::Future;

use anyhow::Result;

/// Retries an arbitrary async call with default backoff.
/// Primarily intended for glitchy Redis connections.
pub async fn retry<F, Fut, T>(mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    // Temporarily disabled to see if we've fixed the underlying issue.
    // for backoff in [0.2, 5.0, 30.0] {
    //     if let Ok(x) = f().await {
    //         return Ok(x);
    //     }
    //     sleep(Duration::from_secs_f64(backoff)).await;
    // }
    // f().await.context("after retries")
    f().await
}
