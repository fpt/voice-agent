use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::event_router::{EventRouter, ReportEventTool};
use crate::llm::{ImageContent, ToolDefinition};
use crate::situation::{ReadSituationMessagesTool, SituationMessages};
use crate::skill::{SkillLookupTool, SkillRegistry};
use crate::AgentError;

/// Result of a tool call, containing text and optional images
#[derive(Debug)]
pub struct ToolResult {
    pub text: String,
    pub images: Vec<ImageContent>,
}

impl ToolResult {
    pub fn text(s: String) -> Self {
        Self {
            text: s,
            images: vec![],
        }
    }

    pub fn with_images(text: String, images: Vec<ImageContent>) -> Self {
        Self { text, images }
    }
}

impl From<String> for ToolResult {
    fn from(s: String) -> Self {
        Self::text(s)
    }
}

/// Trait for tool implementations
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError>;

    /// Optional dynamic description that can change at runtime (e.g. include live stats).
    /// When `Some`, this overrides `description()` in tool definitions sent to the LLM.
    fn dynamic_description(&self) -> Option<String> {
        None
    }
}

/// Trait for accessing tools (implemented by both ToolRegistry and FilteredToolRegistry)
pub trait ToolAccess {
    fn get_definitions(&self) -> Vec<ToolDefinition>;
    fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, AgentError>;
    fn is_empty(&self) -> bool;
}

/// Registry of available tools
pub struct ToolRegistry {
    tools: Vec<Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn ToolHandler>) {
        tracing::info!("Registered tool: {}", tool.name());
        self.tools.push(tool);
    }

    /// Create a filtered view that only exposes the named tools
    pub fn filtered(&self, allowed: &[String]) -> FilteredToolRegistry<'_> {
        FilteredToolRegistry {
            tools: &self.tools,
            allowed: allowed.to_vec(),
        }
    }
}

impl ToolAccess for ToolRegistry {
    fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t
                    .dynamic_description()
                    .unwrap_or_else(|| t.description().to_string()),
                parameters: t.parameters_schema(),
            })
            .collect()
    }

    fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| AgentError::InternalError(format!("Unknown tool: {}", name)))?;

        tracing::info!("Calling tool: {} with args: {}", name, args);
        let result = tool.call(args)?;
        tracing::debug!("Tool {} returned {} chars", name, result.text.len());
        Ok(result)
    }

    fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// A filtered view of a ToolRegistry that only exposes certain tools
pub struct FilteredToolRegistry<'a> {
    tools: &'a [Box<dyn ToolHandler>],
    allowed: Vec<String>,
}

impl<'a> ToolAccess for FilteredToolRegistry<'a> {
    fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter(|t| self.allowed.iter().any(|a| a == t.name()))
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t
                    .dynamic_description()
                    .unwrap_or_else(|| t.description().to_string()),
                parameters: t.parameters_schema(),
            })
            .collect()
    }

    fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        if !self.allowed.iter().any(|a| a == name) {
            return Err(AgentError::InternalError(format!("Tool not allowed: {}", name)));
        }
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| AgentError::InternalError(format!("Unknown tool: {}", name)))?;

        tracing::info!("Calling tool: {} with args: {}", name, args);
        let result = tool.call(args)?;
        tracing::debug!("Tool {} returned {} chars", name, result.text.len());
        Ok(result)
    }

    fn is_empty(&self) -> bool {
        !self.tools.iter().any(|t| self.allowed.iter().any(|a| a == t.name()))
    }
}

/// Create default tool registry with built-in tools
pub fn create_default_registry(
    working_dir: PathBuf,
    skill_registry: Arc<SkillRegistry>,
    event_router: Option<Arc<EventRouter>>,
    situation: Arc<SituationMessages>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(working_dir.clone())));
    registry.register(Box::new(GlobTool::new(working_dir)));
    registry.register(Box::new(TaskTool::new()));
    registry.register(Box::new(SkillLookupTool::new(skill_registry)));
    registry.register(Box::new(ReadSituationMessagesTool::new(situation)));
    if let Some(router) = event_router {
        registry.register(Box::new(ReportEventTool::new(router)));
    }
    registry
}

// ============================================================================
// ReadTool — Read file contents with line numbers
// ============================================================================

pub struct ReadTool {
    working_dir: PathBuf,
}

impl ReadTool {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    fn resolve_path(&self, file_path: &str) -> PathBuf {
        let path = Path::new(file_path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.working_dir.join(path)
        }
    }
}

