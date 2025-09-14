// src/mcp/context.rs
use crate::error::{Error, Result};
use crate::types::chat::{ChatCompletionRequest, Message};
use crate::mcp::client::{ContextProcessor, MCPClient};
use std::sync::Arc;

/// Advanced implementation of context processor using summarization
pub struct SummarizingProcessor {
    /// OpenRouter client for calling summarization model
    client: Arc<crate::client::OpenRouterClient<crate::client::Ready>>,
    /// Model to use for summarization
    summarization_model: String,
    /// Tokio runtime for blocking operations
    runtime: tokio::runtime::Runtime,
}

impl SummarizingProcessor {
    pub fn new(
        client: Arc<crate::client::OpenRouterClient<crate::client::Ready>>,
        summarization_model: impl Into<String>,
    ) -> Result<Self> {
        // Create a runtime for async operations within sync contexts
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::ConfigError(format!("Failed to create Tokio runtime: {}", e)))?;
            
        Ok(Self {
            client,
            summarization_model: summarization_model.into(),
            runtime,
        })
    }
}

impl ContextProcessor for SummarizingProcessor {
    fn compress(&self, messages: Vec<Message>) -> Result<Vec<Message>> {
        // Clone the necessary data to avoid lifetime issues
        let client = self.client.clone();
        let model = self.summarization_model.clone();
        
        // Use the runtime to execute the async block
        self.runtime.block_on(async move {
            // Combine user/assistant exchanges into a single history
            let mut history = String::new();
            
            for msg in &messages {
                let role_prefix = match msg.role.as_str() {
                    "user" => "User",
                    "assistant" => "Assistant",
                    "system" => "System",
                    _ => &msg.role,
                };
                
                history.push_str(&format!("{}: {}\n\n", role_prefix, msg.content));
            }
            
            // Prepare to call the summarization model
            let chat_api = client.chat()?;
            
            // Create a system message asking for summarization
            let system_msg = Message {
                role: "system".to_string(),
                content: "Summarize the following conversation history concisely, capturing all important points and context needed for continuing the conversation.".to_string(),
                name: None,
                tool_calls: None,
            };
            
            // Create a user message with the conversation history
            let user_msg = Message {
                role: "user".to_string(),
                content: history,
                name: None,
                tool_calls: None,
            };
            
            // Call the model to summarize
            let request = ChatCompletionRequest {
                model: model.clone(),
                messages: vec![system_msg, user_msg],
                stream: None,
                response_format: None,
                tools: None,
                provider: None,
                models: None,
                transforms: None,
            };
            
            // Get the summary
            let response = chat_api.chat_completion(request)
                .await
                .map_err(|e| Error::ConfigError(format!("Failed to summarize context: {}", e)))?;
                
            if response.choices.is_empty() {
                return Err(Error::ConfigError("No summary generated".to_string()));
            }
            
            // Create a summarized context
            let summary = Message {
                role: "system".to_string(),
                content: format!("Previous conversation summary: {}", 
                    response.choices[0].message.content),
                name: None,
                tool_calls: None,
            };
            
            // Return the summary as the first message
            let mut result = Vec::new();
            
            // If there was a system message, preserve it first
            if !messages.is_empty() && messages[0].role == "system" {
                result.push(messages[0].clone());
            }
            
            // Add the summary
            result.push(summary);
            
            // Add the most recent messages (one exchange)
            if messages.len() >= 2 {
                result.push(messages[messages.len() - 2].clone());
                result.push(messages[messages.len() - 1].clone());
            }
            
            Ok(result)
        })
    }
    
