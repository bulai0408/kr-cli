import SwiftUI

@main
struct KrSampleApp: App {
    var body: some Scene {
        WindowGroup {
            Text("Hello, kr")
                .accessibilityIdentifier("Root.text.greeting")
        }
    }
}

enum Greeting {
    static func text(for name: String) -> String { "Hello, \(name)" }
}
