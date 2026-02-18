use crate::llm::{ChatMessage, LlmProvider, LlmResponse, ToolCallInfo};
use crate::tool::{ToolAccess, ToolResult};
use crate::AgentError;

const DEFAULT_MAX_ITERATIONS: u32 = 10;

/// Run a ReAct (Reason+Act) loop: call LLM with tools, execute tool calls, repeat until text response.
///
/// Returns the final text response and optional reasoning from the LLM.
pub fn run(
    client: &dyn LlmProvider,
    messages: &mut Vec<ChatMessage>,
    tools: &dyn ToolAccess,
    max_iterations: Option<u32>,
) -> Result<(String, Option<String>), AgentError> {
    let max_iter = max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS);
    let tool_defs = tools.get_definitions();

    for iteration in 0..max_iter {
        tracing::info!("ReAct iteration {}/{}", iteration + 1, max_iter);

        let response = client
            .chat_with_tools(messages, &tool_defs)
            .map_err(|e| AgentError::NetworkError(e.to_string()))?;

        match response {
            LlmResponse::Text { content, reasoning } => {
                tracing::info!("ReAct complete: text response after {} iterations", iteration + 1);
                return Ok((content, reasoning));
            }
            LlmResponse::ToolCalls(calls) => {
                tracing::info!(
                    "ReAct iteration {}: {} tool call(s)",
                    iteration + 1,
                    calls.len()
                );

                // Record the assistant's tool calls in message history
                messages.push(ChatMessage::assistant_tool_calls(calls.clone()));

                // Execute each tool call and add results
                for call in &calls {
                    let result = execute_tool_call(tools, call);

                    tracing::info!(
                        "Tool '{}' ({}): {} chars result, {} images",
                        call.name,
                        call.id,
                        result.text.len(),
                        result.images.len(),
                    );

                    if result.images.is_empty() {
                        messages.push(ChatMessage::tool_result(
                            call.id.clone(),
                            call.name.clone(),
                            result.text,
                        ));
                    } else {
                        messages.push(ChatMessage::tool_result_with_images(
                            call.id.clone(),
                            call.name.clone(),
                            result.text,
                            result.images,
                        ));
                    }
                }
            }
        }
    }

    Err(AgentError::InternalError(format!(
        "ReAct loop exceeded maximum iterations ({})",
        max_iter
    )))
}

