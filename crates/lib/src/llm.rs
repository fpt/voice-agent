use anyhow::Result;
use serde::{Deserialize, Serialize};

// ============================================================================
// Core types
// ============================================================================

/// Chat message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Tool calls made by assistant (set by ReAct loop)
    #[serde(skip)]
    pub tool_calls: Option<Vec<ToolCallInfo>>,
    /// Tool call ID this message is responding to (for role=Tool)
    #[serde(skip)]
    pub tool_call_id: Option<String>,
    /// Tool name this message is responding to (for role=Tool)
    #[serde(skip)]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    pub fn user(content: String) -> Self {
        Self {
            role: ChatRole::User,
            content,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn system(content: String) -> Self {
        Self {
            role: ChatRole::System,
            content,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn assistant_tool_calls(calls: Vec<ToolCallInfo>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: String::new(),
            tool_calls: Some(calls),
            tool_call_id: None,
            tool_name: None,
        }
    }

    pub fn tool_result(call_id: String, name: String, content: String) -> Self {
        Self {
            role: ChatRole::Tool,
            content,
            tool_calls: None,
            tool_call_id: Some(call_id),
            tool_name: Some(name),
        }
    }
}

/// Tool definition for LLM
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Tool call info returned by LLM
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// LLM response — either text or tool calls
#[derive(Debug)]
pub enum LlmResponse {
    Text {
        content: String,
        reasoning: Option<String>,
    },
    ToolCalls(Vec<ToolCallInfo>),
}

// ============================================================================
// LlmProvider trait
// ============================================================================

/// Trait for LLM providers
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request
    fn chat(&self, messages: &[ChatMessage]) -> Result<String>;

    /// Send a chat completion request with JSON Schema for structured output
    fn chat_with_schema(
        &self,
        messages: &[ChatMessage],
        _schema: serde_json::Value,
        _schema_name: &str,
    ) -> Result<String> {
        tracing::warn!(
            "chat_with_schema not supported by this provider, falling back to regular chat"
        );
        self.chat(messages)
    }

    /// Send a chat request with tool definitions, returning either text or tool calls
    fn chat_with_tools(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        Err(anyhow::anyhow!("Tool calling not supported by this provider"))
    }

    /// Check if this provider supports structured output
    fn supports_structured_output(&self) -> bool {
        false
    }

    /// Check if this provider supports tool calling
    fn supports_tools(&self) -> bool {
        false
    }
}

// ============================================================================
// OpenAI Provider (cloud API) — Responses API
// ============================================================================

// -- Wire format types for Responses API --

/// Input item for Responses API
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ResponsesInputItem {
    #[serde(rename = "message")]
    Message { role: String, content: String },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

/// Tool definition for Responses API
#[derive(Debug, Serialize)]
struct ResponsesTool {
    #[serde(rename = "type")]
    tool_type: String,
    name: String,
    description: String,
    parameters: serde_json::Value,
    strict: bool,
}

/// Reasoning parameter for OpenAI reasoning models
#[derive(Debug, Serialize)]
struct ReasoningParam {
    effort: String,
    summary: String, // "auto", "concise", or "detailed" — must be set to get reasoning output
}

/// OpenAI Responses API request
#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<ResponsesInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<ResponseTextFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
}

/// Text format specification for structured output
#[derive(Debug, Serialize)]
struct ResponseTextFormat {
    format: ResponseFormatSpec,
}

/// Format specification with JSON Schema
#[derive(Debug, Serialize)]
struct ResponseFormatSpec {
    #[serde(rename = "type")]
    format_type: String, // "json_schema"
    name: String,
    schema: serde_json::Value,
    strict: bool,
}

/// OpenAI Responses API response
#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    status: String,
    output: Vec<ResponseOutput>,
    #[serde(default)]
    incomplete_details: Option<IncompleteDetails>,
}

#[derive(Debug, Deserialize)]
struct IncompleteDetails {
    reason: String,
}

#[derive(Debug, Deserialize)]
struct ResponseOutput {
    #[serde(rename = "type")]
    output_type: String,
    // For "message" type
    #[serde(default)]
    content: Option<Vec<ResponseContent>>,
    #[serde(default)]
    text: Option<String>,
    // For "function_call" type
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    // For "reasoning" type
    #[serde(default)]
    summary: Option<Vec<ReasoningSummary>>,
}

#[derive(Debug, Deserialize)]
struct ReasoningSummary {
    text: String,
}

