
## Project Directory Structure

```
openrouter_api/
├── src/
│   ├── lib.rs                 # Library root, public exports, and high-level documentation
│   ├── client.rs              # OpenRouter client implementation and type‑state builder
│   ├── config.rs              # (Future) Dedicated client configuration module (currently merged into client.rs)
│   ├── models/                # Data models and types for API endpoints
│   │   ├── mod.rs             # Exports for models
│   │   ├── chat.rs            # Chat completion types
│   │   ├── completion.rs      # Text completion types
│   │   ├── common.rs          # Shared types used across endpoints
│   │   ├── error.rs           # API error types and conversions
│   │   ├── parameters.rs      # LLM parameter types (stub for now)
│   │   └── provider_preferences.rs  # Provider preferences & routing settings
│   ├── api/                   # API endpoint implementations
│   │   ├── mod.rs             # Exports for API implementations
│   │   ├── chat.rs            # Chat completion endpoint implementation (includes streaming support)
│   │   ├── completion.rs      # Text completion endpoint implementation
│   │   ├── generation.rs      # Generation metadata endpoint implementation
│   │   ├── models.rs          # Models listing endpoint implementation
│   │   ├── credits.rs         # Credits information endpoint implementation
│   │   └── web_search.rs      # Web search endpoint implementation (new)
│   ├── types/                 # Shared type definitions (and stubs)
│   │   ├── mod.rs             # Exports for shared types
│   │   ├── provider.rs        # Provider-related types (stub)
│   │   ├── routing.rs         # Routing preference types (stub)
│   │   ├── transform.rs       # Message transform types (stub)
│   │   ├── web_search.rs      # Web search request and response types (new)
│   │   └── completion.rs      # Text completion request/response types
│   └── utils/                 # Utility functions
│       ├── mod.rs             # Exports for utilities
│       ├── http.rs            # HTTP-related utility functions
│       ├── auth.rs            # Authentication utilities (if needed)
│       └── validation.rs      # Input validation utilities
├── tests/                     # Integration tests
│   ├── common/               # Shared test utilities (for mocking, etc.)
│   │   ├── mod.rs             # Exports for test utilities
│   │   └── mock_server.rs     # Mock server for testing (to be added later)
│   ├── chat_tests.rs         # Chat completion endpoint tests (including streaming scenarios)
│   ├── completion_tests.rs   # Text completion endpoint tests
│   └── integration_tests.rs  # Full integration tests
├── examples/                  # Usage examples and sample applications
│   ├── chat.rs              # Simple chat completion example
│   ├── completion.rs        # Text completion example
│   ├── streaming.rs         # Streaming chat completion example
│   └── minimal_chat.rs      # Minimal chat example (new)
├── Cargo.toml                # Package manifest and dependencies
├── README.md                 # High-level documentation and usage instructions
├── CHANGELOG.md              # Version history and release notes
└── LICENSE                   # License information (MIT or Apache-2.0)
```

---

## Key Design Considerations

1. **Modular Organization:**
   - **Models:** Contains data structures and types for API request/response schemas. The `ProviderPreferences` and `WebSearch` modules have been added to provide enhanced type‑safety for routing options, and we now also include text completion types.
   - **API Modules:** Each endpoint (chat, completions, web_search, etc.) is implemented in its own module. The new text completion endpoint is located in `api/completion.rs`.
   - **Types:** Shared types are defined separately to reduce duplication, with new types for CompletionRequest and CompletionResponse housed in `types/completion.rs`.
   - **Utils:** Helper functions (HTTP, authentication, validation) are isolated.

2. **Clear Separation of Concerns:**
   The library cleanly separates endpoint implementations, data models, and client configuration to improve maintainability and testability.

3. **Testing Structure:**
   Integration tests are located in the `/tests` directory to ensure end‑to‑end behavior. Future improvements may include advanced mocks within `tests/common`.

4. **Documentation:**
   Usage examples are provided in the `/examples` directory, and inline Rust documentation (rustdoc) is maintained throughout. A CHANGELOG and LICENSE file are kept up-to-date.

5. **Robust Error Handling:**
   All error variants—including HTTP errors, configuration issues, and JSON decoding errors—are centralized in the error modules (using the `thiserror` crate), ensuring consistent error propagation.

6. **Structured Outputs Support:**
   - **Overview:** Clients can request responses that adhere to a specified JSON Schema, ensuring type‑safety.
   - **Configuration:** Structured output support is integrated via the client’s type‑state builder and can be enabled on any endpoint (except interactive chat).
   - **Validation:** Responses can be validated against the user‑provided JSON Schema, and detailed errors are returned on validation failure.

7. **Provider Preferences & Routing:**
   A strongly‑typed interface allows configuration of routing preferences (such as provider ordering, fallback behavior, and data collection policies) directly in the request payload.

8. **New Web Search Endpoint:**
   In addition to chat and text completion endpoints, the library now offers a web search endpoint. It accepts a minimal search query (with an optional result count) and returns structured search results.