/// Execute a single tool call, returning the result (or error message)
fn execute_tool_call(tools: &dyn ToolAccess, call: &ToolCallInfo) -> ToolResult {
    match tools.call(&call.name, call.arguments.clone()) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("Tool '{}' error: {}", call.name, e);
            ToolResult::text(format!("Error executing tool '{}': {}", call.name, e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatRole, ToolDefinition};
    use crate::tool::ToolRegistry;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock LLM provider for testing the ReAct loop
    struct MockProvider {
        responses: Vec<LlmResponse>,
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    impl LlmProvider for MockProvider {
        fn chat(&self, _messages: &[ChatMessage]) -> anyhow::Result<String> {
            Ok("mock".to_string())
        }

        fn supports_tools(&self) -> bool {
            true
        }

        fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            if idx < self.responses.len() {
                // We need to clone the response — reconstruct it
                let resp = &self.responses[idx];
                match resp {
                    LlmResponse::Text { content, reasoning } => Ok(LlmResponse::Text {
                        content: content.clone(),
                        reasoning: reasoning.clone(),
                    }),
                    LlmResponse::ToolCalls(calls) => {
                        Ok(LlmResponse::ToolCalls(calls.clone()))
                    }
                }
            } else {
                Ok(LlmResponse::Text {
                    content: "fallback".to_string(),
                    reasoning: None,
                })
            }
        }
    }

    #[test]
    fn test_react_direct_text_response() {
        let provider = MockProvider::new(vec![LlmResponse::Text {
            content: "Hello!".to_string(),
            reasoning: None,
        }]);
        let mut messages = vec![ChatMessage::user("Hi".to_string())];
        let tools = ToolRegistry::new();

        let (text, reasoning) = run(&provider, &mut messages, &tools, Some(5)).unwrap();
        assert_eq!(text, "Hello!");
        assert!(reasoning.is_none());
    }

    #[test]
    fn test_react_tool_then_text() {
        let provider = MockProvider::new(vec![
            LlmResponse::ToolCalls(vec![ToolCallInfo {
                id: "call_1".to_string(),
                name: "tasks".to_string(),
                arguments: serde_json::json!({"action": "list"}),
            }]),
            LlmResponse::Text {
                content: "There are no tasks.".to_string(),
                reasoning: None,
            },
        ]);

        let mut messages = vec![ChatMessage::user("List tasks".to_string())];

        // Create registry with task tool
        use crate::tool::TaskTool;
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(TaskTool::new()));

        let (text, _) = run(&provider, &mut messages, &tools, Some(5)).unwrap();
        assert_eq!(text, "There are no tasks.");

        // Messages should contain: user, assistant(tool_calls), tool_result
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ChatRole::User);
        assert_eq!(messages[1].role, ChatRole::Assistant);
        assert!(messages[1].tool_calls.is_some());
        assert_eq!(messages[2].role, ChatRole::Tool);
    }

    /// Mock tool that returns a ToolResult with images
    struct MockImageTool;

    impl crate::tool::ToolHandler for MockImageTool {
        fn name(&self) -> &str {
            "capture_screen"
        }
        fn description(&self) -> &str {
            "Mock screen capture"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn call(&self, _args: serde_json::Value) -> Result<ToolResult, crate::AgentError> {
            Ok(ToolResult::with_images(
                "Window: Chrome, Size: 1920x1080".to_string(),
                vec![crate::llm::ImageContent {
                    base64: "iVBORw0KGgoAAAANS".to_string(),
                    media_type: "image/png".to_string(),
                }],
            ))
        }
    }

    #[test]
    fn test_react_tool_with_images_stores_in_messages() {
        let provider = MockProvider::new(vec![
            LlmResponse::ToolCalls(vec![ToolCallInfo {
                id: "call_img".to_string(),
                name: "capture_screen".to_string(),
                arguments: serde_json::json!({"process_name": "Chrome"}),
            }]),
            LlmResponse::Text {
                content: "I can see a Chrome window.".to_string(),
                reasoning: None,
            },
        ]);

        let mut messages = vec![ChatMessage::user("capture Chrome".to_string())];
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(MockImageTool));

        let (text, _) = run(&provider, &mut messages, &tools, Some(5)).unwrap();
        assert_eq!(text, "I can see a Chrome window.");

        // Messages: user, assistant(tool_calls), tool_result_with_images
        assert_eq!(messages.len(), 3);

        // The tool result message should have images
        let tool_msg = &messages[2];
        assert_eq!(tool_msg.role, ChatRole::Tool);
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_img"));
        assert_eq!(tool_msg.tool_name.as_deref(), Some("capture_screen"));
        assert_eq!(tool_msg.content, "Window: Chrome, Size: 1920x1080");
        assert_eq!(tool_msg.images.len(), 1, "Tool result should carry 1 image");
        assert_eq!(tool_msg.images[0].media_type, "image/png");
        assert_eq!(tool_msg.images[0].base64, "iVBORw0KGgoAAAANS");
    }

    #[test]
    fn test_react_tool_without_images_has_empty_images() {
        let provider = MockProvider::new(vec![
            LlmResponse::ToolCalls(vec![ToolCallInfo {
                id: "call_1".to_string(),
                name: "tasks".to_string(),
                arguments: serde_json::json!({"action": "list"}),
            }]),
            LlmResponse::Text {
                content: "done".to_string(),
                reasoning: None,
            },
        ]);

        let mut messages = vec![ChatMessage::user("list".to_string())];
        use crate::tool::TaskTool;
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(TaskTool::new()));

        run(&provider, &mut messages, &tools, Some(5)).unwrap();

        let tool_msg = &messages[2];
        assert_eq!(tool_msg.role, ChatRole::Tool);
        assert!(tool_msg.images.is_empty(), "Plain tool result should have no images");
    }

    #[test]
    fn test_react_max_iterations() {
        // Provider always returns tool calls — should hit max iterations
        let provider = MockProvider::new(vec![
            LlmResponse::ToolCalls(vec![ToolCallInfo {
                id: "call_1".to_string(),
                name: "tasks".to_string(),
                arguments: serde_json::json!({"action": "list"}),
            }]),
            LlmResponse::ToolCalls(vec![ToolCallInfo {
                id: "call_2".to_string(),
                name: "tasks".to_string(),
                arguments: serde_json::json!({"action": "list"}),
            }]),
            LlmResponse::ToolCalls(vec![ToolCallInfo {
                id: "call_3".to_string(),
                name: "tasks".to_string(),
                arguments: serde_json::json!({"action": "list"}),
            }]),
        ]);

        let mut messages = vec![ChatMessage::user("Loop forever".to_string())];

        use crate::tool::TaskTool;
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(TaskTool::new()));

        let result = run(&provider, &mut messages, &tools, Some(2));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("maximum iterations"));
    }
}
