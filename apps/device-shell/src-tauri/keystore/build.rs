//! Hands the Kotlin half (`android/`) to the app's Gradle build.

fn main() {
    tauri_plugin::Builder::new(&[])
        .android_path("android")
        .build();
}
