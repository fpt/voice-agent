import Foundation

/// An event parsed from a Claude Code session JSONL file
public struct SessionEvent: @unchecked Sendable {
    public let type: String              // "user", "assistant", etc.
    public let timestamp: String?
    public let message: [String: Any]?   // the "message" field if present
    public let raw: [String: Any]        // full parsed JSON line
    public let sessionId: String?        // working directory (session identifier)

    /// Extract tool_use content blocks from assistant messages
    public var toolUses: [[String: Any]] {
        guard let message = message,
              let content = message["content"] as? [[String: Any]] else { return [] }
        return content.filter { ($0["type"] as? String) == "tool_use" }
    }

    /// Extract text content from messages
    public var textContent: String? {
        guard let message = message else { return nil }
        // Simple string content
        if let text = message["content"] as? String { return text }
        // Array content with text blocks
        if let content = message["content"] as? [[String: Any]] {
            let texts = content
                .filter { ($0["type"] as? String) == "text" }
                .compactMap { $0["text"] as? String }
            return texts.isEmpty ? nil : texts.joined(separator: "\n")
        }
        return nil
    }

    /// Convert to JSON string for the Rust EventRouter.
    public func toRouterJSON() -> String? {
        var dict: [String: Any] = ["source": "session", "type": type]
        let uses = toolUses.map { tu -> [String: Any] in
            var entry: [String: Any] = ["name": tu["name"] as? String ?? "unknown"]
            if let input = tu["input"] as? [String: Any] { entry["input"] = input }
            return entry
        }
        if !uses.isEmpty { dict["tool_uses"] = uses }
        if let text = textContent { dict["text_content"] = text }
        if let sid = sessionId { dict["session_id"] = sid }
        guard let data = try? JSONSerialization.data(withJSONObject: dict) else { return nil }
        return String(data: data, encoding: .utf8)
    }
}

/// Watches a single session JSONL file for new appended lines using DispatchSource (kqueue).
public class SessionJSONLWatcher {
    private let filePath: String
    /// Session identifier derived from the JSONL path (decoded project directory).
    private let sessionId: String?
    private var lastOffset: UInt64 = 0
    private var dispatchSource: DispatchSourceFileSystemObject?
    private var fileDescriptor: Int32 = -1
    private var continuation: AsyncStream<SessionEvent>.Continuation?

    public init(filePath: String) {
        self.filePath = filePath
        // Derive project path from JSONL path:
        // ~/.claude/projects/-Users-foo-project/xxx.jsonl → /Users/foo/project
        let dir = (filePath as NSString).deletingLastPathComponent
        let dirName = (dir as NSString).lastPathComponent
        if dirName.hasPrefix("-") {
            self.sessionId = dirName.replacingOccurrences(of: "-", with: "/")
        } else {
            self.sessionId = nil
        }
    }

    /// Returns an AsyncStream of new events. Starts watching on first iteration.
    public func events() -> AsyncStream<SessionEvent> {
        AsyncStream { continuation in
            self.continuation = continuation
            self.startWatching()

            continuation.onTermination = { @Sendable _ in
                self.stop()
            }
        }
    }

    /// Auto-detect the most recently modified session JSONL in the Claude projects directory.
    public static func findActiveSessionJSONL(projectDir: String? = nil) -> String? {
        let dir: String
        if let projectDir = projectDir {
            dir = projectDir
        } else {
            // Derive from cwd: /Users/foo/project → ~/.claude/projects/-Users-foo-project/
            let cwd = FileManager.default.currentDirectoryPath
            let encoded = cwd.replacingOccurrences(of: "/", with: "-")
            dir = NSHomeDirectory() + "/.claude/projects/" + encoded
        }

        let fm = FileManager.default
        guard let entries = try? fm.contentsOfDirectory(atPath: dir) else { return nil }

        var best: (path: String, date: Date)?
        for entry in entries where entry.hasSuffix(".jsonl") {
            let fullPath = (dir as NSString).appendingPathComponent(entry)
            guard let attrs = try? fm.attributesOfItem(atPath: fullPath),
                  let modDate = attrs[.modificationDate] as? Date else { continue }
            if best == nil || modDate > best!.date {
                best = (fullPath, modDate)
            }
        }
        return best?.path
    }

    private func startWatching() {
        // Set initial offset to current file size (skip existing content)
        if let attrs = try? FileManager.default.attributesOfItem(atPath: filePath),
           let size = attrs[.size] as? UInt64 {
            lastOffset = size
        }

        fileDescriptor = open(filePath, O_RDONLY | O_EVTONLY)
        guard fileDescriptor >= 0 else {
            print("[Watcher] Failed to open \(filePath)")
            continuation?.finish()
            return
        }

        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fileDescriptor,
            eventMask: [.write, .extend],
            queue: DispatchQueue.global(qos: .utility)
        )

        source.setEventHandler { [weak self] in
            self?.readNewLines()
        }

        source.setCancelHandler { [weak self] in
            if let fd = self?.fileDescriptor, fd >= 0 {
                close(fd)
                self?.fileDescriptor = -1
            }
        }

        dispatchSource = source
        source.resume()
    }

    private func readNewLines() {
        guard let handle = FileHandle(forReadingAtPath: filePath) else { return }
        defer { handle.closeFile() }

        handle.seek(toFileOffset: lastOffset)
        let data = handle.readDataToEndOfFile()
        guard !data.isEmpty else { return }

        lastOffset += UInt64(data.count)

        guard let text = String(data: data, encoding: .utf8) else { return }
        let lines = text.components(separatedBy: "\n")

        for line in lines {
            let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { continue }

            guard let jsonData = trimmed.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else {
                continue
            }

            let event = SessionEvent(
                type: json["type"] as? String ?? "unknown",
                timestamp: json["timestamp"] as? String,
                message: json["message"] as? [String: Any],
                raw: json,
                sessionId: self.sessionId
            )
            continuation?.yield(event)
        }
    }

    public func stop() {
        dispatchSource?.cancel()
        dispatchSource = nil
        continuation?.finish()
        continuation = nil
    }

    deinit {
        stop()
    }
}
