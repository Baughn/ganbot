/// Minimal example demonstrating error chain loss with Kameo's `.ask().await?` pattern
use anyhow::{Context as _, Result};
use kameo::prelude::*;

#[derive(Actor)]
struct Worker;

struct DoWork;

impl Message<DoWork> for Worker {
    type Reply = Result<String>;

    async fn handle(
        &mut self,
        _msg: DoWork,
        _ctx: &mut kameo::message::Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // Simulate a deep error chain
        let root_error = std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "checkpoint file 'model.safetensors' not found",
        );

        Err(anyhow::Error::from(root_error))
            .context("while loading model from disk")
            .context("while initializing ComfyUI backend")
    }
}

async fn call_with_question_mark(actor: &ActorRef<Worker>) -> Result<String> {
    // Problem: Using `?` loses the error chain!
    actor.ask(DoWork).await.context("while processing request")
}

async fn call_with_explicit_handling(actor: &ActorRef<Worker>) -> Result<String> {
    // Solution: Extract HandlerError explicitly
    use kameo::error::SendError;

    let ask_result = actor.ask(DoWork).await;
    match ask_result {
        Ok(result) => Ok(result),
        Err(send_err) => match send_err {
            SendError::HandlerError(inner_error) => {
                Err(inner_error).context("while processing request")
            }
            other => {
                Err(anyhow::anyhow!("Actor error: {:?}", other)).context("while processing request")
            }
        },
    }
}

fn print_error_chain(label: &str, error: &anyhow::Error) {
    println!("\n{}", label);
    println!("{}", "=".repeat(60));

    let chain: Vec<String> = error.chain().map(|e| e.to_string()).collect();
    println!("Chain has {} levels:", chain.len());
    for (i, err) in chain.iter().enumerate() {
        println!("  [{}] {}", i, err);
    }

    println!("\nFormatted with {{:#}}:");
    println!("{:#}", error);
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("Kameo Error Chain Loss - Minimal Example");
    println!("{}", "=".repeat(60));

    let actor = Worker::spawn(Worker);

    // Test 1: Using `?` operator (loses chain)
    println!("\n>>> Test 1: Using .await? (PROBLEM)");
    match call_with_question_mark(&actor).await {
        Ok(_) => println!("Unexpected success"),
        Err(e) => print_error_chain("Error with .await?", &e),
    }

    // Test 2: Explicit handling (preserves chain)
    println!("\n\n>>> Test 2: Explicit SendError handling (SOLUTION)");
    match call_with_explicit_handling(&actor).await {
        Ok(_) => println!("Unexpected success"),
        Err(e) => print_error_chain("Error with explicit handling", &e),
    }

    println!("\n{}", "=".repeat(60));
    println!("Notice: Test 1 loses the root cause about the missing file!");
    println!("        Test 2 preserves the full error chain.");

    Ok(())
}