#[derive(Debug, Deserialize)]
struct ResponseContent {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

pub struct OpenAiProvider {
    api_key: String,
    model: String,
    temperature: Option<f32>,
    max_tokens: u32,
    reasoning_effort: Option<String>,
}

impl OpenAiProvider {
    /// Build TLS connector with custom CA certificates
    fn build_tls_with_custom_ca(cert_file: &str) -> Result<native_tls::TlsConnector> {
        use std::fs::File;
        use std::io::Read;

        // Read certificate file
        let mut file = File::open(cert_file)
            .map_err(|e| anyhow::anyhow!("Failed to open certificate file: {}", e))?;
        let mut cert_data = Vec::new();
        file.read_to_end(&mut cert_data)
            .map_err(|e| anyhow::anyhow!("Failed to read certificate file: {}", e))?;

        // Parse certificate(s) - PEM format can contain multiple certificates
        let mut builder = native_tls::TlsConnector::builder();

        // Try to parse as PEM (most common format)
        let cert_str = String::from_utf8_lossy(&cert_data);
        let mut found_cert = false;

        // Split by PEM boundaries
        for pem_block in cert_str.split("-----END CERTIFICATE-----") {
            if let Some(cert_start) = pem_block.find("-----BEGIN CERTIFICATE-----") {
                let pem_cert = format!("{}-----END CERTIFICATE-----", &pem_block[cert_start..]);

                match native_tls::Certificate::from_pem(pem_cert.as_bytes()) {
                    Ok(cert) => {
                        builder.add_root_certificate(cert);
                        found_cert = true;
                        tracing::debug!("Added certificate from PEM");
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse PEM certificate: {}", e);
                    }
                }
            }
        }

        if !found_cert {
            // Try DER format as fallback
            match native_tls::Certificate::from_der(&cert_data) {
                Ok(cert) => {
                    builder.add_root_certificate(cert);
                    tracing::debug!("Added certificate from DER");
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("No valid certificates found in file: {}", e));
                }
            }
        }

        builder.build()
            .map_err(|e| anyhow::anyhow!("Failed to build TLS connector: {}", e))
    }

    pub fn new(
        api_key: String,
        model: String,
        temperature: Option<f32>,
        max_tokens: u32,
        reasoning_effort: Option<String>,
    ) -> Self {
        tracing::info!("Initializing OpenAI provider (Responses API)");
        tracing::info!("  Model: {}", model);
        tracing::info!("  Reasoning effort: {:?}", reasoning_effort);

        Self {
            api_key,
            model,
            temperature,
            max_tokens,
            reasoning_effort,
        }
    }

    /// Build reasoning param if configured
    fn reasoning_param(&self) -> Option<ReasoningParam> {
        self.reasoning_effort.as_ref().map(|effort| ReasoningParam {
            effort: effort.clone(),
            summary: "auto".to_string(),
        })
    }

    /// Convert ChatMessages to Responses API input items
    fn convert_to_input_items(messages: &[ChatMessage]) -> Vec<ResponsesInputItem> {
        messages
            .iter()
            .flat_map(|msg| {
                // Handle assistant messages with tool calls
                if let Some(ref calls) = msg.tool_calls {
                    return calls
                        .iter()
                        .map(|c| ResponsesInputItem::FunctionCall {
                            call_id: c.id.clone(),
                            name: c.name.clone(),
                            arguments: serde_json::to_string(&c.arguments).unwrap_or_default(),
                        })
                        .collect::<Vec<_>>();
                }

                // Handle tool result messages
                if let Some(ref call_id) = msg.tool_call_id {
                    return vec![ResponsesInputItem::FunctionCallOutput {
                        call_id: call_id.clone(),
                        output: msg.content.clone(),
                    }];
                }

                // Regular message
                let role = match msg.role {
                    ChatRole::System => "system",
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                    ChatRole::Tool => return vec![], // Handled above via tool_call_id
                };

                vec![ResponsesInputItem::Message {
                    role: role.to_string(),
                    content: msg.content.clone(),
                }]
            })
            .collect()
    }

    /// Convert ToolDefinitions to Responses API tools
    fn convert_tools(tools: &[ToolDefinition]) -> Vec<ResponsesTool> {
        tools
            .iter()
            .map(|t| ResponsesTool {
                tool_type: "function".to_string(),
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
                strict: false,
            })
            .collect()
    }

