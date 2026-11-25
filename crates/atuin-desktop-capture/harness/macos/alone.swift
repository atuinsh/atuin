// Shows only an alert, for the given milliseconds, then exits. Usage: Alone <ms>
import AppKit
let ms = Int(CommandLine.arguments.dropFirst().first ?? "300") ?? 300
final class Delegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ note: Notification) {
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = "Copy failed"
        alert.informativeText = "No space left on device."
        let timer = Timer(timeInterval: Double(ms) / 1000, repeats: false) { _ in NSApp.abortModal() }
        RunLoop.main.add(timer, forMode: .modalPanel)
        alert.runModal()
        NSApp.terminate(nil)
    }
}
let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
