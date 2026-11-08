import AuthenticationServices
import Tauri
import UIKit
import WebKit

class AuthenticateArgs: Decodable {
    let url: String
    let callbackScheme: String
}

/// Sign-in through `ASWebAuthenticationSession` (RFC 8252): the system
/// browser's sign-in is shared, the app never sees the member's
/// credentials, and the session hands back the redirect to the app's scheme.
class BrowserPlugin: Plugin, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?

    @objc public func authenticate(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(AuthenticateArgs.self)
        guard let url = URL(string: args.url), url.scheme == "https" || url.scheme == "http" else {
            invoke.reject("the sign-in address is not a web address")
            return
        }
        DispatchQueue.main.async {
            self.session?.cancel()
            let session = ASWebAuthenticationSession(
                url: url,
                callbackURLScheme: args.callbackScheme
            ) { callback, error in
                self.session = nil
                if let callback = callback {
                    invoke.resolve(["url": callback.absoluteString])
                } else if let error = error as? ASWebAuthenticationSessionError,
                          error.code == .canceledLogin {
                    invoke.reject("the sign-in was cancelled")
                } else {
                    invoke.reject(error?.localizedDescription ?? "the sign-in did not come back")
                }
            }
            session.presentationContextProvider = self
            self.session = session
            if !session.start() {
                self.session = nil
                invoke.reject("the sign-in could not start")
            }
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        let windows = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
        return windows.first { $0.isKeyWindow } ?? windows.first ?? ASPresentationAnchor()
    }
}

@_cdecl("init_plugin_device_browser")
func initPlugin() -> Plugin {
    return BrowserPlugin()
}