impl ToolHandler for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a file's contents with line numbers. Returns the file content formatted with line numbers."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to read (absolute or relative to working directory)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based, default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default: 2000)"
                }
            },
            "required": ["file_path"]
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| AgentError::ParseError("Missing file_path argument".to_string()))?;
        let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = args["limit"].as_u64().unwrap_or(2000) as usize;

        let resolved = self.resolve_path(file_path);

        let content = std::fs::read_to_string(&resolved).map_err(|e| {
            AgentError::InternalError(format!("Failed to read {}: {}", resolved.display(), e))
        })?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // offset is 1-based
        let start = (offset - 1).min(total_lines);
        let end = (start + limit).min(total_lines);

        let mut output = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_num = start + i + 1;
            output.push_str(&format!("{:>6}\t{}\n", line_num, line));
        }

        if end < total_lines {
            output.push_str(&format!(
                "\n... ({} more lines, {} total)\n",
                total_lines - end,
                total_lines
            ));
        }

        Ok(ToolResult::text(output))
    }
}

// ============================================================================
// GlobTool — Find files by glob pattern
// ============================================================================

pub struct GlobTool {
    working_dir: PathBuf,
}

impl GlobTool {
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }
}

impl ToolHandler for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern (e.g. \"**/*.rs\", \"src/**/*.swift\"). Returns matching file paths."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files (e.g. \"**/*.rs\", \"src/*.swift\")"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (default: working directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| AgentError::ParseError("Missing pattern argument".to_string()))?;

        let base_dir = args["path"]
            .as_str()
            .map(|p| {
                let path = Path::new(p);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.working_dir.join(path)
                }
            })
            .unwrap_or_else(|| self.working_dir.clone());

        let full_pattern = base_dir.join(pattern);
        let full_pattern_str = full_pattern.to_string_lossy();

        let mut matches: Vec<String> = Vec::new();
        let entries = glob::glob(&full_pattern_str).map_err(|e| {
            AgentError::InternalError(format!("Invalid glob pattern '{}': {}", full_pattern_str, e))
        })?;

        for entry in entries {
            match entry {
                Ok(path) => {
                    let display = path
                        .strip_prefix(&self.working_dir)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    matches.push(display);
                }
                Err(e) => {
                    tracing::warn!("Glob error for entry: {}", e);
                }
            }
        }

        matches.sort();

        if matches.is_empty() {
            Ok(ToolResult::text(format!("No files found matching '{}'", pattern)))
        } else {
            let count = matches.len();
            let mut output = matches.join("\n");
            output.push_str(&format!("\n\n({} files found)", count));
            Ok(ToolResult::text(output))
        }
    }
}

// ============================================================================
// TaskTool — In-memory task list
// ============================================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TaskItem {
    id: u32,
    subject: String,
    description: String,
    status: String, // "pending", "in_progress", "completed"
}

pub struct TaskTool {
    tasks: Mutex<Vec<TaskItem>>,
    next_id: Mutex<u32>,
}

impl TaskTool {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
        }
    }
}

impl ToolHandler for TaskTool {
    fn name(&self) -> &str {
        "tasks"
    }

