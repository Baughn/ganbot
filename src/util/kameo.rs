//! Utilities for working with Kameo actors and error handling.
//!
//! ## Error Chain Preservation
//!
//! When using Kameo's `.ask().await?` pattern with anyhow errors, the error chain
//! is lost during automatic conversion from `SendError<M, anyhow::Error>` to `anyhow::Error`.
//!
//! ### Problem
//!
//! ```ignore
//! // Actor returns: "context A" -> "context B" -> "root cause"
//! let result = actor.ask(msg).await?;  // Chain lost!
//! // Now only have: "SendError description"
//! ```
//!
//! ### Solution
//!
//! Use `extract_ask_result` to preserve the error chain:
//!
//! ```ignore
//! use crate::util::kameo::extract_ask_result;
//!
//! let result = extract_ask_result(actor.ask(msg).await)?;
//! // Chain preserved: "context A" -> "context B" -> "root cause"
//! ```
//!
//! Or with additional context:
//!
//! ```ignore
//! let result = extract_ask_result(actor.ask(msg).await)
//!     .context("while processing message")?;
//! // Chain: "while processing message" -> "context A" -> "context B" -> "root cause"
//! ```

use anyhow::Result;
use kameo::error::SendError;

/// Extract the result from a Kameo ask call, preserving error chains.
///
/// When an actor's message handler returns an error, Kameo wraps it in
/// `SendError::HandlerError(E)`. Using the `?` operator directly causes
/// error chain loss. This function extracts the inner error to preserve
/// the full chain.
///
/// # Example
///
/// ```ignore
/// use crate::util::kameo::extract_ask_result;
///
/// let result = extract_ask_result(my_actor.ask(message).await)
///     .context("while executing action")?;
/// ```
///
/// # Returns
///
/// - `Ok(T)` if the actor call succeeded
/// - `Err` with preserved error chain from handler
/// - `Err` with actor communication error for non-handler failures
pub fn extract_ask_result<T, M>(result: Result<T, SendError<M, anyhow::Error>>) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(SendError::HandlerError(inner_error)) => {
            // Preserve the error chain from the handler
            Err(inner_error)
        }
        Err(other_error) => {
            // Actor communication failures
            Err(anyhow::anyhow!(
                "Actor communication failed: {:?}",
                other_error
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn test_extract_ok() {
        let result: Result<i32, SendError<(), anyhow::Error>> = Ok(42);
        let extracted = extract_ask_result(result).unwrap();
        assert_eq!(extracted, 42);
    }

    #[test]
    fn test_extract_handler_error() {
        let inner = anyhow::anyhow!("root cause")
            .context("level 1")
            .context("level 2");
        let result: Result<i32, SendError<(), anyhow::Error>> = Err(SendError::HandlerError(inner));

        let err = extract_ask_result(result).unwrap_err();

        // Check that the error chain is preserved
        let chain: Vec<String> = err.chain().map(|e| e.to_string()).collect();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0], "level 2");
        assert_eq!(chain[1], "level 1");
        assert_eq!(chain[2], "root cause");
    }

    #[test]
    fn test_extract_actor_stopped() {
        let result: Result<i32, SendError<(), anyhow::Error>> = Err(SendError::ActorStopped);
        let err = extract_ask_result(result).unwrap_err();

        assert!(err.to_string().contains("Actor communication failed"));
    }

    #[test]
    fn test_extract_with_additional_context() {
        let inner = anyhow::anyhow!("root cause").context("handler context");
        let result: Result<i32, SendError<(), anyhow::Error>> = Err(SendError::HandlerError(inner));

        let err = extract_ask_result(result)
            .context("additional context")
            .unwrap_err();

        // Check that both the original chain and new context are preserved
        let chain: Vec<String> = err.chain().map(|e| e.to_string()).collect();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0], "additional context");
        assert_eq!(chain[1], "handler context");
        assert_eq!(chain[2], "root cause");
    }
}
