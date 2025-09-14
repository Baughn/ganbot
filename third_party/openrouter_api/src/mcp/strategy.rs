// src/mcp/strategy.rs
use crate::error::Result;
use crate::types::chat::Message;
use crate::mcp::client::ContextStrategy;

/// Simple context truncation strategy
pub struct TruncationStrategy;

impl ContextStrategy for TruncationStrategy {
    fn fit_to_context(&self, mut messages: Vec<Message>, max_tokens: usize) -> Result<Vec<Message>> {
        // Keep system messages and recent messages, removing older ones in the middle
        let mut system_messages = Vec::new();
        let mut user_messages = Vec::new();
        
        // Separate system and user messages
        for msg in messages.drain(..) {
            if msg.role == "system" {
                system_messages.push(msg);
            } else {
                user_messages.push(msg);
            }
        }
        
        // If we have too many messages, remove older ones
        while self.estimate_token_count_split(&system_messages, &user_messages) > max_tokens && !user_messages.is_empty() {
            // Remove the oldest non-system message
            // Skip the most recent user message to maintain conversation flow
            if user_messages.len() > 2 {
                user_messages.remove(1); // Remove the second message (keeping the latest)
            } else {
                // If we only have a few messages left, truncate content instead
                if let Some(msg) = user_messages.get_mut(0) {
                    msg.content = format!("... [truncated] {}", 
                        msg.content.chars().skip(msg.content.len() / 2).collect::<String>());
                }
                break;
            }
        }
        
        // Recombine messages in the correct order
        let mut result = system_messages;
        result.extend(user_messages);
        Ok(result)
    }
    
    fn compress(&self, messages: Vec<Message>) -> Result<Vec<Message>> {
        // Simple compression: combine consecutive messages from the same role
        let mut compressed = Vec::new();
        let mut current_role = String::new();
        let mut current_content = String::new();
        
        for msg in messages {
            if msg.role == current_role && !current_content.is_empty() {
                // Combine with previous message of same role
                current_content.push_str("\n\n");
                current_content.push_str(&msg.content);
            } else {
                // Add the previous combined message if it exists
                if !current_role.is_empty() && !current_content.is_empty() {
                    compressed.push(Message {
                        role: current_role,
                        content: current_content,
                        name: None,
                        tool_calls: None,
                    });
                }
                
                // Start a new combined message
                current_role = msg.role;
                current_content = msg.content;
            }
        }
        
        // Add the last combined message if it exists
        if !current_role.is_empty() && !current_content.is_empty() {
            compressed.push(Message {
                role: current_role,
                content: current_content,
                name: None,
                tool_calls: None,
            });
        }
        
        Ok(compressed)
    }
}

impl TruncationStrategy {
    // Helper method to estimate token count for split messages
    fn estimate_token_count_split(&self, system_msgs: &[Message], user_msgs: &[Message]) -> usize {
        // Basic estimation - in a real implementation, this would be more sophisticated
        let system_tokens = self.estimate_token_count(system_msgs);
        let user_tokens = self.estimate_token_count(user_msgs);
        system_tokens + user_tokens
    }
}

/// Sliding window context strategy
pub struct SlidingWindowStrategy {
    /// Size of the sliding window (in number of messages)
    window_size: usize,
    /// Whether to always include the first message (often a system message)
    always_include_first: bool,
}

impl SlidingWindowStrategy {
    pub fn new(window_size: usize, always_include_first: bool) -> Self {
        Self {
            window_size,
            always_include_first,
        }
    }
}

impl ContextStrategy for SlidingWindowStrategy {
    fn fit_to_context(&self, messages: Vec<Message>, _max_tokens: usize) -> Result<Vec<Message>> {
        if messages.len() <= self.window_size {
            return Ok(messages);
        }
        
        let mut result = Vec::new();
        
        // Always include the first message if flag is set
        if self.always_include_first && !messages.is_empty() {
            result.push(messages[0].clone());
        }
        
        // Add the most recent messages up to window_size
        let start_idx = if self.always_include_first { 1 } else { 0 };
        let window_start = messages.len().saturating_sub(self.window_size - start_idx);
        
        // Add messages from the sliding window
        for idx in window_start..messages.len() {
            result.push(messages[idx].clone());
        }
        
        Ok(result)
    }
    
    fn compress(&self, messages: Vec<Message>) -> Result<Vec<Message>> {
        // For sliding window, we simply ensure we're within the window size
        self.fit_to_context(messages, usize::MAX)
    }
}

/// Summary-based context strategy
pub struct SummaryStrategy {
    /// Function to summarize a set of messages
    summarizer: Box<dyn Fn(Vec<Message>) -> Result<Message> + Send + Sync>,
    /// Number of recent messages to always include without summarization
    recent_count: usize,
}

impl SummaryStrategy {
    pub fn new<F>(summarizer: F, recent_count: usize) -> Self 
    where
        F: Fn(Vec<Message>) -> Result<Message> + Send + Sync + 'static
    {
        Self {
            summarizer: Box::new(summarizer),
            recent_count,
        }
    }
}

impl ContextStrategy for SummaryStrategy {
    fn fit_to_context(&self, messages: Vec<Message>, _max_tokens: usize) -> Result<Vec<Message>> {
        if messages.len() <= self.recent_count + 1 {
            return Ok(messages);
        }
        
        let mut result = Vec::new();
        
        // Always include the first message (system prompt)
        if !messages.is_empty() {
            result.push(messages[0].clone());
        }
        
        // Split into history to summarize and recent messages to keep
        let history = messages[1..messages.len() - self.recent_count].to_vec();
        let recent = messages[messages.len() - self.recent_count..].to_vec();
        
        // Generate a summary of the history
        if !history.is_empty() {
            let summary = (self.summarizer)(history)?;
            result.push(summary);
        }
        
        // Add recent messages
        result.extend(recent);
        
        Ok(result)
    }
    
    fn compress(&self, messages: Vec<Message>) -> Result<Vec<Message>> {
        // For summary strategy, compression is effectively the same as fitting
        self.fit_to_context(messages, usize::MAX)
    }
}

