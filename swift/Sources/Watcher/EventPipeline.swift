import Foundation

/// Unified event type from both watcher sources
public enum WatcherEvent: Sendable {
    case session(SessionEvent)
    case hook(HookEvent)
}


/// Debounces watcher events and dispatches summarized batches to a handler.
public actor EventPipeline {
    public typealias Handler = @Sendable (String) async -> Void

    private var buffer: [WatcherEvent] = []
    private var debounceTask: Task<Void, Never>?
    private let debounceInterval: TimeInterval
    private let handler: Handler

    public init(debounceInterval: TimeInterval = 3.0, handler: @escaping Handler) {
        self.debounceInterval = debounceInterval
        self.handler = handler
    }

    /// Feed an event into the pipeline. Resets the debounce timer.
    public func feed(_ event: WatcherEvent) {
        buffer.append(event)

        // Reset debounce timer
        debounceTask?.cancel()
        debounceTask = Task { [debounceInterval] in
            try? await Task.sleep(for: .seconds(debounceInterval))
            guard !Task.isCancelled else { return }
            await self.flush()
        }
    }

    /// Flush buffered events: summarize and call handler.
    private func flush() async {
        guard !buffer.isEmpty else { return }

        let events = buffer
        buffer.removeAll()

        guard let summary = EventSummarizer.summarize(events) else { return }
        await handler(summary)
    }

    /// Stop the pipeline and discard pending events.
    public func stop() {
        debounceTask?.cancel()
        debounceTask = nil
        buffer.removeAll()
    }
}
