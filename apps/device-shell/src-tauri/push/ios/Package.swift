// swift-tools-version:5.3

import PackageDescription

let package = Package(
    name: "device-push",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v13),
    ],
    products: [
        .library(
            name: "device-push",
            type: .static,
            targets: ["device-push"]
        ),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
    ],
    targets: [
        .target(
            name: "device-push",
            dependencies: [.byName(name: "Tauri")],
            path: "Sources"
        ),
    ]
)
