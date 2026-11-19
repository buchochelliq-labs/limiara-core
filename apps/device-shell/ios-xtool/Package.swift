// swift-tools-version: 6.0

// The iOS shell as an xtool SwiftPM project: an experiment in building
// and sideloading the app from Linux or Windows (WSL) without Xcode.
// See README.md. Run `./prepare.sh` first: it builds the Rust static
// library into build/rust and copies the Swift plugin packages into
// build/packages.

import PackageDescription

let package = Package(
    name: "LimiaraShell",
    platforms: [
        .iOS(.v16),
    ],
    products: [
        .library(name: "LimiaraShell", targets: ["LimiaraShell"]),
    ],
    dependencies: [
        // The shell's native plugins with a Swift half, and through them
        // Tauri's own iOS package (build/packages/.tauri/tauri-api).
        .package(path: "build/packages/device-browser"),
        .package(path: "build/packages/device-push"),
        .package(path: "build/packages/device-route-probe"),
    ],
    targets: [
        .target(
            name: "LimiaraShell",
            dependencies: [
                .product(name: "device-browser", package: "device-browser"),
                .product(name: "device-push", package: "device-push"),
                .product(name: "device-route-probe", package: "device-route-probe"),
            ],
            linkerSettings: [
                // The Rust shell (crate device-shell), built as a static
                // library for aarch64-apple-ios by prepare.sh.
                .unsafeFlags(["-L\(Context.packageDirectory)/build/rust"]),
                .linkedLibrary("device_shell"),
                .linkedLibrary("c++"),
                // A static library carries no framework list: these are
                // the frameworks the Rust crates link on iOS (prepare.sh
                // prints rustc's own list to compare with).
                .linkedFramework("Foundation"),
                .linkedFramework("CoreFoundation"),
                .linkedFramework("UIKit"),
                .linkedFramework("WebKit"),
                .linkedFramework("Security"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("QuartzCore"),
                .linkedFramework("CoreImage"),
                .linkedFramework("CoreText"),
                .linkedFramework("CoreData"),
                .linkedFramework("CoreLocation"),
                .linkedFramework("CloudKit"),
                .linkedFramework("UserNotifications"),
                .linkedFramework("Metal"),
                .linkedFramework("MetalKit"),
            ]
        ),
    ]
)
