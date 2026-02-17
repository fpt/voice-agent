// swift-tools-version: 6.1
import PackageDescription

let package = Package(
    name: "VoiceAgent",
    platforms: [.macOS("26.0")],
    products: [
        .executable(name: "voice-agent", targets: ["VoiceAgentCLI"]),
    ],
    dependencies: [
        // YAML parsing
        .package(url: "https://github.com/jpsim/Yams.git", from: "5.0.0"),
    ],
    targets: [
        .executableTarget(
            name: "VoiceAgentCLI",
            dependencies: [
                "Util",
                "AgentBridge",
                "CEditline",
                "TTS",
                "Audio",
                "Watcher",
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
            name: "Watcher",
            dependencies: ["Util"],
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
                    "-lagent_core",
                ])
            ]
        ),
    ]
)
