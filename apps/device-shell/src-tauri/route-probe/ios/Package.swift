// swift-tools-version:5.3

import PackageDescription

let package = Package(
    name: "device-route-probe",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v13),
    ],
    products: [
        .library(
            name: "device-route-probe",
            type: .static,
            targets: ["device-route-probe"]
        ),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
    ],
    targets: [
        .target(
            name: "device-route-probe",
            dependencies: [.byName(name: "Tauri")],
            path: "Sources"
        ),
    ]
)
