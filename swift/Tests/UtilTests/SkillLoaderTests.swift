import XCTest

@testable import Util

/// `SkillLoader.parse` is a hand-rolled frontmatter reader, not a YAML parser,
/// so its edge cases are worth pinning: a skill that silently fails to load just
/// makes the agent quietly less capable.
final class SkillLoaderTests: XCTestCase {

    private var dir: URL!

    override func setUpWithError() throws {
        dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("voice-agent-skill-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: dir)
    }

    private func parse(_ contents: String) throws -> SkillLoader.SkillDefinition? {
        let path = dir.appendingPathComponent("SKILL.md")
        try contents.write(to: path, atomically: true, encoding: .utf8)
        return SkillLoader.parse(path: path.path)
    }

    func testParsesNameDescriptionAndBody() throws {
        let skill = try parse("""
            ---
            name: desk-activity
            description: "Connects the screen to a task board"
            ---
            Look at the desktop and report what the user is doing.
            """)
        XCTAssertEqual(skill?.name, "desk-activity")
        XCTAssertEqual(skill?.description, "Connects the screen to a task board")
        XCTAssertEqual(skill?.prompt, "Look at the desktop and report what the user is doing.")
    }

    /// Both quoting styles and bare values appear in real SKILL.md files.
    func testStripsSurroundingQuotes() throws {
        let double = try parse("---\nname: \"a\"\ndescription: \"d\"\n---\nbody")
        XCTAssertEqual(double?.name, "a")
        XCTAssertEqual(double?.description, "d")

        let single = try parse("---\nname: 'a'\ndescription: 'd'\n---\nbody")
        XCTAssertEqual(single?.name, "a")

        let bare = try parse("---\nname: a\ndescription: d\n---\nbody")
        XCTAssertEqual(bare?.name, "a")
    }

    /// A skill is identified by name; without one there is nothing to register.
    func testRejectsMissingOrEmptyName() throws {
        XCTAssertNil(try parse("---\ndescription: d\n---\nbody"))
        XCTAssertNil(try parse("---\nname:\ndescription: d\n---\nbody"))
    }

    /// A description is optional — it becomes empty rather than failing the load.
    func testMissingDescriptionIsEmptyNotFatal() throws {
        let skill = try parse("---\nname: a\n---\nbody")
        XCTAssertEqual(skill?.name, "a")
        XCTAssertEqual(skill?.description, "")
    }

    func testRejectsFileWithoutFrontmatter() throws {
        XCTAssertNil(try parse("Just a markdown file with no frontmatter."))
    }

    /// An unterminated frontmatter block would otherwise swallow the whole file
    /// as YAML.
    func testRejectsUnterminatedFrontmatter() throws {
        XCTAssertNil(try parse("---\nname: a\ndescription: d\nbody with no closing marker"))
    }

    func testMissingFileIsNilNotACrash() {
        XCTAssertNil(SkillLoader.parse(path: dir.appendingPathComponent("nope.md").path))
    }

    /// The body may contain `---` (horizontal rules are common in markdown); only
    /// the *first* closing marker ends the frontmatter.
    func testBodyMayContainDashes() throws {
        let skill = try parse("""
            ---
            name: a
            description: d
            ---
            intro

            ---

            more
            """)
        XCTAssertEqual(skill?.name, "a")
        XCTAssertTrue(skill?.prompt.contains("more") ?? false)
    }

    /// The project's own skills must load — they are injected into every turn.
    func testShippedSkillsLoad() {
        let repo = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let skills = SkillLoader.loadAll(paths: ["skills"], baseDir: repo.path)
        XCTAssertFalse(skills.isEmpty, "expected the repo's skills/ to load")
        for skill in skills {
            XCTAssertFalse(skill.name.isEmpty)
            XCTAssertFalse(skill.prompt.isEmpty, "\(skill.name) has an empty body")
        }
    }
}