    fn description(&self) -> &str {
        "Manage an in-memory task list. Actions: create (new task), update (change status), list (show all tasks)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform: 'create', 'update', or 'list'",
                    "enum": ["create", "update", "list"]
                },
                "subject": {
                    "type": "string",
                    "description": "Task subject/title (for create)"
                },
                "description": {
                    "type": "string",
                    "description": "Task description (for create)"
                },
                "task_id": {
                    "type": "integer",
                    "description": "Task ID (for update)"
                },
                "status": {
                    "type": "string",
                    "description": "New status (for update): 'pending', 'in_progress', 'completed'",
                    "enum": ["pending", "in_progress", "completed"]
                }
            },
            "required": ["action"]
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| AgentError::ParseError("Missing action argument".to_string()))?;

        match action {
            "create" => {
                let subject = args["subject"]
                    .as_str()
                    .unwrap_or("Untitled task")
                    .to_string();
                let description = args["description"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                let mut tasks = self.tasks.lock().map_err(|e| {
                    AgentError::InternalError(format!("Lock error: {}", e))
                })?;
                let mut next_id = self.next_id.lock().map_err(|e| {
                    AgentError::InternalError(format!("Lock error: {}", e))
                })?;

                let id = *next_id;
                *next_id += 1;

                let task = TaskItem {
                    id,
                    subject: subject.clone(),
                    description,
                    status: "pending".to_string(),
                };
                tasks.push(task);

                Ok(ToolResult::text(format!("Created task #{}: {}", id, subject)))
            }
            "update" => {
                let task_id = args["task_id"]
                    .as_u64()
                    .ok_or_else(|| AgentError::ParseError("Missing task_id for update".to_string()))?
                    as u32;
                let new_status = args["status"]
                    .as_str()
                    .ok_or_else(|| AgentError::ParseError("Missing status for update".to_string()))?;

                let mut tasks = self.tasks.lock().map_err(|e| {
                    AgentError::InternalError(format!("Lock error: {}", e))
                })?;

                let task = tasks
                    .iter_mut()
                    .find(|t| t.id == task_id)
                    .ok_or_else(|| {
                        AgentError::InternalError(format!("Task #{} not found", task_id))
                    })?;

                task.status = new_status.to_string();
                Ok(ToolResult::text(format!(
                    "Updated task #{} '{}' → {}",
                    task_id, task.subject, new_status
                )))
            }
            "list" => {
                let tasks = self.tasks.lock().map_err(|e| {
                    AgentError::InternalError(format!("Lock error: {}", e))
                })?;

                if tasks.is_empty() {
                    return Ok(ToolResult::text("No tasks.".to_string()));
                }

                let mut output = String::from("Tasks:\n");
                for task in tasks.iter() {
                    let status_icon = match task.status.as_str() {
                        "completed" => "[x]",
                        "in_progress" => "[~]",
                        _ => "[ ]",
                    };
                    output.push_str(&format!(
                        "  #{} {} {} - {}\n",
                        task.id, status_icon, task.subject, task.status
                    ));
                    if !task.description.is_empty() {
                        output.push_str(&format!("       {}\n", task.description));
                    }
                }
                Ok(ToolResult::text(output))
            }
            _ => Err(AgentError::ParseError(format!(
                "Unknown action: {}. Use 'create', 'update', or 'list'.",
                action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_tool() {
        let dir = std::env::temp_dir();
        let mut file = NamedTempFile::new_in(&dir).unwrap();
        writeln!(file, "line one").unwrap();
        writeln!(file, "line two").unwrap();
        writeln!(file, "line three").unwrap();

        let tool = ReadTool::new(dir);
        let result = tool
            .call(serde_json::json!({
                "file_path": file.path().to_string_lossy().to_string()
            }))
            .unwrap()
            .text;

        assert!(result.contains("line one"));
        assert!(result.contains("line two"));
        assert!(result.contains("line three"));
        // Check line numbers
        assert!(result.contains("1\t"));
        assert!(result.contains("2\t"));
    }

    #[test]
    fn test_read_tool_with_offset_limit() {
        let dir = std::env::temp_dir();
        let mut file = NamedTempFile::new_in(&dir).unwrap();
        for i in 1..=10 {
            writeln!(file, "line {}", i).unwrap();
        }

        let tool = ReadTool::new(dir);
        let result = tool
            .call(serde_json::json!({
                "file_path": file.path().to_string_lossy().to_string(),
                "offset": 3,
                "limit": 2
            }))
            .unwrap()
            .text;

        assert!(result.contains("line 3"));
        assert!(result.contains("line 4"));
        assert!(!result.contains("line 5"));
        assert!(result.contains("more lines"));
    }

    #[test]
    fn test_glob_tool() {
        let dir = std::env::temp_dir().join("glob_test_tool");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("test.txt"), "hello").unwrap();
        std::fs::write(dir.join("test.rs"), "fn main()").unwrap();

        let tool = GlobTool::new(dir.clone());
        let result = tool
            .call(serde_json::json!({
                "pattern": "*.txt"
            }))
            .unwrap()
            .text;

        assert!(result.contains("test.txt"));
        assert!(!result.contains("test.rs"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_task_tool_lifecycle() {
        let tool = TaskTool::new();

        // Create
        let result = tool
            .call(serde_json::json!({
                "action": "create",
                "subject": "Fix bug",
                "description": "Fix the audio bug"
            }))
            .unwrap()
            .text;
        assert!(result.contains("#1"));
        assert!(result.contains("Fix bug"));

        // List
        let result = tool
            .call(serde_json::json!({ "action": "list" }))
            .unwrap()
            .text;
        assert!(result.contains("Fix bug"));
        assert!(result.contains("pending"));

        // Update
        let result = tool
            .call(serde_json::json!({
                "action": "update",
                "task_id": 1,
                "status": "completed"
            }))
            .unwrap()
            .text;
        assert!(result.contains("completed"));

        // List again
        let result = tool
            .call(serde_json::json!({ "action": "list" }))
            .unwrap()
            .text;
        assert!(result.contains("[x]"));
    }

    #[test]
    fn test_registry() {
        let dir = std::env::temp_dir();
        let skill_reg = Arc::new(SkillRegistry::new());
        let situation = Arc::new(SituationMessages::default());
        let registry = create_default_registry(dir, skill_reg, None, situation);

        let defs = registry.get_definitions();
        assert_eq!(defs.len(), 5);

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"read"));
        assert!(names.contains(&"glob"));
        assert!(names.contains(&"tasks"));
        assert!(names.contains(&"lookup_skill"));
        assert!(names.contains(&"read_situation_messages"));
    }
}
