import AgentCore

/// The context gauge as the REPL prints it, or `nil` when there is nothing to
/// say.
///
/// Two kinds of nothing, both spelled as no line at all:
///
/// - **The backend measured nothing.** A zero arrives when it reported no usage
///   (a backend without `thread/tokenUsage/updated`) or no context window it
///   could vouch for — gallium sends `modelContextWindow: null` rather than the
///   guess it compacts against. That zero used to be printed as a confident
///   "[0% context]" on every app-server turn while the backend logged thousands
///   of tokens for that same turn (#18).
/// - **A real share that rounds to zero.** A fresh conversation against a 262k
///   window sits under half a percent, and truncating it to "[0% context]" is
///   the same false claim by another route — the reason the guard rounds rather
///   than truncates. Under one percent it says "<1%": something was measured,
///   and it is nearly nothing.
func contextGaugeLine(percent: Float) -> String? {
    guard percent > 0 else { return nil }
    let shown = percent < 1 ? "<1" : String(Int(percent.rounded()))
    return "\u{1B}[90m[\(shown)% context]\u{1B}[0m\n"
}

/// Print the gauge for a finished turn, when there is one to print.
func printContextUsage(_ response: AgentResponse) {
    if let line = contextGaugeLine(percent: response.contextPercent) {
        print(line)
    }
}
