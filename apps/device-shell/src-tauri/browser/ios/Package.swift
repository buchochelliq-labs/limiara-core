// swift-tools-version:5.3

import PackageDescription

let package = Package(
    name: "tauri-plugin-device-browser",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v13),
    ],
    products: [
        .library(
            name: "tauri-plugin-device-browser",
            type: .static,
            targets: ["tauri-plugin-device-browser"]
        ),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
    ],
    targets: [
        .target(
            name: "tauri-plugin-device-browser",
            dependencies: [.byName(name: "Tauri")],
            path: "Sources"
        ),
    ]
)