9. **Streaming Support:**
   Real‑time chat completions are supported via Server‑Sent Events (SSE). The implementation correctly handles streaming by ignoring SSE comment lines and yielding valid JSON data chunks, enabling responsive and dynamic UI updates.

10. **Text Completion Endpoint:**
    The new text completion endpoint allows users to send a simple prompt to generate a text completion.
    - **Request:** At minimum, the `model` and `prompt` fields are required. Additional parameters (such as temperature, top_p, etc.) are supported via a flattened field.
    - **Response:** Returns an object with an optional `id` and a list of `choices` that contain the generated text, the index (if available), and the finish reason.
    - The implementation follows the same error handling and header building patterns as the chat endpoint.

---

## Type‑State Builder Pattern

A key feature of this library is the type‑state builder pattern used to create an `OpenRouterClient`. This pattern enforces that configuration steps (e.g. setting the base URL and API key) are performed at compile time before any API calls are executed.

- **Unconfigured State:**
  The client is initially created in an unconfigured state.

- **NoAuth State:**
  Once the base URL is set (which must include a trailing slash), the client transitions to a state where it is not yet authenticated.

- **Ready State:**
  When the API key (and optionally other settings like timeout and custom headers) are provided, the client transitions to the Ready state, where all HTTP resources are fully built and API methods like `chat_completion`, `completions`, and `web_search` may be invoked.

The state markers (`Unconfigured`, `NoAuth`, and `Ready`) are defined in `src/client.rs` and re‑exported through `src/lib.rs`.

Below is an excerpt of the type‑state builder implementation:

```rust
// src/client.rs (excerpt)

use std::marker::PhantomData;
use std::time::Duration;
use url::Url;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use crate::error::{Error, Result};
use crate::types;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub api_key: Option<String>,
    pub base_url: Url,
    pub http_referer: Option<String>,
    pub site_title: Option<String>,
    pub timeout: Duration,
}

impl ClientConfig {
    pub fn build_headers(&self) -> HeaderMap { /* ... */ }
}

// State markers.
pub struct Unconfigured;
pub struct NoAuth;
pub struct Ready;

/// Main OpenRouter client using type‑state validation.
pub struct OpenRouterClient<State = Unconfigured> {
    pub config: ClientConfig,
    pub http_client: Option<reqwest::Client>,
    pub _state: PhantomData<State>,
}

impl OpenRouterClient<Unconfigured> {
    // new(), with_base_url(), and transitioning methods...
}

impl OpenRouterClient<NoAuth> {
    // with_api_key(), with_timeout(), etc. transitioning to Ready...
}

impl OpenRouterClient<Ready> {
    /// Access the chat API.
    pub fn chat(&self) -> chat::ChatApi {
        chat::ChatApi::new(self.http_client.clone().unwrap(), &self.config)
    }
    /// Access the text completion API.
    pub fn completions(&self) -> crate::api::completion::CompletionApi {
        crate::api::completion::CompletionApi::new(self.http_client.clone().unwrap(), &self.config)
    }
    // Other API modules: web_search, etc.
}
```

---

## Implementation Roadmap

### Phase 1: Core Functionality (In Progress / Completed)
- [x] **Client Framework:**
  - Implement type‑state builder pattern for configuration.
  - Validate required fields at compile time.
- [x] **Chat Completion Endpoint:**
  - Implement `chat_completion` to call the OpenRouter chat completions API.
  - Provide basic error handling and JSON decoding.
- [x] **Core Data Models:**
  - Define types for chat messages, requests, responses, and usage.
  - Implement error types for consistent error handling.

### Phase 2: Additional Endpoints and Advanced Features
- [x] **Streaming Support:**
  - Implement a streaming API for chat completions via Server‑Sent Events (SSE).
- [x] **Text Completion Endpoint:**
  - New endpoint for text completions with a request that accepts `model`, `prompt` and other parameters.
  - Returns an object with an optional `id` and a list of choices (with text, index, and finish_reason).
- [x] **Web Search Endpoint:**
  - Introduce a new endpoint for web search queries with type‑safe request/response models.
- [x] **Tool Calling & Structured Outputs:**
  - Implement support for tool calls with JSON Schema validation, including optional validation and fallback modes.
- [x] **Provider Preferences & Model Routing:**
  - Enable configuration for provider preferences, fallbacks, and routing options via the strongly‑typed ProviderPreferences module.
- [ ] **Models Listing and Credits:**
  - Implement endpoints to list models and fetch credit details.

### Phase 3: Robust Testing & Documentation
- [ ] **Test Coverage:**
  - Expand integration and unit tests, including tests for the text completion endpoint.
- [ ] **Documentation Improvements:**
  - Enhance inline documentation and API docs using rustdoc.
  - Provide additional usage examples in the `/examples` directory.
- [ ] **Continuous Integration (CI):**
  - Set up CI pipelines for automated builds and testing.

---

## Summary

The OpenRouter API Client Library is a robust, modular, and type‑safe interface for interacting with the OpenRouter API. It now includes not only streaming chat completions, web search, and tool calling but also a new text completion endpoint that supports a range of configurable generation parameters. As the project evolves, further endpoints and enhanced testing/documentation will continue to improve its capabilities.
