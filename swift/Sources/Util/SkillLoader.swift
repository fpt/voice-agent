import Foundation

/// Discovers and parses SKILL.md files from ~/.claude plugins directory.
public struct SkillLoader {

    public struct SkillDefinition {
        public let name: String
        public let description: String
        public let prompt: String
    }

    /// Scan directories for SKILL.md files and parse them.
    /// Searches: project-local `skills/` directory + `~/.claude/plugins`.
    public static func loadAll(projectDir: String? = nil) -> [SkillDefinition] {
        var searchDirs: [String] = []

        // Project-local skills/ directory
        if let dir = projectDir {
            let skillsDir = "\(dir)/skills"
            if FileManager.default.fileExists(atPath: skillsDir) {
                searchDirs.append(skillsDir)
            }
        }

        // ~/.claude/plugins
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let pluginsDir = "\(home)/.claude/plugins"
        if FileManager.default.fileExists(atPath: pluginsDir) {
            searchDirs.append(pluginsDir)
        }

        var results: [SkillDefinition] = []
        let fm = FileManager.default

        for dir in searchDirs {
            if let enumerator = fm.enumerator(atPath: dir) {
                while let relativePath = enumerator.nextObject() as? String {
                    guard relativePath.hasSuffix("/SKILL.md") || relativePath == "SKILL.md" else {
                        continue
                    }
                    let fullPath = "\(dir)/\(relativePath)"
                    if let skill = parse(path: fullPath) {
                        results.append(skill)
                    }
                }
            }
        }

        return results
    }

    /// Parse a SKILL.md file: YAML frontmatter + markdown body.
    static func parse(path: String) -> SkillDefinition? {
        guard let data = FileManager.default.contents(atPath: path),
              let content = String(data: data, encoding: .utf8) else {
            return nil
        }

        // Must start with "---"
        let trimmed = content.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("---") else { return nil }

        // Find closing "---"
        let afterOpening = trimmed.dropFirst(3)
        guard let closingRange = afterOpening.range(of: "\n---") else { return nil }

        let frontmatter = String(afterOpening[afterOpening.startIndex..<closingRange.lowerBound])
        let body = String(afterOpening[closingRange.upperBound...])
            .trimmingCharacters(in: .whitespacesAndNewlines)

        // Simple YAML parsing for name and description
        var name: String?
        var description: String?

        for line in frontmatter.components(separatedBy: "\n") {
            let trimmedLine = line.trimmingCharacters(in: .whitespaces)
            if trimmedLine.hasPrefix("name:") {
                name = extractYamlValue(trimmedLine, key: "name")
            } else if trimmedLine.hasPrefix("description:") {
                description = extractYamlValue(trimmedLine, key: "description")
            }
        }

        guard let skillName = name, !skillName.isEmpty else { return nil }

        return SkillDefinition(
            name: skillName,
            description: description ?? "",
            prompt: body
        )
    }

    private static func extractYamlValue(_ line: String, key: String) -> String {
        let value = String(line.dropFirst(key.count + 1))  // drop "key:"
            .trimmingCharacters(in: .whitespaces)
        // Remove surrounding quotes if present
        if (value.hasPrefix("\"") && value.hasSuffix("\"")) ||
           (value.hasPrefix("'") && value.hasSuffix("'")) {
            return String(value.dropFirst().dropLast())
        }
        return value
    }
}
