// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "FMBridge",
    platforms: [
        // FoundationModels ships in the macOS 26 SDK ("Tahoe").
        .macOS("26.0")
    ],
    products: [
        .executable(name: "FMBridge", targets: ["FMBridge"])
    ],
    targets: [
        .executableTarget(
            name: "FMBridge",
            path: "Sources/FMBridge",
            swiftSettings: [
                .swiftLanguageMode(.v6)
            ]
        )
    ]
)
