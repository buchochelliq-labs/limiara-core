//! Hands the Swift half (`ios/`) to the app's Xcode build. Android's FCM
//! half comes with a deployment's Firebase project (ADR 0030 §5).

fn main() {
    tauri_plugin::Builder::new(&[]).ios_path("ios").build();
}
