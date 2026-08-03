// swift-tools-version: 6.1
import PackageDescription

let package = Package(
    name: "voice-agent",
    platforms: [.macOS("26.0"), .iOS("26.0")],
    products: [
        .executable(name: "voice-agent-cli", targets: ["VoiceAgentCli"]),
        // Library products so another app — an iOS one, say — can depend on
        // exactly the platform-neutral pieces without pulling in the Rust
        // cdylib or the macOS screen APIs.
        .library(name: "AgentCore", targets: ["AgentCore"]),
        .library(name: "FoundationModelsKit", targets: ["FoundationModelsKit"]),
        .library(name: "VoiceAgentAudio", targets: ["Audio"]),
        .library(name: "VoiceAgentTTS", targets: ["TTS"]),
        .library(name: "VoiceAgentUtil", targets: ["Util"]),
        // macOS only in practice: WindowManager uses AppKit and ScreenCaptureKit.
        .library(name: "ScreenCapture", targets: ["ScreenCapture"]),
    ],
    dependencies: [
        // YAML parsing
        .package(url: "https://github.com/jpsim/Yams.git", from: "5.0.0"),
    ],
    targets: [
        // Platform-neutral: domain types, the backend protocol, the perception
        // protocol, and the concurrency/storage helpers. No Rust, no AppKit.
        .target(
            name: "AgentCore",
            dependencies: [],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        // The on-device backend. Runs anywhere Foundation Models does, and knows
        // nothing about screens — perception is injected.
        .target(
            name: "FoundationModelsKit",
            dependencies: ["AgentCore", "Util"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        // The app-server backend and the shared session lifecycle. This is where
        // the Rust cdylib dependency lives, and it stops here.
        .target(
            name: "AgentKit",
            dependencies: [
                "AgentCore", "FoundationModelsKit", "Util", "AgentBridge", "TTS", "ScreenCapture",
            ],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "VoiceAgentCli",
            dependencies: [
                "AgentKit",
                "AgentCore",
                "Util",
                "AgentBridge",
                "CEditline",
                "Audio",
                "ScreenCapture",
            ],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "Util",
            dependencies: ["Yams"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "TTS",
            dependencies: ["Util"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "Audio",
            dependencies: [
                "Util",
            ],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "ScreenCapture",
            dependencies: ["AgentCore"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .systemLibrary(
            name: "CEditline",
            path: "Sources/CEditline"
        ),
        .systemLibrary(
            name: "AgentBridgeFFI",
            path: "Sources/AgentBridgeFFI",
            pkgConfig: nil,
            providers: nil
        ),
        .testTarget(
            name: "UtilTests",
            dependencies: ["Util"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "ScreenCaptureTests",
            dependencies: ["ScreenCapture"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "VoiceAgentCliTests",
            dependencies: ["VoiceAgentCli"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "FoundationModelsKitTests",
            dependencies: ["FoundationModelsKit", "AgentCore"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "TTSTests",
            dependencies: ["TTS"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "AgentCoreTests",
            dependencies: ["AgentCore"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "AgentKitTests",
            dependencies: ["AgentKit", "AgentCore", "FoundationModelsKit"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "AgentBridge",
            dependencies: ["AgentBridgeFFI"],
            swiftSettings: [
                .swiftLanguageMode(.v5),
            ],
            linkerSettings: [
                .unsafeFlags([
                    "-L../crates/target/release",
                    "-lvoice_agent_core",
                ])
            ]
        ),
    ]
)
