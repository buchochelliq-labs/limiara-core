// The app's entry point. Tauri's mobile entry point (`app::run`, marked
// `#[tauri::mobile_entry_point]`) exports `start_app` from the Rust
// static library; it starts UIKit itself and never returns. Tauri's
// generated Xcode project does the same from `main.mm`.

@_silgen_name("start_app")
private func startApp()

@main
enum LimiaraShell {
    static func main() {
        startApp()
    }
}
