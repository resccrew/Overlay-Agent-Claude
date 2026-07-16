// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "DesktopCompanion",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "DesktopCompanion",
            path: "Sources/DesktopCompanion"
        )
    ]
)
