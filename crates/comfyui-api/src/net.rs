// This is an API wrapper; not all code is expected to be used.
#![allow(dead_code)]

use crate::api::Graph;
use bytes::Bytes;
use futures::StreamExt;
use image::RgbImage;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

type WsReceiver = futures::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Errors that can occur when interacting with ComfyUI
#[derive(Error, Debug)]
pub enum ComfyUIError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("WebSocket connection failed: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Image processing error: {0}")]
    Image(#[from] image::ImageError),

    #[error("Workflow validation error: {0}")]
    WorkflowValidation(String),

    #[error("Execution timeout after {timeout}s")]
    ExecutionTimeout { timeout: u64 },

    #[error("Connection failed: {0}")]
    Connection(String),

    #[error("Server error: {status} - {message}")]
    Server { status: u16, message: String },

    #[error("No images found in execution results")]
    NoImagesFound,

    #[error("Invalid workflow: {0}")]
    InvalidWorkflow(String),
}

/// ComfyUI WebSocket message types
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum WsMessageType {
    #[serde(rename = "status")]
    Status { data: StatusData },

    #[serde(rename = "executing")]
    Executing { data: ExecutingData },

    #[serde(rename = "progress")]
    Progress { data: ProgressData },

    #[serde(rename = "execution_cached")]
    ExecutionCached { data: ExecutionCachedData },

    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct StatusData {
    status: QueueStatus,
}

#[derive(Debug, Deserialize)]
struct QueueStatus {
    exec_info: ExecutionInfo,
}

#[derive(Debug, Deserialize)]
struct ExecutionInfo {
    queue_remaining: u32,
}

#[derive(Debug, Deserialize)]
struct ExecutingData {
    node: Option<String>,
    prompt_id: String,
}

#[derive(Debug, Deserialize)]
struct ProgressData {
    value: u32,
    max: u32,
    prompt_id: String,
    node: String,
}

#[derive(Debug, Deserialize)]
struct ExecutionCachedData {
    nodes: Vec<String>,
    prompt_id: String,
}

/// Request structure for prompt submission
#[derive(Debug, Serialize)]
struct PromptRequest {
    prompt: Value,
    client_id: String,
    prompt_id: String,
}

/// Response from prompt submission
#[derive(Debug, Deserialize)]
struct PromptResponse {
    prompt_id: String,
    number: u32,
    node_errors: HashMap<String, Value>,
}

/// History response structure
#[derive(Debug, Deserialize)]
struct HistoryResponse {
    prompt: Vec<Value>,
    outputs: HashMap<String, NodeOutput>,
}

#[derive(Debug, Deserialize)]
struct NodeOutput {
    images: Option<Vec<ImageInfo>>,
}

#[derive(Debug, Deserialize)]
struct ImageInfo {
    filename: String,
    subfolder: String,
    #[serde(rename = "type")]
    image_type: String,
}

/// Configuration for ComfyUI client
#[derive(Debug, Clone)]
pub struct ComfyUIConfig {
    pub server_address: String,
    pub connection_timeout: Duration,
    pub execution_timeout: Duration,
    pub retry_attempts: u32,
    pub retry_delay: Duration,
}

impl Default for ComfyUIConfig {
    fn default() -> Self {
        Self {
            server_address: "localhost:8188".to_string(),
            connection_timeout: Duration::from_secs(30),
            execution_timeout: Duration::from_secs(300), // 5 minutes
            retry_attempts: 3,
            retry_delay: Duration::from_secs(1),
        }
    }
}

/// Builder for ComfyUI client configuration
pub struct ComfyUIConfigBuilder {
    config: ComfyUIConfig,
}

impl ComfyUIConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: ComfyUIConfig::default(),
        }
    }

    pub fn server_address(mut self, address: impl Into<String>) -> Self {
        self.config.server_address = address.into();
        self
    }

    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.config.connection_timeout = timeout;
        self
    }

    pub fn execution_timeout(mut self, timeout: Duration) -> Self {
        self.config.execution_timeout = timeout;
        self
    }

    pub fn retry_attempts(mut self, attempts: u32) -> Self {
        self.config.retry_attempts = attempts;
        self
    }

    pub fn retry_delay(mut self, delay: Duration) -> Self {
        self.config.retry_delay = delay;
        self
    }

    pub fn build(self) -> ComfyUIConfig {
        self.config
    }
}

