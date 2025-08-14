/// This module handles the `/ask` command, which allows users to send questions to the bot.
use anyhow::{Result, bail};

pub async fn ask(question: String, user_id: String) -> Result<String> {
    // Validate the question
    if question.trim().is_empty() {
        bail!("Question cannot be empty".to_string());
    }

    // Log the question
    tracing::info!("User {} asked: {}", user_id, question);

    // Here you would typically send the question to a backend service or process it
    // For demonstration, we'll just return a mock response
    let response = format!("You asked: '{}'. This is a mock response.", question);

    Ok(response)
}
