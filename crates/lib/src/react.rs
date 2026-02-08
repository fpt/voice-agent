use crate::llm::{ChatMessage, LlmProvider, LlmResponse, ToolCallInfo};
use crate::tool::ToolRegistry;
use crate::AgentError;

const DEFAULT_MAX_ITERATIONS: u32 = 10;

/// Run a ReAct (Reason+Act) loop: call LLM with tools, execute tool calls, repeat until text response.
///
/// Returns the final text response and optional reasoning from the LLM.
pub fn run(
    client: &dyn LlmProvider,
    messages: &mut Vec<ChatMessage>,
    tools: &ToolRegistry,
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
                        "Tool '{}' ({}): {} chars result",
                        call.name,
                        call.id,
                        result.len()
                    );

                    messages.push(ChatMessage::tool_result(
                        call.id.clone(),
                        call.name.clone(),
                        result,
                    ));
                }
            }
        }
    }

    Err(AgentError::InternalError(format!(
        "ReAct loop exceeded maximum iterations ({})",
        max_iter
    )))
}

/// Execute a single tool call, returning the result string (or error message)
fn execute_tool_call(tools: &ToolRegistry, call: &ToolCallInfo) -> String {
    match tools.call(&call.name, call.arguments.clone()) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!("Tool '{}' error: {}", call.name, e);
            format!("Error executing tool '{}': {}", call.name, e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatRole, ToolDefinition};
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