    /// Send request and parse response
    fn send_request(&self, request: &ResponsesRequest) -> Result<ResponsesResponse> {
        let url = "https://api.openai.com/v1/responses";
        let auth_header = format!("Bearer {}", self.api_key);

        tracing::debug!("Sending request to OpenAI Responses API");
        tracing::debug!("Model: {}", self.model);

        // Configure TLS with custom CA certificates if provided
        let agent = if let Ok(cert_file) = std::env::var("SSL_CERT_FILE") {
            tracing::info!("Loading custom CA certificates from: {}", cert_file);
            match Self::build_tls_with_custom_ca(&cert_file) {
                Ok(tls) => {
                    tracing::info!("Custom CA certificates loaded successfully");
                    ureq::AgentBuilder::new()
                        .tls_connector(std::sync::Arc::new(tls))
                        .build()
                }
                Err(e) => {
                    tracing::error!("Failed to load custom CA certificates: {}", e);
                    tracing::warn!("Falling back to default TLS configuration");
                    ureq::agent()
                }
            }
        } else {
            ureq::agent()
        };

        let response_result = agent.post(url)
            .set("Content-Type", "application/json")
            .set("Authorization", &auth_header)
            .send_json(request);

        let response: ResponsesResponse = match response_result {
            Ok(resp) => {
                let body = resp.into_string()?;
                tracing::debug!("Raw OpenAI response: {}", body);
                serde_json::from_str(&body).map_err(|e| {
                    tracing::error!("Failed to parse OpenAI response: {}", e);
                    tracing::error!("Response body: {}", body);
                    anyhow::anyhow!("Failed to read JSON: {}", e)
                })?
            }
            Err(ureq::Error::Status(code, resp)) => {
                let error_body = resp
                    .into_string()
                    .unwrap_or_else(|_| "Unable to read error body".to_string());
                tracing::error!("OpenAI API error (status {}): {}", code, error_body);
                return Err(anyhow::anyhow!(
                    "OpenAI API error {}: {}",
                    code,
                    error_body
                ));
            }
            Err(e) => return Err(e.into()),
        };

        // Check if response is complete
        if response.status == "incomplete" {
            let reason = response
                .incomplete_details
                .as_ref()
                .map(|d| d.reason.clone())
                .unwrap_or_else(|| "unknown".to_string());
            return Err(anyhow::anyhow!(
                "Response incomplete: {}. Consider increasing max_output_tokens.",
                reason
            ));
        }

        Ok(response)
    }

    /// Extract text content from response output
    fn extract_text(output: &[ResponseOutput]) -> Option<String> {
        output
            .iter()
            .find(|o| o.output_type == "message" || o.output_type == "text")
            .and_then(|o| {
                if let Some(ref text) = o.text {
                    return Some(text.clone());
                }
                o.content
                    .as_ref()
                    .and_then(|c| c.first())
                    .map(|c| c.text.clone())
            })
    }

    /// Extract reasoning from response output (checks content first, then summary)
    fn extract_reasoning(output: &[ResponseOutput]) -> Option<String> {
        let reasoning_items: Vec<&ResponseOutput> = output
            .iter()
            .filter(|o| o.output_type == "reasoning")
            .collect();

        if reasoning_items.is_empty() {
            return None;
        }

        // Try content first (primary reasoning text)
        let content_parts: Vec<&str> = reasoning_items
            .iter()
            .flat_map(|o| {
                o.content
                    .iter()
                    .flat_map(|c| c.iter().map(|r| r.text.as_str()))
            })
            .collect();

        if !content_parts.is_empty() {
            return Some(content_parts.join("\n"));
        }

        // Fall back to summary
        let summary_parts: Vec<&str> = reasoning_items
            .iter()
            .flat_map(|o| {
                o.summary
                    .iter()
                    .flat_map(|s| s.iter().map(|r| r.text.as_str()))
            })
            .collect();

        if !summary_parts.is_empty() {
            Some(summary_parts.join("\n"))
        } else {
            tracing::debug!("Reasoning items found but no content or summary text");
            None
        }
    }

    /// Extract tool calls from response output
    fn extract_tool_calls(output: &[ResponseOutput]) -> Vec<ToolCallInfo> {
        output
            .iter()
            .filter(|o| o.output_type == "function_call")
            .filter_map(|o| {
                let call_id = o.call_id.as_ref()?;
                let name = o.name.as_ref()?;
                let arguments_str = o.arguments.as_ref()?;
                let arguments: serde_json::Value =
                    serde_json::from_str(arguments_str).unwrap_or_default();

                Some(ToolCallInfo {
                    id: call_id.clone(),
                    name: name.clone(),
                    arguments,
                })
            })
            .collect()
    }
}