impl Default for ComfyUIConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Progress callback for workflow execution
pub type ProgressCallback = Box<dyn Fn(f32, Option<&str>) + Send + Sync>;

/// Main ComfyUI client for interacting with the API
pub struct ComfyUIClient {
    config: ComfyUIConfig,
    client_id: String,
    http_client: Client,
}

impl ComfyUIClient {
    /// Create a new ComfyUI client with default configuration
    pub fn new() -> Self {
        Self::with_config(ComfyUIConfig::default())
    }

    /// Create a new ComfyUI client with custom configuration
    pub fn with_config(config: ComfyUIConfig) -> Self {
        let http_client = Client::builder()
            .timeout(config.connection_timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            client_id: Uuid::new_v4().to_string(),
            http_client,
        }
    }

    /// Get the HTTP base URL for the ComfyUI server
    fn base_url(&self) -> String {
        format!("http://{}/api", self.config.server_address)
    }

    /// Get the WebSocket URL for the ComfyUI server
    fn ws_url(&self) -> String {
        format!(
            "ws://{}/ws?clientId={}",
            self.config.server_address, self.client_id
        )
    }

    /// Submit a workflow to the ComfyUI queue
    async fn queue_prompt(&self, workflow: Value) -> Result<PromptResponse, ComfyUIError> {
        let prompt_id = Uuid::new_v4().to_string();
        let request = PromptRequest {
            prompt: workflow,
            client_id: self.client_id.clone(),
            prompt_id: prompt_id.clone(),
        };

        debug!("Submitting workflow with prompt_id: {}", prompt_id);

        let response = self
            .http_client
            .post(format!("{}/prompt", self.base_url()))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ComfyUIError::Server { status, message });
        }

        let mut result: PromptResponse = response.json().await?;
        result.prompt_id = prompt_id; // Ensure we have the correct prompt_id

        if !result.node_errors.is_empty() {
            let errors = serde_json::to_string_pretty(&result.node_errors)?;
            return Err(ComfyUIError::WorkflowValidation(errors));
        }

        Ok(result)
    }

    /// Get execution history for a specific prompt
    async fn get_history(&self, prompt_id: &str) -> Result<HistoryResponse, ComfyUIError> {
        debug!("Fetching history for prompt_id: {}", prompt_id);

        let response = self
            .http_client
            .get(format!("{}/history/{}", self.base_url(), prompt_id))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ComfyUIError::Server { status, message });
        }

        let history_map: HashMap<String, HistoryResponse> = response.json().await?;

        history_map
            .into_iter()
            .next()
            .map(|(_, history)| history)
            .ok_or_else(|| ComfyUIError::InvalidWorkflow("No history found for prompt".to_string()))
    }

    /// Download an image from ComfyUI
    async fn get_image(
        &self,
        filename: &str,
        subfolder: &str,
        image_type: &str,
    ) -> Result<Bytes, ComfyUIError> {
        debug!(
            "Downloading image: {} from {}/{}",
            filename, image_type, subfolder
        );

        let mut url = format!(
            "{}/view?filename={}&type={}",
            self.base_url(),
            filename,
            image_type
        );
        if !subfolder.is_empty() {
            url.push_str(&format!("&subfolder={}", subfolder));
        }

        let response = self.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ComfyUIError::Server { status, message });
        }

        Ok(response.bytes().await?)
    }

    /// Clear the ComfyUI queue
    pub async fn clear_queue(&self) -> Result<(), ComfyUIError> {
        debug!("Clearing ComfyUI queue");

        let data = serde_json::json!({"clear": true});

        let response = self
            .http_client
            .post(format!("{}/queue", self.base_url()))
            .json(&data)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ComfyUIError::Server { status, message });
        }

        info!("ComfyUI queue cleared successfully");
        Ok(())
    }

    /// Interrupt any ongoing ComfyUI generation
    pub async fn interrupt(&self) -> Result<(), ComfyUIError> {
        debug!("Interrupting ComfyUI generation");

        let response = self
            .http_client
            .post(format!("{}/interrupt", self.base_url()))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(ComfyUIError::Server { status, message });
        }

        info!("ComfyUI generation interrupted successfully");
        Ok(())
    }

    /// Monitor workflow execution via WebSocket with an existing receiver
    async fn monitor_execution_with_receiver(
        &self,
        prompt_id: &str,
        ws_receiver: &mut WsReceiver,
        progress_callback: Option<&ProgressCallback>,
    ) -> Result<(), ComfyUIError> {
        let execution_future = async {
            while let Some(message) = ws_receiver.next().await {
                match message? {
                    WsMessage::Text(text) => {
                        if let Ok(msg) = serde_json::from_str::<WsMessageType>(&text) {
                            match msg {
                                WsMessageType::Executing { data } => {
                                    if data.prompt_id == prompt_id {
                                        if data.node.is_none() {
                                            info!(
                                                "Workflow execution completed for prompt_id: {}",
                                                prompt_id
                                            );
                                            if let Some(callback) = progress_callback {
                                                callback(1.0, Some("completed"));
                                            }
                                            return Ok::<(), ComfyUIError>(());
                                        } else if let Some(node) = &data.node {
                                            debug!("Executing node: {}", node);
                                        }
                                    }
                                }
                                WsMessageType::Progress { data } => {
                                    if data.prompt_id == prompt_id {
                                        let progress = data.value as f32 / data.max as f32;
                                        debug!(
                                            "Progress: {:.1}% (node: {})",
                                            progress * 100.0,
                                            data.node
                                        );
                                        if let Some(callback) = progress_callback {
                                            callback(progress, Some(&data.node));
                                        }
                                    }
                                }
                                WsMessageType::ExecutionCached { data } => {
                                    if data.prompt_id == prompt_id {
                                        debug!(
                                            "Cached nodes for prompt_id {}: {:?}",
                                            prompt_id, data.nodes
                                        );
                                        // For fully cached executions, this might be the completion signal
                                        if let Some(callback) = progress_callback {
                                            callback(0.9, Some("cached"));
                                        }
                                    }
                                }
                                WsMessageType::Status { data } => {
                                    debug!(
                                        "Queue remaining: {}",
                                        data.status.exec_info.queue_remaining
                                    );
                                    if let Some(callback) = progress_callback {
                                        let message = format!(
                                            "queue_remaining:{}",
                                            data.status.exec_info.queue_remaining
                                        );
                                        callback(0.0, Some(&message));
                                    }
                                }
                                WsMessageType::Other => {
                                    // Ignore unknown message types
                                }
                            }
                        }
                    }
                    WsMessage::Binary(_) => {
                        // Binary data (preview images) - we'll ignore these for now
                        debug!("Received binary preview data");
                    }
                    WsMessage::Close(_) => {
                        warn!("WebSocket closed unexpectedly");
                        break;
                    }
                    _ => {}
                }
            }

            Err(ComfyUIError::Connection(
                "WebSocket connection lost before completion".to_string(),
            ))
        };

        // Add timeout to execution monitoring
        match timeout(self.config.execution_timeout, execution_future).await {
            Ok(result) => result,
            Err(_) => Err(ComfyUIError::ExecutionTimeout {
                timeout: self.config.execution_timeout.as_secs(),
            }),
        }
    }

    /// Monitor workflow execution via WebSocket (legacy method)
    async fn monitor_execution(
        &self,
        prompt_id: &str,
        progress_callback: Option<&ProgressCallback>,
    ) -> Result<(), ComfyUIError> {
        debug!("Connecting to WebSocket for monitoring: {}", self.ws_url());

        let (ws_stream, _) = connect_async(&self.ws_url()).await?;
        let (_, mut ws_receiver) = ws_stream.split();

        self.monitor_execution_with_receiver(prompt_id, &mut ws_receiver, progress_callback)
            .await
    }

    /// Execute a workflow and return generated images
    pub async fn execute_workflow(
        &self,
        workflow: Value,
        progress_callback: Option<ProgressCallback>,
    ) -> Result<Vec<RgbImage>, ComfyUIError> {
        info!("Starting workflow execution");

        // First establish WebSocket connection to avoid race condition
        debug!("Connecting to WebSocket for monitoring: {}", self.ws_url());
        let (ws_stream, _) = connect_async(&self.ws_url()).await?;
        let (_, mut ws_receiver) = ws_stream.split();

        // Submit workflow to queue
        let prompt_response = self.queue_prompt(workflow).await?;
        let prompt_id = &prompt_response.prompt_id;

        info!(
            "Workflow queued with prompt_id: {} (queue position: {})",
            prompt_id, prompt_response.number
        );

        if let Some(callback) = progress_callback.as_ref() {
            let message = format!("queued:{}", prompt_response.number);
            callback(0.0, Some(&message));
        }

        // Check if execution completed immediately (cached)
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        if let Ok(_history) = self.get_history(prompt_id).await {
            info!(
                "Workflow execution completed immediately (cached) for prompt_id: {}",
                prompt_id
            );
            if let Some(callback) = progress_callback.as_ref() {
                callback(1.0, Some("completed"));
            }
        } else {
            // Monitor execution via WebSocket
            self.monitor_execution_with_receiver(
                prompt_id,
                &mut ws_receiver,
                progress_callback.as_ref(),
            )
            .await?;
        }

        // Get execution results
        let history = self.get_history(prompt_id).await?;

        // Download generated images
        let mut images = Vec::new();

        for (node_id, output) in &history.outputs {
            if let Some(image_list) = &output.images {
                for image_info in image_list {
                    debug!(
                        "Downloading image from node {}: {}",
                        node_id, image_info.filename
                    );

                    let image_data = self
                        .get_image(
                            &image_info.filename,
                            &image_info.subfolder,
                            &image_info.image_type,
                        )
                        .await?;

                    // Convert bytes to RgbImage
                    let img = image::load_from_memory(&image_data)?;
                    let rgb_img = img.to_rgb8();

                    images.push(rgb_img);
                }
            }
        }

        if images.is_empty() {
            return Err(ComfyUIError::NoImagesFound);
        }

        info!("Successfully generated {} image(s)", images.len());
        Ok(images)
    }

    /// Execute a workflow from a Graph builder
    pub async fn execute_graph(
        &self,
        graph: Graph,
        progress_callback: Option<ProgressCallback>,
    ) -> Result<Vec<RgbImage>, ComfyUIError> {
        let workflow = graph.build();
        self.execute_workflow(workflow, progress_callback).await
    }

    /// Simple text-to-image generation using default settings
    pub async fn text_to_image(
        &self,
        prompt: &str,
        negative_prompt: Option<&str>,
        model_name: &str,
        width: u32,
        height: u32,
    ) -> Result<RgbImage, ComfyUIError> {
        use crate::api::{Graph, KSamplerParams};

        let mut graph = Graph::new();

        // Load model
        let (model, clip, vae) = graph.checkpoint_loader(model_name);

        // Encode prompts
        let positive = graph.clip_text_encode(&clip, prompt);
        let negative = graph.clip_text_encode(&clip, negative_prompt.unwrap_or(""));

        // Create latent image
        let latent = graph.empty_latent_image(width, height, 1);

        // Sample
        let params = KSamplerParams::default();
        let samples = graph.ksampler(&model, &positive, &negative, &latent, params);

        // Decode and save
        let images = graph.vae_decode(&vae, &samples);
        graph.save_images(&images, "ganbot");

        let mut results = self.execute_graph(graph, None).await?;

        if results.is_empty() {
            return Err(ComfyUIError::NoImagesFound);
        }

        Ok(results.remove(0))
    }
}

impl Default for ComfyUIClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function to create a client with custom configuration
pub fn create_client() -> ComfyUIConfigBuilder {
    ComfyUIConfigBuilder::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config = ComfyUIConfigBuilder::new()
            .server_address("192.168.1.100:8188")
            .execution_timeout(Duration::from_secs(600))
            .retry_attempts(5)
            .build();

        assert_eq!(config.server_address, "192.168.1.100:8188");
        assert_eq!(config.execution_timeout, Duration::from_secs(600));
        assert_eq!(config.retry_attempts, 5);
    }

    #[test]
    fn test_client_creation() {
        let client = ComfyUIClient::new();
        assert!(!client.client_id.is_empty());
        assert_eq!(client.config.server_address, "localhost:8188");
    }

    #[test]
    fn test_url_generation() {
        let client = ComfyUIClient::new();
        assert!(client.base_url().starts_with("http://"));
        assert!(client.ws_url().starts_with("ws://"));
        assert!(client.ws_url().contains("clientId="));
    }
}