    fn summarize(&self, messages: Vec<Message>) -> Result<Message> {
        // Clone data for 'static lifetime
        let client = self.client.clone();
        let model = self.summarization_model.clone();
        
        self.runtime.block_on(async move {
            // First compress the messages
            let mut history = String::new();
            
            for msg in &messages {
                let role_prefix = match msg.role.as_str() {
                    "user" => "User",
                    "assistant" => "Assistant",
                    "system" => "System",
                    _ => &msg.role,
                };
                
                history.push_str(&format!("{}: {}\n\n", role_prefix, msg.content));
            }
            
            // Prepare to call the summarization model
            let chat_api = client.chat()?;
            
            // Create a system message asking for summarization
            let system_msg = Message {
                role: "system".to_string(),
                content: "Summarize the following conversation history concisely, capturing all important points and context needed for continuing the conversation.".to_string(),
                name: None,
                tool_calls: None,
            };
            
            // Create a user message with the conversation history
            let user_msg = Message {
                role: "user".to_string(),
                content: history,
                name: None,
                tool_calls: None,
            };
            
            // Call the model to summarize
            let request = ChatCompletionRequest {
                model,
                messages: vec![system_msg, user_msg],
                stream: None,
                response_format: None,
                tools: None,
                provider: None,
                models: None,
                transforms: None,
            };
            
            // Get the summary
            let response = chat_api.chat_completion(request)
                .await
                .map_err(|e| Error::ConfigError(format!("Failed to summarize context: {}", e)))?;
                
            if response.choices.is_empty() {
                return Err(Error::ConfigError("No summary generated".to_string()));
            }
            
            // Create and return the summary message
            Ok(Message {
                role: "system".to_string(),
                content: format!("Previous conversation summary: {}", 
                    response.choices[0].message.content),
                name: None,
                tool_calls: None,
            })
        })
    }
    
    fn extract_key_info(&self, messages: Vec<Message>) -> Result<Vec<String>> {
        // Clone data for 'static lifetime
        let client = self.client.clone();
        let model = self.summarization_model.clone();
        
        self.runtime.block_on(async move {
            // Combine all messages
            let combined = messages.iter()
                .map(|msg| msg.content.clone())
                .collect::<Vec<String>>()
                .join("\n\n");
                
            // Create a prompt to extract key information
            let system_msg = Message {
                role: "system".to_string(),
                content: "Extract the key pieces of information from the following text. Return each key point as a separate line.".to_string(),
                name: None,
                tool_calls: None,
            };
            
            let user_msg = Message {
                role: "user".to_string(),
                content: combined,
                name: None,
                tool_calls: None,
            };
            
            // Call the model
            let chat_api = client.chat()?;
            let request = ChatCompletionRequest {
                model,
                messages: vec![system_msg, user_msg],
                stream: None,
                response_format: None,
                tools: None,
                provider: None,
                models: None,
                transforms: None,
            };
            
            let response = chat_api.chat_completion(request)
                .await
                .map_err(|e| Error::ConfigError(format!("Failed to extract key info: {}", e)))?;
                
            if response.choices.is_empty() {
                return Err(Error::ConfigError("No key information extracted".to_string()));
            }
            
            // Split the response into lines
            let key_points = response.choices[0].message.content
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();
                
            Ok(key_points)
        })
    }
}

/// Factory for creating MCP clients with different strategies
pub struct MCPClientFactory;

impl MCPClientFactory {
    /// Create a client with truncation strategy
    pub fn with_truncation(max_context_size: usize) -> MCPClient {
        MCPClient::new(
            max_context_size,
            Box::new(crate::mcp::strategy::TruncationStrategy)
        )
    }
    
    /// Create a client with sliding window strategy
    pub fn with_sliding_window(
        max_context_size: usize,
        window_size: usize,
        always_include_first: bool
    ) -> MCPClient {
        MCPClient::new(
            max_context_size,
            Box::new(crate::mcp::strategy::SlidingWindowStrategy::new(
                window_size, 
                always_include_first
            ))
        )
    }
    
    /// Create a client with summary-based strategy
    pub fn with_summary<F>(
        max_context_size: usize,
        summarizer: F,
        recent_count: usize
    ) -> MCPClient 
    where
        F: Fn(Vec<Message>) -> Result<Message> + Send + Sync + 'static
    {
        MCPClient::new(
            max_context_size,
            Box::new(crate::mcp::strategy::SummaryStrategy::new(
                summarizer,
                recent_count
            ))
        )
    }
    
    /// Create a client with advanced summarization processor
    pub fn with_advanced_summarization(
        max_context_size: usize,
        client: Arc<crate::client::OpenRouterClient<crate::client::Ready>>,
        summarization_model: impl Into<String>,
        recent_count: usize
    ) -> Result<MCPClient> {
        // Create the processor
        let processor = Arc::new(SummarizingProcessor::new(
            client.clone(),
            summarization_model,
        )?);
        
        // Create a summary strategy that uses the processor
        let processor_clone = Arc::clone(&processor);
        let summarizer = move |messages: Vec<Message>| -> Result<Message> {
            processor_clone.summarize(messages)
        };
        
        // Create the client with the summary strategy and processor
        Ok(MCPClient::new(
            max_context_size,
            Box::new(crate::mcp::strategy::SummaryStrategy::new(
                summarizer,
                recent_count
            ))
        ).with_processor(processor))
    }
}

