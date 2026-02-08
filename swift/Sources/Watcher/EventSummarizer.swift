import Foundation

/// Summarizes a batch of watcher events into a concise natural-language string.
public struct EventSummarizer {

    /// Summarize events into a string suitable for agent.step().
    /// Returns nil if no interesting events found.
    public static func summarize(_ events: [WatcherEvent]) -> String? {
        var toolCounts: [String: Int] = [:]
        var editedFiles: [String] = []
        var bashCommands: [String] = []
        var testResults: [String] = []
        var commitMessages: [String] = []
        var stopCount = 0
        // var otherDetails: [String] = []

        for event in events {
            switch event {
            case .session(let se):
                // Skip noise types
                let noiseTypes = ["progress", "file-history-snapshot", "queue-operation",
                                  "system", "result", "summary"]
                if noiseTypes.contains(se.type) { continue }

                // Extract tool uses from assistant messages
                for toolUse in se.toolUses {
                    let toolName = toolUse["name"] as? String ?? "unknown"
                    toolCounts[toolName, default: 0] += 1

                    if let input = toolUse["input"] as? [String: Any] {
                        // File paths from Write/Edit/Read
                        if let fp = input["file_path"] as? String {
                            let short = (fp as NSString).lastPathComponent
                            if !editedFiles.contains(short) {
                                editedFiles.append(short)
                            }
                        }
                        // Bash commands
                        if toolName == "Bash", let cmd = input["command"] as? String {
                            let short = String(cmd.prefix(80))
                            bashCommands.append(short)
                        }
                    }
                }

                // Check for text that looks like test results
                if let text = se.textContent {
                    if text.contains("passed") && text.contains("failed") {
                        // Extract test summary line
                        for line in text.components(separatedBy: "\n") {
                            if line.contains("passed") || line.contains("failed") {
                                testResults.append(String(line.prefix(100)))
                                break
                            }
                        }
                    }
                    if text.contains("git commit") || text.contains("Co-Authored-By") {
                        // Try to find commit message
                        for line in text.components(separatedBy: "\n") {
                            if line.contains("-m ") || line.contains("commit") {
                                commitMessages.append(String(line.prefix(80)))
                                break
                            }
                        }
                    }
                }

            case .hook(let he):
                if he.event == "Stop" {
                    stopCount += 1
                    continue
                }

                if let tool = he.toolName {
                    toolCounts[tool, default: 0] += 1
                }
                if let fp = he.filePath {
                    let short = (fp as NSString).lastPathComponent
                    if !editedFiles.contains(short) {
                        editedFiles.append(short)
                    }
                }
            }
        }

        // Build summary
        var parts: [String] = []

        // Tool usage summary
        let interestingTools = toolCounts.filter { ["Write", "Edit", "MultiEdit", "Bash", "Read"].contains($0.key) }
        if !interestingTools.isEmpty {
            let toolSummary = interestingTools
                .sorted { $0.value > $1.value }
                .map { "\($0.key) x\($0.value)" }
                .joined(separator: ", ")
            parts.append("Tools used: \(toolSummary)")
        }

        // Files
        if !editedFiles.isEmpty {
            let fileList = editedFiles.prefix(5).joined(separator: ", ")
            let suffix = editedFiles.count > 5 ? " (+\(editedFiles.count - 5) more)" : ""
            parts.append("Files: \(fileList)\(suffix)")
        }

        // Bash commands
        if !bashCommands.isEmpty {
            let cmdList = bashCommands.prefix(3).joined(separator: "; ")
            parts.append("Ran: \(cmdList)")
        }

        // Test results
        if !testResults.isEmpty {
            parts.append("Tests: \(testResults.first!)")
        }

        // Commits
        if !commitMessages.isEmpty {
            parts.append("Committed: \(commitMessages.first!)")
        }

        // Stop events
        if stopCount > 0 {
            parts.append("Claude Code finished responding")
        }

        guard !parts.isEmpty else { return nil }

        var summary = "[Claude Code Update] " + parts.joined(separator: ". ")
        // Cap at 500 chars
        if summary.count > 500 {
            summary = String(summary.prefix(497)) + "..."
        }
        return summary
    }
}
