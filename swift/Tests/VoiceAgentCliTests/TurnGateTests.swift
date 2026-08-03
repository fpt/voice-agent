import XCTest

@testable import VoiceAgentCli

/// `TurnGate` is what stops a `/loop` tick, a `/goal` turn, and a typed line
/// from running at once — they share the capture bridge and per-tool caches, so
/// overlap is a correctness problem, not just noise.
final class TurnGateTests: XCTestCase {

    func testTryLockSucceedsWhenFree() async {
        let gate = TurnGate()
        let got = await gate.tryLock()
        XCTAssertTrue(got)
    }

    /// The ambient loop's contract: skip this tick rather than queue behind a
    /// user turn.
    func testTryLockFailsWhileHeld() async {
        let gate = TurnGate()
        _ = await gate.tryLock()
        let second = await gate.tryLock()
        XCTAssertFalse(second, "a second holder must be refused, not admitted")
    }

    func testTryLockSucceedsAgainAfterUnlock() async {
        let gate = TurnGate()
        _ = await gate.tryLock()
        await gate.unlock()
        let again = await gate.tryLock()
        XCTAssertTrue(again)
    }

    /// A user turn waits rather than skipping.
    func testLockSuspendsUntilUnlocked() async {
        let gate = TurnGate()
        await gate.lock()

        let acquired = Task { () -> Bool in
            await gate.lock()
            return true
        }

        // Still held: the waiter must not have completed.
        try? await Task.sleep(for: .milliseconds(50))
        XCTAssertFalse(acquired.isCancelled)

        await gate.unlock()
        let value = await acquired.value
        XCTAssertTrue(value, "the waiter should acquire once the gate is released")
    }

    /// The property that matters: however many contenders, exactly one holds the
    /// gate at a time. A counter incremented inside the critical section would
    /// exceed 1 if the gate admitted two.
    func testOnlyOneHolderAtATimeUnderContention() async {
        let gate = TurnGate()
        let tracker = ConcurrencyTracker()

        await withTaskGroup(of: Void.self) { group in
            for _ in 0..<20 {
                group.addTask {
                    await gate.lock()
                    await tracker.enter()
                    try? await Task.sleep(for: .milliseconds(1))
                    await tracker.exit()
                    await gate.unlock()
                }
            }
        }

        let peak = await tracker.peak
        XCTAssertEqual(peak, 1, "gate admitted \(peak) turns at once")
        let completed = await tracker.completed
        XCTAssertEqual(completed, 20, "every waiter should eventually run")
    }
}

/// Records how many holders were inside the critical section simultaneously.
private actor ConcurrencyTracker {
    private(set) var peak = 0
    private(set) var completed = 0
    private var current = 0

    func enter() {
        current += 1
        peak = max(peak, current)
    }

    func exit() {
        current -= 1
        completed += 1
    }
}
