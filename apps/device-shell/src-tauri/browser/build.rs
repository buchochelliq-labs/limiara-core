//! Hands the Kotlin (`android/`) and Swift (`ios/`) halves to the app's
//! native builds.

fn main() {
    tauri_plugin::Builder::new(&[])
        .android_path("android")
        .ios_path("ios")
        .build();
}
