import XCTest

@testable import TTS

/// What reaches the speaker. The printed reply keeps everything; speech drops
/// what is meaningless to hear.
final class SanitizeForSpeechTests: XCTestCase {

    private func spoken(_ s: String) -> String { TextToSpeech.sanitizeForSpeech(s) }

    func testOrdinaryTextIsUntouched() {
        let text = "You're working on goal.rs in voice-agent."
        XCTAssertEqual(spoken(text), text)
    }

    func testReasoningBlocksAreStillStripped() {
        XCTAssertEqual(spoken("<think>hmm</think>Hello."), "Hello.")
        XCTAssertEqual(spoken("Hello.<think>dangling"), "Hello.")
    }

    /// The reported failure: a model asked for a screenshot replied with a
    /// base64 data URI and this was read out character by character.
    func testBase64DataUriIsNotSpoken() {
        let reply = """
            Here is a screenshot of your desktop:

            ![Desktop Screenshot](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACJv8lCAAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwIARUj97f1tAAAAASUVORK5CYII=)

            Let me know if you need further assistance!
            """
        let out = spoken(reply)
        XCTAssertFalse(out.contains("iVBORw0KGgo"), "base64 must not be spoken: \(out)")
        XCTAssertFalse(out.contains("data:image"), out)
        XCTAssertTrue(out.contains("Here is a screenshot"), "the sentence should survive: \(out)")
        XCTAssertTrue(out.contains("Desktop Screenshot"), "alt text is worth keeping: \(out)")
    }

    func testFencedCodeIsSummarised() {
        let out = spoken("Try this:\n```swift\nlet x = 1\nprint(x)\n```\nThat's it.")
        XCTAssertFalse(out.contains("print(x)"), out)
        XCTAssertTrue(out.contains("code block"), out)
        XCTAssertTrue(out.contains("That's it."), out)
    }

    func testUnterminatedCodeFenceIsAlsoHandled() {
        let out = spoken("Here:\n```swift\nlet x = 1")
        XCTAssertFalse(out.contains("let x = 1"), out)
    }

    func testMarkdownLinkKeepsItsLabel() {
        let out = spoken("See [the pull request](https://github.com/fpt/voice-agent/pull/16).")
        XCTAssertTrue(out.contains("the pull request"), out)
        XCTAssertFalse(out.contains("github.com"), out)
    }

    func testBareUrlIsNotReadOut() {
        let out = spoken("Open https://developer.apple.com/documentation/foundationmodels now.")
        XCTAssertFalse(out.contains("developer.apple.com"), out)
        XCTAssertTrue(out.contains("Open"), out)
    }

    func testLongOpaqueTokenIsElided() {
        let out = spoken("The digest is 3f786850e387550fdab836ed7e6dc881de23001b3f786850e387550f.")
        XCTAssertFalse(out.contains("3f786850e387550fdab836"), out)
        XCTAssertTrue(out.contains("digest"), out)
    }

    /// Guard against over-eager stripping: normal long words must survive.
    func testOrdinaryLongWordsSurvive() {
        let text = "Internationalization and antidisestablishmentarianism are long words."
        XCTAssertEqual(spoken(text), text)
    }
}
