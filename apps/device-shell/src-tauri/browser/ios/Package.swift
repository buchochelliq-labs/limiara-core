// swift-tools-version:5.3

import PackageDescription

let package = Package(
    name: "device-browser",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v13),
    ],
    products: [
        .library(
            name: "device-browser",
            type: .static,
            targets: ["device-browser"]
        ),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
    ],
    targets: [
        .target(
            name: "device-browser",
            dependencies: [.byName(name: "Tauri")],
            path: "Sources"
        ),
    ]
)
