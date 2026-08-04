import XCTest

@testable import VoiceAgentCli

final class ContextGaugeTests: XCTestCase {
    /// Zero is "nothing was measured" — no usage from the backend, or no window
    /// it could vouch for. Printing "[0% context]" there is a claim that the
    /// context is empty, which is the lie #18 removed.
    func testNothingMeasuredPrintsNoGauge() {
        XCTAssertNil(contextGaugeLine(percent: 0))
    }

    /// A real share under one percent — a fresh conversation against a 262k
    /// window — is reported as itself rather than truncated back into that same
    /// false zero.
    func testASubOnePercentShareIsNotTruncatedToZero() {
        XCTAssertEqual(contextGaugeLine(percent: 0.67), "\u{1B}[90m[<1% context]\u{1B}[0m\n")
        XCTAssertEqual(contextGaugeLine(percent: 0.04), "\u{1B}[90m[<1% context]\u{1B}[0m\n")
    }

    /// Rounded, not truncated, so the figure matches the backend's own REPL.
    func testRoundsToTheNearestPercent() {
        XCTAssertEqual(contextGaugeLine(percent: 2.18), "\u{1B}[90m[2% context]\u{1B}[0m\n")
        XCTAssertEqual(contextGaugeLine(percent: 6.7), "\u{1B}[90m[7% context]\u{1B}[0m\n")
        XCTAssertEqual(contextGaugeLine(percent: 99.6), "\u{1B}[90m[100% context]\u{1B}[0m\n")
    }

    /// A midpoint goes away from zero, which is the rule the Windows CLI is
    /// asked for explicitly — .NET rounds midpoints to even by default, and the
    /// two frontends reading the same number differently is a bug report nobody
    /// can reproduce on one machine.
    func testAMidpointRoundsAwayFromZero() {
        XCTAssertEqual(contextGaugeLine(percent: 2.5), "\u{1B}[90m[3% context]\u{1B}[0m\n")
    }
}
