// Two windows: "Background" changes its text every 3 s, "Front" stays key and changes once at 20 s.
// Usage: TwoWin [normal|minimize|minimize-all|fullscreen]
//   minimize      the Background window is minimized
//   minimize-all  both windows are minimized
//   fullscreen    the Background window goes full screen, which gives it a Space of its own, and
//                 the Front window comes back to the first Space
import AppKit

final class Delegate: NSObject, NSApplicationDelegate {
    var windows: [NSWindow] = []

    var ticks = 0

    func make(_ title: String, _ text: String, _ x: CGFloat) -> NSTextView {
        let window = NSWindow(contentRect: NSRect(x: x, y: 300, width: 360, height: 120),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = title
        let label = NSTextView(frame: NSRect(x: 20, y: 20, width: 320, height: 80))
        label.string = text
        window.contentView?.addSubview(label)
        window.makeKeyAndOrderFront(nil)
        windows.append(window)
        return label
    }

    func set(_ label: NSTextView, _ text: String) {
        label.setSelectedRange(NSRange(location: (label.string as NSString).length, length: 0))
        label.insertText("\n" + text, replacementRange: label.selectedRange())
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let background = make("Background", "message 0", 100)
        let front = make("Front", "static page", 500)
        NSApp.activate(ignoringOtherApps: true)
        windows[1].makeKeyAndOrderFront(nil)
        let mode = CommandLine.arguments.dropFirst().first ?? "normal"
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
            switch mode {
            case "minimize": self.windows[0].miniaturize(nil)
            case "minimize-all": self.windows.forEach { $0.miniaturize(nil) }
            case "fullscreen":
                self.windows[0].collectionBehavior.insert(.fullScreenPrimary)
                self.windows[0].toggleFullScreen(nil)
                DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                    self.windows[1].makeKeyAndOrderFront(nil)
                }
            default: break
            }
        }
        Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { _ in
            self.ticks += 1
            self.set(background, "message \(self.ticks)")
            if self.ticks == 7 { self.set(front, "static page, edited once") }
        }
    }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
