# InvokeAI API Reference

Last Updated: August 4, 2025

## Overview

InvokeAI is a leading creative engine for Stable Diffusion models that provides both a web-based UI and REST API for AI image generation. The API enables programmatic integration of image generation capabilities into applications, supporting automation and custom workflows.

**Important Note**: The InvokeAI HTTP API is currently marked as "not intended for public consumption" and may have breaking changes without notice. Use with caution in production environments.

## Table of Contents

- [Installation & Setup](#installation--setup)
- [API Access](#api-access)
- [Authentication](#authentication)
- [Core Endpoints](#core-endpoints)
- [Request/Response Formats](#requestresponse-formats)
- [Error Handling](#error-handling)
- [Code Examples](#code-examples)
- [Workflow API](#workflow-api)
- [WebSocket Integration](#websocket-integration)
- [Troubleshooting](#troubleshooting)

## Installation & Setup

### Installation Options

1. **Invoke Launcher (Recommended)**
   - Easiest way to install, update, and run InvokeAI
   - Provides GUI for installation and configuration

2. **Python Package**

   ```bash
   pip install InvokeAI
   ```

3. **Docker**
   - Container-based deployment option
   - Suitable for production environments

### Starting the API Server

Once installed, InvokeAI runs a local API server:

```bash
# Start InvokeAI (includes both web UI and API server)
invokeai-web
```

**Default Configuration:**

- API Server: `http://localhost:9090`
- Web UI: `http://localhost:9090` (same port)
- API Documentation: `http://localhost:9090/docs` (Swagger UI)
- OpenAPI Specification: `http://localhost:9090/openapi.json`

## API Access

### Base URL

```text
http://localhost:9090/api/v1
```

### Protocol Support

- **REST API**: HTTP/HTTPS for standard operations
- **WebSocket**: Real-time communication for status updates and streaming

## Authentication

Currently, InvokeAI appears to use session-based authentication through the web interface. For API access:

1. No explicit API key system documented
2. Local installations typically don't require authentication
3. For production deployments, consider implementing reverse proxy authentication

**Security Note**: Always secure your InvokeAI installation when exposing it beyond localhost.

## Core Endpoints

### 1. Image Generation (Batch Queue)

**Endpoint:** `POST /api/v1/queue/default/enqueue_batch`

Primary endpoint for generating images by adding requests to the processing queue.

**Request Format:**

```json
{
  "batch": {
    "graph": {
      // Complex graph structure with nodes and edges
      // Defines the generation pipeline
    },
    "runs": 1,
    "data": {
      // Input parameters and values
    }
  },
  "prepend": false
}
```

**Key Parameters:**

- `graph`: Complex JSON structure defining the generation pipeline
- `runs`: Number of times to execute the batch
- `data`: Input data and parameters for the generation
- `prepend`: Whether to add to front of queue (priority)

### 2. Queue Status

**Endpoint:** `GET /api/v1/queue/default/status`

Check the status of the processing queue.

**Response:**

```json
{
  "queue_id": "default",
  "item_id": null,
  "batch_id": null,
  "session_id": "session_uuid",
  "in_progress": 0,
  "pending": 0,
  "total": 0
}
```

### 3. Queue History

**Endpoint:** `GET /api/v1/queue/default/history`

Retrieve history of completed generations.

**Response:**

```json
{
  "items": [
    {
      "queue_id": "default",
      "item_id": "item_uuid",
      "batch_id": "batch_uuid",
      "session_id": "session_uuid",
      "status": "completed",
      "created_at": "2025-08-04T10:00:00Z",
      "updated_at": "2025-08-04T10:01:00Z"
    }
  ],
  "page": 0,
  "pages": 1,
  "per_page": 10,
  "total": 1
}
```

## Request/Response Formats

### Generation Graph Structure

The `graph` parameter requires a complex nested structure:

```json
{
  "id": "graph_id",
  "nodes": {
    "node_id_1": {
      "id": "node_id_1",
      "type": "main_model_loader",
      "inputs": {
        "model": {
          "model_name": "stable-diffusion-v1-5",
          "model_type": "main"
        }
      }
    },
    "node_id_2": {
      "id": "node_id_2", 
      "type": "compel",
      "inputs": {
        "prompt": "a beautiful landscape"
      }
    }
    // Additional nodes...
  },
  "edges": [
    {
      "source": {
        "node_id": "node_id_1",
        "field": "model"
      },
      "destination": {
        "node_id": "node_id_3",
        "field": "model"
      }
    }
    // Additional edges...
  ]
}
```

### Common Node Types

- `main_model_loader`: Loads the base model
- `compel`: Text prompt processing
- `noise`: Noise generation
- `denoise_latents`: Core denoising process
- `l2i`: Latent to image conversion
- `image_output`: Final image output

## Error Handling

### Common HTTP Status Codes

- `200 OK`: Successful request
- `400 Bad Request`: Invalid request format or parameters
- `422 Unprocessable Entity`: Validation errors
- `500 Internal Server Error`: Server-side processing errors

### Error Response Format

```json
{
  "error": "Error description",
  "detail": "Detailed error information",
  "validation_errors": [
    {
      "field": "field_name",
      "message": "Validation error message"
    }
  ]
}
```

## Code Examples

### Basic Python Client

```python
import requests
import json

class InvokeAIClient:
    def __init__(self, base_url="http://localhost:9090"):
        self.base_url = base_url
        self.api_url = f"{base_url}/api/v1"
    
    def generate_image(self, prompt, model="stable-diffusion-v1-5"):
        """Generate an image using a simplified approach"""
        
        # This is a simplified example - actual graph structure is more complex
        graph = {
            "id": "simple_generation",
            "nodes": {
                "model_loader": {
                    "id": "model_loader",
                    "type": "main_model_loader",
                    "inputs": {
                        "model": {
                            "model_name": model,
                            "model_type": "main"
                        }
                    }
                },
                "prompt_node": {
                    "id": "prompt_node", 
                    "type": "compel",
                    "inputs": {
                        "prompt": prompt
                    }
                }
                # Additional nodes required...
            },
            "edges": [
                # Define connections between nodes
            ]
        }
        
        payload = {
            "batch": {
                "graph": graph,
                "runs": 1,
                "data": {}
            },
            "prepend": False
        }
        
        response = requests.post(
            f"{self.api_url}/queue/default/enqueue_batch",
            json=payload,
            headers={"Content-Type": "application/json"}
        )
        
        return response.json()
    
    def get_queue_status(self):
        """Get current queue status"""
        response = requests.get(f"{self.api_url}/queue/default/status")
        return response.json()
    
    def get_history(self, page=0, per_page=10):
        """Get generation history"""
        params = {"page": page, "per_page": per_page}
        response = requests.get(
            f"{self.api_url}/queue/default/history",
            params=params
        )
        return response.json()

# Usage example
client = InvokeAIClient()

# Generate an image
result = client.generate_image("a serene mountain landscape at sunset")
print(f"Batch queued: {result}")

# Check queue status
status = client.get_queue_status()
print(f"Queue status: {status}")
```

### Rust Client Example (for Ganbot3)

```rust
use reqwest;
use serde_json::{json, Value};

pub struct InvokeAIClient {
    client: reqwest::Client,
    base_url: String,
}

impl InvokeAIClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.to_string(),
        }
    }
    
    pub async fn generate_image(&self, prompt: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let url = format!("{}/api/v1/queue/default/enqueue_batch", self.base_url);
        
        // Simplified payload - actual implementation requires complex graph
        let payload = json!({
            "batch": {
                "graph": {
                    "id": "simple_generation",
                    "nodes": {
                        // Complex node structure required
                    },
                    "edges": []
                },
                "runs": 1,
                "data": {}
            },
            "prepend": false
        });
        
        let response = self.client
            .post(&url)
            .json(&payload)
            .send()
            .await?;
            
        let result: Value = response.json().await?;
        Ok(result)
    }
    
    pub async fn get_queue_status(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let url = format!("{}/api/v1/queue/default/status", self.base_url);
        let response = self.client.get(&url).send().await?;
        let result: Value = response.json().await?;
        Ok(result)
    }
}
```

## Workflow API

### Current Limitations

The current API requires providing the complete `graph` structure for each request, which can be complex and inflexible.

### Proposed Enhancements

The InvokeAI community has proposed additional endpoints for workflow management:

- **Queue by Workflow ID**: `POST /api/v1/workflows/{workflow_id}/queue`
- **Queue by Workflow JSON**: `POST /api/v1/workflows/queue`

These would allow:

- Reusing saved workflows without regenerating graphs
- Easier management of complex workflows
- Better separation of workflow definition from execution

## WebSocket Integration

InvokeAI uses WebSocket connections for real-time updates:

**WebSocket Endpoint:** `ws://localhost:9090/ws`

### Event Types

- `queue_item_status_changed`: Queue item status updates
- `invocation_started`: Node execution started
- `invocation_complete`: Node execution completed
- `invocation_error`: Node execution failed

### WebSocket Client Example

```javascript
const ws = new WebSocket('ws://localhost:9090/ws');

ws.onmessage = (event) => {
    const data = JSON.parse(event.data);
    
    switch(data.event) {
        case 'invocation_complete':
            console.log('Generation step completed:', data);
            break;
        case 'queue_item_status_changed':
            console.log('Queue status changed:', data);
            break;
        default:
            console.log('Unknown event:', data);
    }
};
```

## Troubleshooting

### Common Issues

1. **Complex Graph Structure**
   - **Problem**: The graph structure is complex and not well documented
   - **Solution**: Use browser dev tools to inspect network requests from the web UI
   - **Tip**: Start with simple workflows and gradually add complexity

2. **API Changes**
   - **Problem**: API may change without notice
   - **Solution**: Pin InvokeAI versions in production
   - **Tip**: Monitor the GitHub repository for API-related changes

3. **Model Loading**
   - **Problem**: Models not found or failing to load
   - **Solution**: Ensure models are properly installed via the web UI first
   - **Tip**: Check model names exactly match installed models

4. **Queue Processing**
   - **Problem**: Items stuck in queue
   - **Solution**: Check InvokeAI logs for processing errors
   - **Tip**: Monitor memory usage - large models require significant RAM

### Debugging Tips

1. **Enable Debug Logging**

   ```bash
   INVOKEAI_LOG_LEVEL=debug invokeai-web
   ```

2. **Inspect Web UI Network Requests**
   - Open browser dev tools
   - Generate an image via web UI
   - Copy the network request as cURL or JSON

3. **Test with Simple Payloads**
   - Start with minimal graph structures
   - Add complexity incrementally

## Additional Resources

- **GitHub Repository**: <https://github.com/invoke-ai/InvokeAI>
- **Official Documentation**: <https://invoke-ai.github.io/InvokeAI/>
- **Discord Community**: <https://discord.gg/ZmtBAhwWhy>
- **Local API Documentation**: <http://localhost:9090/docs> (when running)

## Version Information

- **Current Version**: 6.2.0 (as of July 2025)
- **API Version**: v1
- **Python Support**: 3.10+
- **Platforms**: Windows, macOS, Linux

---

**Note**: This documentation is based on available information as of August 2025. The InvokeAI API is actively developed and may change. Always refer to the official documentation and local Swagger UI for the most current information.
