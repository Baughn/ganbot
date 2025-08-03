# ComfyUI API Reference Guide

Last updated: 2025-08-07

## Overview

ComfyUI is a powerful node-based workflow system for AI image generation that provides both REST API endpoints and WebSocket communication for real-time updates. This guide covers the complete API structure for integrating ComfyUI into applications, particularly useful for bots that need to generate images programmatically.

## Table of Contents

1. [Core Architecture](#core-architecture)
2. [REST API Endpoints](#rest-api-endpoints)
3. [WebSocket Protocol](#websocket-protocol)
4. [Workflow Structure](#workflow-structure)
5. [Queue Management](#queue-management)
6. [Image Upload/Download](#image-uploaddownload)
7. [Node Types and Parameters](#node-types-and-parameters)
8. [Authentication](#authentication)
9. [Error Handling](#error-handling)
10. [Usage Examples](#usage-examples)
11. [Common Patterns](#common-patterns)
12. [Troubleshooting](#troubleshooting)

## Core Architecture

ComfyUI uses a hybrid communication model:

- **REST API**: For workflow submission, image retrieval, and system queries
- **WebSocket**: For real-time execution updates and progress monitoring
- **Node-based workflows**: JSON structures defining AI generation pipelines

### Key Components

- **Server Address**: Typically `127.0.0.1:8188` for local instances
- **Client ID**: Unique identifier for each client connection (UUID recommended)
- **Prompt ID**: Unique identifier for each workflow execution request

## REST API Endpoints

### Core Endpoints

#### 1. Root Endpoint

```http
GET /
```

- **Purpose**: Serves the main ComfyUI web interface
- **Response**: HTML content of the main interface

#### 2. WebSocket Endpoint

```http
GET /ws?clientId={client_id}
```

- **Purpose**: Establishes WebSocket connection for real-time updates
- **Parameters**:
  - `clientId`: Unique client identifier (UUID)
- **Protocol**: WebSocket upgrade

#### 3. Prompt Submission

```http
POST /prompt
```

- **Purpose**: Submit workflow for execution
- **Headers**: `Content-Type: application/json`
- **Body**:

```json
{
  "prompt": {workflow_json},
  "client_id": "uuid-string",
  "prompt_id": "uuid-string"
}
```

- **Response**: Execution details and prompt ID

#### 4. Queue Management

```http
POST /queue
```

- **Purpose**: Manage execution queue (get status, clear, etc.)
- **Actions**: View queue, clear pending items

#### 5. History Retrieval

```http
GET /history/{prompt_id}
```

- **Purpose**: Get execution results for completed workflows
- **Response**: Generation parameters and output filenames

#### 6. System Information

```http
GET /system_stats
```

- **Purpose**: Retrieve system status and performance metrics
- **Response**: Memory usage, GPU stats, queue status

### Model and Asset Endpoints

#### 7. Model Listing

```http
GET /models
```

- **Purpose**: List available model types and specific models
- **Response**: Categorized model inventory

#### 8. Embeddings

```http
GET /embeddings
```

- **Purpose**: List available textual inversions/embeddings
- **Response**: Available embedding files

#### 9. Extensions

```http
GET /extensions
```

- **Purpose**: Retrieve available JavaScript extensions
- **Response**: Core and custom extension list

### Image Handling Endpoints

#### 10. Image Upload

```http
POST /upload/image
```

- **Purpose**: Upload images for use in workflows
- **Content-Type**: `multipart/form-data`
- **Fields**:
  - `image`: Image file
  - `subfolder`: Optional subfolder
  - `overwrite`: Boolean for file replacement

#### 11. Mask Upload

```http
POST /upload/mask
```

- **Purpose**: Upload mask images for inpainting workflows
- **Content-Type**: `multipart/form-data`

#### 12. Image Viewing

```http
GET /view?filename={name}&subfolder={folder}&type={type}
```

- **Purpose**: Retrieve generated images
- **Parameters**:
  - `filename`: Image filename from workflow output
  - `subfolder`: Subfolder location (optional)
  - `type`: Image type (`output`, `temp`, `input`)

### Control Endpoints

#### 13. Interrupt Execution

```http
POST /interrupt
```

- **Purpose**: Stop current workflow execution
- **Use case**: Cancel long-running generations

#### 14. Features

```http
GET /features
```

- **Purpose**: Get server feature flags and capabilities
- **Response**: Available server features

## WebSocket Protocol

### Connection Setup

```python
import websocket
import uuid

server_address = "127.0.0.1:8188"
client_id = str(uuid.uuid4())
ws = websocket.WebSocket()
ws.connect(f"ws://{server_address}/ws?clientId={client_id}")
```

### Message Types

#### 1. Status Messages

```json
{
  "type": "status",
  "data": {
    "status": {
      "exec_info": {
        "queue_remaining": 0
      }
    }
  }
}
```

#### 2. Execution Messages

```json
{
  "type": "executing",
  "data": {
    "node": "node_id",
    "prompt_id": "prompt_uuid"
  }
}
```

- When `node` is `null`: Execution completed

#### 3. Progress Messages

```json
{
  "type": "progress",
  "data": {
    "value": 5,
    "max": 20,
    "prompt_id": "prompt_uuid",
    "node": "node_id"
  }
}
```

#### 4. Binary Image Data

- **Type**: Binary frames
- **Context**: Sent when preview nodes are executing
- **Handling**: Detect binary frames during preview node execution

### Real-time Monitoring Pattern

```python
def monitor_execution(ws, prompt_id):
    """Monitor workflow execution via WebSocket"""
    while True:
        out = ws.recv()
        if isinstance(out, str):
            message = json.loads(out)
            
            if message['type'] == 'executing':
                data = message['data']
                if data['node'] is None and data['prompt_id'] == prompt_id:
                    print("Execution completed")
                    break
                else:
                    print(f"Executing node: {data['node']}")
                    
            elif message['type'] == 'progress':
                data = message['data']
                progress = data['value'] / data['max'] * 100
                print(f"Progress: {progress:.1f}%")
                
        else:
            # Binary data (preview images)
            print("Received preview image data")
```

## Workflow Structure

### Basic Workflow Format

ComfyUI workflows are JSON objects where keys are node IDs and values are node configurations:

```json
{
  "loader": {
    "class_type": "CheckpointLoaderSimple",
    "inputs": {
      "ckpt_name": "sd_xl_base_1.0.safetensors"
    }
  },
  "clip_encode": {
    "class_type": "CLIPTextEncode",
    "inputs": {
      "text": "a beautiful landscape",
      "clip": ["loader", 0]
    }
  },
  "sampler": {
    "class_type": "KSampler",
    "inputs": {
      "seed": 12345,
      "steps": 20,
      "cfg": 7.0,
      "sampler_name": "euler",
      "scheduler": "normal",
      "model": ["loader", 0],
      "positive": ["clip_encode", 0],
      "negative": ["clip_encode_neg", 0],
      "latent_image": ["create_latent", 0]
    }
  }
}
```

### Node Connection Format

Connections between nodes use the format `["node_id", output_index]`:

- `["1", 0]`: First output of node with ID "1"
- `["3", 1]`: Second output of node with ID "3"

Node IDs can be any string.

### Exporting API Format

1. Enable Dev Mode in ComfyUI settings
2. Use "Save (API Format)" to export workflows
3. The exported JSON is ready for API submission

## Queue Management

### Submitting to Queue

```python
def queue_prompt(prompt, client_id, prompt_id=None):
    """Submit workflow to execution queue"""
    if prompt_id is None:
        prompt_id = str(uuid.uuid4())
    
    data = {
        "prompt": prompt,
        "client_id": client_id,
        "prompt_id": prompt_id
    }
    
    response = requests.post(
        f"http://{server_address}/prompt",
        json=data
    )
    return response.json(), prompt_id
```

### Queue Status

```python
def get_queue_status():
    """Get current queue status"""
    response = requests.post(f"http://{server_address}/queue")
    return response.json()
```

### Queue Operations

- **Clear Queue**: Remove all pending items
- **Delete Item**: Remove specific queued item
- **Reorder**: Change execution priority

## Image Upload/Download

### Upload Images

```python
def upload_image(image_path, subfolder="", overwrite=False):
    """Upload image to ComfyUI"""
    with open(image_path, 'rb') as f:
        files = {'image': f}
        data = {
            'subfolder': subfolder,
            'overwrite': str(overwrite).lower()
        }
        response = requests.post(
            f"http://{server_address}/upload/image",
            files=files,
            data=data
        )
    return response.json()
```

### Download Images

```python
def get_image(filename, subfolder="", folder_type="output"):
    """Retrieve generated image"""
    params = {
        'filename': filename,
        'subfolder': subfolder,
        'type': folder_type
    }
    response = requests.get(
        f"http://{server_address}/view",
        params=params
    )
    return response.content
```

### Image Storage Locations

- **output/**: Final generated images (`save_image` nodes)
- **temp/**: Preview images (`preview_image` nodes)
- **input/**: Uploaded user images

## Node Types and Parameters

### Common Node Types

#### 1. CheckpointLoaderSimple

```json
{
  "class_type": "CheckpointLoaderSimple",
  "inputs": {
    "ckpt_name": "model_name.safetensors"
  }
}
```

- **Purpose**: Load AI model checkpoint
- **Outputs**: model, clip, vae

#### 2. CLIPTextEncode

```json
{
  "class_type": "CLIPTextEncode",
  "inputs": {
    "text": "prompt text here",
    "clip": ["1", 1]
  }
}
```

- **Purpose**: Encode text prompts
- **Outputs**: conditioning

#### 3. KSampler

```json
{
  "class_type": "KSampler",
  "inputs": {
    "seed": 12345,
    "steps": 20,
    "cfg": 7.0,
    "sampler_name": "euler",
    "scheduler": "normal",
    "denoise": 1.0,
    "model": ["1", 0],
    "positive": ["2", 0],
    "negative": ["3", 0],
    "latent_image": ["4", 0]
  }
}
```

- **Purpose**: Generate images using diffusion sampling
- **Key Parameters**:
  - `seed`: Random seed for reproducible results
  - `steps`: Number of denoising steps
  - `cfg`: Classifier-free guidance scale
  - `sampler_name`: Sampling algorithm
  - `scheduler`: Noise schedule type

#### 4. SaveImage

```json
{
  "class_type": "SaveImage",
  "inputs": {
    "filename_prefix": "ComfyUI",
    "images": ["8", 0]
  }
}
```

- **Purpose**: Save generated images to disk
- **Output Location**: `output/` directory

#### 5. LoadImage

```json
{
  "class_type": "LoadImage",
  "inputs": {
    "image": "input_image.png"
  }
}
```

- **Purpose**: Load uploaded images
- **Outputs**: image, mask

### Parameter Types

- **String**: Text values (`"value"`)
- **Integer**: Whole numbers (`42`)
- **Float**: Decimal numbers (`7.5`)
- **Boolean**: True/false values
- **Array**: Node connections (`["node_id", output_index]`)

## Authentication

### Local Development

- **Default**: No authentication required for local instances
- **Port**: Usually runs on `127.0.0.1:8188`

### Production Deployment

- **Reverse Proxy**: Use nginx/apache for HTTPS and auth
- **API Keys**: Implement custom authentication middleware
- **Network Security**: Restrict access to trusted IPs

### API Nodes Authentication

For external API services (new feature in 2025):

- **Comfy Account**: Required for API nodes
- **Credits System**: Prepaid credits for external model usage
- **Secure Login**: OAuth-style authentication flow

## Error Handling

### Common Error Types

#### 1. Connection Errors

```python
try:
    ws.connect(f"ws://{server_address}/ws?clientId={client_id}")
except ConnectionRefusedError:
    print("ComfyUI server not running")
except Exception as e:
    print(f"Connection failed: {e}")
```

#### 2. Workflow Validation Errors

- **Missing Inputs**: Node has unconnected required inputs
- **Invalid Models**: Referenced model files don't exist
- **Type Mismatches**: Incompatible connection types

#### 3. Execution Errors

- **Out of Memory**: Workflow requires more GPU/RAM
- **Model Loading**: Checkpoint file corrupted or missing
- **CUDA Errors**: GPU driver or compatibility issues

#### 4. HTTP Error Handling

```python
def safe_api_call(url, data=None):
    """Make API call with error handling"""
    try:
        if data:
            response = requests.post(url, json=data, timeout=30)
        else:
            response = requests.get(url, timeout=30)
        
        response.raise_for_status()
        return response.json()
        
    except requests.exceptions.Timeout:
        raise Exception("Request timed out")
    except requests.exceptions.ConnectionError:
        raise Exception("Connection failed")
    except requests.exceptions.HTTPError as e:
        raise Exception(f"HTTP error: {e.response.status_code}")
```

### Error Response Format

```json
{
  "error": {
    "type": "prompt_validation_error",
    "message": "Node 5: Input 'image' is not connected",
    "details": {
      "node_id": "5",
      "node_type": "LoadImage",
      "field": "image"
    }
  }
}
```

## Usage Examples

### Complete Basic Workflow

```python
import json
import uuid
import requests
import websocket
from urllib.parse import urlencode

class ComfyUIClient:
    def __init__(self, server_address="127.0.0.1:8188"):
        self.server_address = server_address
        self.client_id = str(uuid.uuid4())
        self.ws = None
    
    def connect_websocket(self):
        """Establish WebSocket connection"""
        self.ws = websocket.WebSocket()
        self.ws.connect(f"ws://{self.server_address}/ws?clientId={self.client_id}")
    
    def queue_prompt(self, workflow):
        """Submit workflow for execution"""
        prompt_id = str(uuid.uuid4())
        data = {
            "prompt": workflow,
            "client_id": self.client_id,
            "prompt_id": prompt_id
        }
        
        response = requests.post(
            f"http://{self.server_address}/prompt",
            json=data
        )
        return prompt_id, response.json()
    
    def get_images(self, workflow):
        """Execute workflow and retrieve results"""
        if not self.ws:
            self.connect_websocket()
        
        prompt_id, queue_result = self.queue_prompt(workflow)
        
        # Monitor execution
        output_images = {}
        while True:
            out = self.ws.recv()
            if isinstance(out, str):
                message = json.loads(out)
                if message['type'] == 'executing':
                    data = message['data']
                    if data['node'] is None and data['prompt_id'] == prompt_id:
                        break  # Execution complete
        
        # Get history and download images
        history = self.get_history(prompt_id)
        for node_id, node_output in history['outputs'].items():
            if 'images' in node_output:
                images = []
                for image in node_output['images']:
                    image_data = self.get_image(
                        image['filename'],
                        image['subfolder'],
                        image['type']
                    )
                    images.append(image_data)
                output_images[node_id] = images
        
        return output_images
    
    def get_history(self, prompt_id):
        """Get execution history for prompt"""
        response = requests.get(f"http://{self.server_address}/history/{prompt_id}")
        return response.json()[prompt_id]
    
    def get_image(self, filename, subfolder, folder_type):
        """Download image file"""
        params = {
            'filename': filename,
            'subfolder': subfolder,
            'type': folder_type
        }
        response = requests.get(
            f"http://{self.server_address}/view",
            params=params
        )
        return response.content

# Example usage
def create_simple_workflow(prompt_text, model_name):
    """Create a basic text-to-image workflow"""
    workflow = {
        "1": {
            "class_type": "CheckpointLoaderSimple",
            "inputs": {
                "ckpt_name": model_name
            }
        },
        "2": {
            "class_type": "CLIPTextEncode",
            "inputs": {
                "text": prompt_text,
                "clip": ["1", 1]
            }
        },
        "3": {
            "class_type": "CLIPTextEncode",
            "inputs": {
                "text": "bad quality, blurry",
                "clip": ["1", 1]
            }
        },
        "4": {
            "class_type": "EmptyLatentImage",
            "inputs": {
                "width": 512,
                "height": 512,
                "batch_size": 1
            }
        },
        "5": {
            "class_type": "KSampler",
            "inputs": {
                "seed": 42,
                "steps": 20,
                "cfg": 7.0,
                "sampler_name": "euler",
                "scheduler": "normal",
                "denoise": 1.0,
                "model": ["1", 0],
                "positive": ["2", 0],
                "negative": ["3", 0],
                "latent_image": ["4", 0]
            }
        },
        "6": {
            "class_type": "VAEDecode",
            "inputs": {
                "samples": ["5", 0],
                "vae": ["1", 2]
            }
        },
        "7": {
            "class_type": "SaveImage",
            "inputs": {
                "filename_prefix": "ComfyUI_generated",
                "images": ["6", 0]
            }
        }
    }
    return workflow

# Usage example
if __name__ == "__main__":
    client = ComfyUIClient()
    workflow = create_simple_workflow(
        "a beautiful sunset over mountains",
        "sd_xl_base_1.0.safetensors"
    )
    
    images = client.get_images(workflow)
    print(f"Generated {len(images)} image sets")
```

### Bot Integration Example

```python
class ImageBot:
    def __init__(self):
        self.comfy_client = ComfyUIClient()
    
    async def generate_image(self, prompt, user_id):
        """Generate image for bot command"""
        try:
            # Create workflow with user prompt
            workflow = create_simple_workflow(
                prompt,
                "sd_xl_base_1.0.safetensors"
            )
            
            # Generate image
            images = self.comfy_client.get_images(workflow)
            
            if images:
                # Save image with user identifier
                image_data = list(images.values())[0][0]
                filename = f"generated_{user_id}_{int(time.time())}.png"
                
                with open(filename, 'wb') as f:
                    f.write(image_data)
                
                return filename
            else:
                return None
                
        except Exception as e:
            print(f"Image generation failed: {e}")
            return None
```

## Common Patterns

### 1. Workflow Templates

Create reusable workflow templates with parameter substitution:

```python
class WorkflowTemplate:
    def __init__(self, template_path):
        with open(template_path, 'r') as f:
            self.template = json.load(f)
    
    def substitute_params(self, **params):
        """Replace template parameters with actual values"""
        workflow = json.loads(json.dumps(self.template))  # Deep copy
        
        for node_id, node in workflow.items():
            for input_key, input_value in node.get('inputs', {}).items():
                if isinstance(input_value, str) and input_value.startswith('{{'):
                    param_name = input_value.strip('{}')
                    if param_name in params:
                        workflow[node_id]['inputs'][input_key] = params[param_name]
        
        return workflow

# Template example with placeholders
template_workflow = {
    "1": {
        "class_type": "CLIPTextEncode",
        "inputs": {
            "text": "{{prompt}}",  # Placeholder
            "clip": ["0", 1]
        }
    },
    "2": {
        "class_type": "KSampler",
        "inputs": {
            "seed": "{{seed}}",  # Placeholder
            "steps": "{{steps}}",  # Placeholder
            # ... other inputs
        }
    }
}

# Usage
template = WorkflowTemplate("text2img_template.json")
workflow = template.substitute_params(
    prompt="a cat in a hat",
    seed=12345,
    steps=20
)
```

### 2. Batch Processing

```python
async def batch_generate(prompts, max_concurrent=3):
    """Generate multiple images concurrently"""
    semaphore = asyncio.Semaphore(max_concurrent)
    
    async def generate_single(prompt):
        async with semaphore:
            client = ComfyUIClient()
            workflow = create_simple_workflow(prompt, "model.safetensors")
            return await asyncio.to_thread(client.get_images, workflow)
    
    tasks = [generate_single(prompt) for prompt in prompts]
    results = await asyncio.gather(*tasks, return_exceptions=True)
    
    return results
```

### 3. Progress Monitoring

```python
def monitor_with_progress(ws, prompt_id, callback=None):
    """Monitor execution with progress callback"""
    while True:
        out = ws.recv()
        if isinstance(out, str):
            message = json.loads(out)
            
            if message['type'] == 'progress':
                data = message['data']
                progress = data['value'] / data['max']
                
                if callback:
                    callback(progress, data['node'])
            
            elif message['type'] == 'executing':
                data = message['data']
                if data['node'] is None and data['prompt_id'] == prompt_id:
                    if callback:
                        callback(1.0, "completed")
                    break

# Usage with progress bar
def progress_callback(progress, node):
    bar_length = 20
    filled_length = int(bar_length * progress)
    bar = '=' * filled_length + '-' * (bar_length - filled_length)
    print(f'\r[{bar}] {progress:.1%} - {node}', end='')
```

## Troubleshooting

### Common Issues

#### 1. Connection Failed

**Problem**: Cannot connect to ComfyUI server
**Solutions**:

- Verify ComfyUI is running (`python main.py --port 8188`)
- Check firewall settings
- Confirm correct server address and port

#### 2. Model Not Found

**Problem**: Workflow fails with model loading error
**Solutions**:

- Verify model file exists in `models/checkpoints/`
- Check filename exactly matches (case-sensitive)
- Ensure model format is supported (.safetensors, .ckpt)

#### 3. Out of Memory

**Problem**: Generation fails due to insufficient GPU memory
**Solutions**:

- Reduce image dimensions
- Lower batch size
- Enable model offloading in ComfyUI settings
- Use CPU for VAE decode (`--cpu-vae` flag)

#### 4. WebSocket Timeout

**Problem**: WebSocket connection drops during long generations
**Solutions**:

- Increase WebSocket timeout settings
- Implement reconnection logic
- Use heartbeat/ping mechanism

#### 5. Workflow Validation Errors

**Problem**: Workflow rejected due to validation issues
**Solutions**:

- Check all required inputs are connected
- Verify node types exist in ComfyUI installation
- Test workflow in ComfyUI web interface first

### Debug Techniques

#### Enable Verbose Logging

```python
import logging
logging.basicConfig(level=logging.DEBUG)

# For WebSocket debugging
websocket.enableTrace(True)
```

#### Workflow Validation

```python
def validate_workflow(workflow):
    """Basic workflow validation"""
    errors = []
    
    for node_id, node in workflow.items():
        if 'class_type' not in node:
            errors.append(f"Node {node_id}: Missing class_type")
        
        if 'inputs' not in node:
            errors.append(f"Node {node_id}: Missing inputs")
        
        # Check connections
        for input_key, input_value in node.get('inputs', {}).items():
            if isinstance(input_value, list) and len(input_value) == 2:
                target_node = input_value[0]
                if target_node not in workflow:
                    errors.append(f"Node {node_id}: Invalid connection to {target_node}")
    
    return errors
```

#### Performance Monitoring

```python
class PerformanceMonitor:
    def __init__(self):
        self.start_time = None
        self.node_times = {}
    
    def start_generation(self):
        self.start_time = time.time()
    
    def node_started(self, node_id):
        self.node_times[node_id] = time.time()
    
    def node_completed(self, node_id):
        if node_id in self.node_times:
            duration = time.time() - self.node_times[node_id]
            print(f"Node {node_id} took {duration:.2f}s")
    
    def generation_completed(self):
        total_time = time.time() - self.start_time
        print(f"Total generation time: {total_time:.2f}s")
```

## Advanced Features

### Custom Node Support

ComfyUI supports custom nodes for extended functionality:

- Install custom nodes in `custom_nodes/` directory
- Restart ComfyUI to load new nodes
- Custom nodes appear in workflow JSON with their registered class names

### API Nodes (2025 Feature)

New external API integration nodes:

- Access to closed-source models (Flux, GPT, etc.)
- Requires Comfy account and credits
- Seamless integration with local nodes
- Controlled costs through prepaid system

### Model Management

- **Automatic Downloads**: Some nodes can download models automatically
- **Version Control**: Track model versions and updates
- **Storage Optimization**: Link duplicate models to save space

## Best Practices

### 1. Resource Management

- Monitor GPU memory usage
- Implement queue limits to prevent overload
- Use appropriate image dimensions for your hardware

### 2. Error Resilience

- Implement retry logic for transient failures
- Validate workflows before submission
- Handle WebSocket disconnections gracefully

### 3. Security Considerations

- Validate user inputs to prevent injection attacks
- Limit workflow complexity for untrusted users
- Implement rate limiting for API access

### 4. Performance Optimization

- Cache frequently used models in memory
- Use batching for multiple similar generations
- Optimize workflow structure to minimize node switching

### 5. User Experience

- Provide real-time progress feedback
- Implement generation queuing with position updates
- Cache results for identical prompts

This comprehensive guide covers the essential aspects of integrating with ComfyUI's API. The combination of REST endpoints for workflow management and WebSocket for real-time updates provides a powerful foundation for building AI-powered applications and bots.