impl LlmProvider for OpenAiProvider {
    fn supports_structured_output(&self) -> bool {
        true
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn chat(&self, messages: &[ChatMessage]) -> Result<String> {
        let input = Self::convert_to_input_items(messages);

        let request = ResponsesRequest {
            model: self.model.clone(),
            input,
            temperature: self.temperature,
            max_output_tokens: Some(self.max_tokens),
            tools: None,
            text: None,
            reasoning: self.reasoning_param(),
        };

        let response = self.send_request(&request)?;

        Self::extract_text(&response.output)
            .ok_or_else(|| anyhow::anyhow!("No text content in response"))
    }

    fn chat_with_schema(
        &self,
        messages: &[ChatMessage],
        schema: serde_json::Value,
        schema_name: &str,
    ) -> Result<String> {
        let input = Self::convert_to_input_items(messages);

        let request = ResponsesRequest {
            model: self.model.clone(),
            input,
            temperature: self.temperature,
            max_output_tokens: Some(self.max_tokens),
            tools: None,
            text: Some(ResponseTextFormat {
                format: ResponseFormatSpec {
                    format_type: "json_schema".to_string(),
                    name: schema_name.to_string(),
                    schema,
                    strict: true,
                },
            }),
            reasoning: self.reasoning_param(),
        };

        tracing::debug!("Sending request to OpenAI Responses API with JSON Schema");

        let response = self.send_request(&request)?;

        Self::extract_text(&response.output)
            .ok_or_else(|| anyhow::anyhow!("No text content in response"))
    }

    fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let input = Self::convert_to_input_items(messages);
        let wire_tools = Self::convert_tools(tools);

        let request = ResponsesRequest {
            model: self.model.clone(),
            input,
            temperature: self.temperature,
            max_output_tokens: Some(self.max_tokens),
            tools: if wire_tools.is_empty() {
                None
            } else {
                Some(wire_tools)
            },
            text: None,
            reasoning: self.reasoning_param(),
        };

        tracing::debug!("Sending chat_with_tools request to OpenAI Responses API");

        let response = self.send_request(&request)?;

        // Check for tool calls first
        let tool_calls = Self::extract_tool_calls(&response.output);
        if !tool_calls.is_empty() {
            tracing::info!("OpenAI returned {} tool calls", tool_calls.len());
            return Ok(LlmResponse::ToolCalls(tool_calls));
        }

        // Text response
        let text = Self::extract_text(&response.output)
            .ok_or_else(|| anyhow::anyhow!("No text content or tool calls in response"))?;
        let reasoning = Self::extract_reasoning(&response.output);
        tracing::debug!(
            "Response output types: {:?}",
            response.output.iter().map(|o| &o.output_type).collect::<Vec<_>>()
        );

        Ok(LlmResponse::Text {
            content: text,
            reasoning,
        })
    }
}

// ============================================================================
// Factory function
// ============================================================================

/// Create LLM provider based on runtime configuration
///
/// Selection logic:
/// 1. If model_path is provided → local FFI (in-process llama.cpp)
/// 2. If api_key is provided → OpenAI (cloud)
/// 3. Otherwise → error
pub fn create_provider(
    model_path: Option<String>,
    _base_url: String,
    model: String,
    api_key: Option<String>,
    temperature: Option<f32>,
    max_tokens: u32,
    reasoning_effort: Option<String>,
) -> Box<dyn LlmProvider> {
    if let Some(ref path) = model_path {
        tracing::info!("Using local llama.cpp provider (FFI)");
        let temp = temperature.unwrap_or(0.7);
        match crate::llm_local::LlamaLocalProvider::new(path, temp, max_tokens, 8192) {
            Ok(provider) => return Box::new(provider),
            Err(e) => {
                tracing::error!("Failed to create local provider: {}", e);
                panic!("Failed to load model from {}: {}", path, e);
            }
        }
    }

    if let Some(key) = api_key {
        tracing::info!("Using OpenAI provider (API key provided)");
        Box::new(OpenAiProvider::new(
            key,
            model,
            temperature,
            max_tokens,
            reasoning_effort,
        ))
    } else {
        panic!("No model_path or api_key provided. Set MODEL_PATH for local inference or OPENAI_API_KEY for cloud.");
    }
}

