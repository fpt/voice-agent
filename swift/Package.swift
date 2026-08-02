// swift-tools-version: 6.1
import PackageDescription

let package = Package(
    name: "voice-agent",
    platforms: [.macOS("26.0")],
    products: [
        .executable(name: "voice-agent-cli", targets: ["VoiceAgentCli"]),
    ],
    dependencies: [
        // YAML parsing
        .package(url: "https://github.com/jpsim/Yams.git", from: "5.0.0"),
    ],
    targets: [
        .target(
            name: "AgentKit",
            dependencies: ["Util", "AgentBridge", "TTS", "ScreenCapture"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "VoiceAgentCli",
            dependencies: [
                "AgentKit",
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
            dependencies: [],
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
