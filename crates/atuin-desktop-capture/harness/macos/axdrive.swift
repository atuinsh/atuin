// Drives applications through the AX API, for tests of capture on macOS.
//
// The driver needs the Accessibility permission, like capture. It re-spawns itself with the
// responsibility of its parent disclaimed, so that macOS checks the driver's own permission.
// Otherwise macOS would check the permission of the SSH session or the terminal that started the
// driver.
//
// Usage: axdrive <command> [arguments]
//
//   trusted                       prints whether the driver has the permission
//   apps                          lists the regular applications, with their windows
//   windows                       lists the on-screen windows, front to back, with their layers
//   tree <app> [depth]            prints the elements of the application's windows
//   press <app> <name> [role]     presses the first element whose title or description is name
//   menu <app> <item>...          presses the menu bar item at a path, such as File Save
//   key <app> <combo>...          posts keys to the application, such as cmd+s, return, or tab
//   hotkey <combo>...             posts keys to the system, such as cmd+space for Spotlight
//   type <app> <text>             types text into the application
//   set <app> <name> <text>       sets the value of the first text field that is named name,
//                                 or of the focused element when name is "-"
//   menus <app> [depth]           prints the menu bar of the application
//   watch <app> [seconds]         prints the AX notifications that the application posts
//   flag <app> <attribute> [false]  sets a boolean attribute of the application element
//   alert <title> <message> [timeout]  shows an alert through UserNotificationCenter
//   attrs <app> <name>            prints every attribute of the first element named name, or
//                                 of the application element when name is "-"
//   activate <app>                brings the application to the front
//   wait <app> <text> [seconds]   waits until the application shows the text
//
// An application is named by its name, its bundle id, or its pid.
import AppKit
import ApplicationServices
import Foundation

@_silgen_name("responsibility_spawnattrs_setdisclaim")
func responsibility_spawnattrs_setdisclaim(_ attrs: UnsafeMutablePointer<posix_spawnattr_t?>, _ disclaim: Int32) -> Int32

func respawnDisclaimed() {
    if ProcessInfo.processInfo.environment["AXDRIVE_CHILD"] != nil { return }
    var attrs: posix_spawnattr_t?
    posix_spawnattr_init(&attrs)
    _ = responsibility_spawnattrs_setdisclaim(&attrs, 1)
    setenv("AXDRIVE_CHILD", "1", 1)
    guard let resolved = Bundle.main.executablePath else { return }
    var argv: [UnsafeMutablePointer<CChar>?] = CommandLine.arguments.map { strdup($0) }
    argv.append(nil)
    var pid: pid_t = 0
    let status = posix_spawn(&pid, resolved, nil, &attrs, argv, environ)
    if status != 0 {
        fputs("spawn failed: \(status)\n", stderr)
        return
    }
    var wait: Int32 = 0
    waitpid(pid, &wait, 0)
    exit((wait >> 8) & 0xff)
}

func fail(_ message: String) -> Never {
    fputs("axdrive: \(message)\n", stderr)
    exit(1)
}

func attr(_ element: AXUIElement, _ name: String) -> AnyObject? {
    var value: AnyObject?
    guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
    return value
}

func str(_ element: AXUIElement, _ name: String) -> String? {
    attr(element, name) as? String
}

func children(_ element: AXUIElement, _ name: String = "AXChildren") -> [AXUIElement] {
    (attr(element, name) as? [AXUIElement]) ?? []
}

func runningApp(_ name: String) -> NSRunningApplication? {
    let apps = NSWorkspace.shared.runningApplications
    if let pid = Int32(name) { return NSRunningApplication(processIdentifier: pid) }
    return apps.first { $0.localizedName?.caseInsensitiveCompare(name) == .orderedSame }
        ?? apps.first { $0.bundleIdentifier?.caseInsensitiveCompare(name) == .orderedSame }
        ?? apps.first { ($0.bundleURL?.deletingPathExtension().lastPathComponent ?? "").caseInsensitiveCompare(name) == .orderedSame }
}

func appElement(_ name: String) -> (AXUIElement, pid_t) {
    guard let app = runningApp(name) else { fail("no running application named \(name)") }
    AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), 3)
    return (AXUIElementCreateApplication(app.processIdentifier), app.processIdentifier)
}

func label(_ element: AXUIElement) -> String {
    let role = str(element, "AXRole") ?? "?"
    let subrole = str(element, "AXSubrole").map { "/\($0)" } ?? ""
    var parts = ["\(role)\(subrole)"]
    if let title = str(element, "AXTitle"), !title.isEmpty { parts.append("title=\(quote(title))") }
    if let desc = str(element, "AXDescription"), !desc.isEmpty { parts.append("desc=\(quote(desc))") }
    if let id = str(element, "AXIdentifier"), !id.isEmpty { parts.append("id=\(quote(id))") }
    if let value = attr(element, "AXValue") {
        if let s = value as? String { if !s.isEmpty { parts.append("value=\(quote(s))") } }
        else if let n = value as? NSNumber { parts.append("value=\(n)") }
    }
    return parts.joined(separator: " ")
}

func quote(_ s: String) -> String {
    let flat = s.replacingOccurrences(of: "\n", with: "⏎")
    let cut = flat.count > 160 ? String(flat.prefix(160)) + "…" : flat
    return "\"\(cut)\""
}

func windowsOf(_ app: AXUIElement) -> [AXUIElement] {
    var windows = children(app, "AXWindows")
    for child in children(app) where str(child, "AXRole") != "AXMenuBar" && !windows.contains(where: { CFEqual($0, child) }) {
        windows.append(child)
    }
    return windows
}

func walk(_ element: AXUIElement, depth: Int = 0, maxDepth: Int, visit: (AXUIElement, Int) -> Bool) -> Bool {
    if !visit(element, depth) { return false }
    if depth >= maxDepth { return true }
    for child in children(element) {
        if !walk(child, depth: depth + 1, maxDepth: maxDepth, visit: visit) { return false }
    }
    return true
}

func find(_ app: AXUIElement, includeMenus: Bool = false, _ matches: (AXUIElement) -> Bool) -> AXUIElement? {
    var found: AXUIElement?
    var roots = windowsOf(app)
    if includeMenus, let bar = attr(app, "AXMenuBar") { roots.append(bar as! AXUIElement) }
    for root in roots {
        _ = walk(root, maxDepth: 60) { element, _ in
            if matches(element) { found = element; return false }
            return true
        }
        if found != nil { break }
    }
    return found
}

func named(_ element: AXUIElement, _ name: String) -> Bool {
    for key in ["AXTitle", "AXDescription", "AXIdentifier"] {
        if let value = str(element, key), value.caseInsensitiveCompare(name) == .orderedSame { return true }
    }
    if str(element, "AXRole") == "AXStaticText" || str(element, "AXRole") == "AXButton",
       let value = attr(element, "AXValue") as? String, value.caseInsensitiveCompare(name) == .orderedSame {
        return true
    }
    return false
}

let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12,
    "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22,
    "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37,
    "j": 38, "k": 40, "n": 45, "m": 46, "return": 36, "tab": 48, "space": 49, "delete": 51,
    "escape": 53, "left": 123, "right": 124, "down": 125, "up": 126, "home": 115, "end": 119,
    "pageup": 116, "pagedown": 121, "forwarddelete": 117, ",": 43, ".": 47, "/": 44, "[": 33, "]": 30,
    "=": 24, "-": 27, "`": 50, "l_": 37,
]

func postKey(_ pid: pid_t?, _ combo: String) {
    var flags: CGEventFlags = []
    var key = combo.lowercased()
    let parts = key.split(separator: "+").map(String.init)
    if parts.count > 1 {
        key = parts.last!
        for modifier in parts.dropLast() {
            switch modifier {
            case "cmd": flags.insert(.maskCommand)
            case "shift": flags.insert(.maskShift)
            case "alt", "opt": flags.insert(.maskAlternate)
            case "ctrl": flags.insert(.maskControl)
            default: fail("unknown modifier \(modifier)")
            }
        }
    }
    guard let code = keyCodes[key] else { fail("unknown key \(key)") }
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else { fail("no event") }
        event.flags = flags
        if let pid { event.postToPid(pid) } else { event.post(tap: .cghidEventTap) }
        usleep(15_000)
    }
    usleep(60_000)
}

func typeText(_ pid: pid_t, _ text: String) {
    for unit in text.utf16 {
        if unit == 10 { postKey(pid, "return"); continue }
        for down in [true, false] {
            guard let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down) else { fail("no event") }
            var u = unit
            event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &u)
            event.postToPid(pid)
        }
        usleep(12_000)
    }
}

func textOf(_ app: AXUIElement) -> String {
    var out = ""
    for root in windowsOf(app) {
        _ = walk(root, maxDepth: 80) { element, _ in
            for key in ["AXTitle", "AXDescription"] { if let s = str(element, key) { out += s + "\n" } }
            if let s = attr(element, "AXValue") as? String { out += s + "\n" }
            return true
        }
    }
    return out
}

respawnDisclaimed()
var args = Array(CommandLine.arguments.dropFirst())
guard let command = args.first else { fail("usage: axdrive <command> [arguments]") }
args.removeFirst()
func arg(_ index: Int) -> String {
    guard index < args.count else { fail("\(command) needs more arguments") }
    return args[index]
}

switch command {
case "trusted":
    print(AXIsProcessTrusted())
case "apps":
    for app in NSWorkspace.shared.runningApplications where app.activationPolicy == .regular {
        let element = AXUIElementCreateApplication(app.processIdentifier)
        print("\(app.processIdentifier) \(app.localizedName ?? "?") \(app.bundleIdentifier ?? "")\(app.isActive ? " (active)" : "")")
        for window in windowsOf(element) { print("    \(label(window))") }
    }
case "windows":
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    for window in list {
        let layer = window[kCGWindowLayer as String] as? Int ?? -1
        let owner = window[kCGWindowOwnerName as String] as? String ?? "?"
        let pid = window[kCGWindowOwnerPID as String] as? Int ?? -1
        let bounds = window[kCGWindowBounds as String] as? [String: Any] ?? [:]
        print("layer \(layer) \(owner) (\(pid)) \(bounds["Width"] ?? "?")x\(bounds["Height"] ?? "?")")
    }
case "tree":
    let (app, _) = appElement(arg(0))
    let depth = args.count > 1 ? Int(args[1]) ?? 12 : 12
    for window in windowsOf(app) {
        _ = walk(window, maxDepth: depth) { element, level in
            print(String(repeating: "  ", count: level) + label(element))
            return true
        }
    }
case "press":
    let (app, _) = appElement(arg(0))
    let name = arg(1)
    let role = args.count > 2 ? args[2] : nil
    guard let element = find(app, includeMenus: true, { named($0, name) && (role == nil || str($0, "AXRole") == role) }) else {
        fail("no element named \(name)")
    }
    let error = AXUIElementPerformAction(element, kAXPressAction as CFString)
    if error != .success { fail("press failed: \(error.rawValue)") }
    print("pressed \(label(element))")
case "menu":
    let (app, _) = appElement(arg(0))
    guard var current = attr(app, "AXMenuBar").map({ $0 as! AXUIElement }) else { fail("no menu bar") }
    for (index, item) in args.dropFirst().enumerated() {
        var match: AXUIElement?
        _ = walk(current, maxDepth: 3) { element, _ in
            let role = str(element, "AXRole")
            if (role == "AXMenuBarItem" || role == "AXMenuItem") && named(element, item) { match = element; return false }
            return true
        }
        guard let found = match else { fail("no menu item \(item)") }
        if index == args.count - 2 {
            let error = AXUIElementPerformAction(found, kAXPressAction as CFString)
            if error != .success { fail("press failed: \(error.rawValue)") }
        }
        current = found
    }
    print("pressed \(args.dropFirst().joined(separator: " > "))")
case "key":
    let (_, pid) = appElement(arg(0))
    for combo in args.dropFirst() { postKey(pid, combo) }
case "hotkey":
    // Keys that go to the whole system, such as the shortcut of Spotlight.
    for combo in args { postKey(nil, combo) }
case "type":
    let (_, pid) = appElement(arg(0))
    typeText(pid, arg(1))
case "set":
    let (app, _) = appElement(arg(0))
    let name = arg(1)
    let element: AXUIElement?
    if name == "-" {
        element = attr(app, "AXFocusedUIElement").map { $0 as! AXUIElement }
    } else {
        element = find(app) { named($0, name) && ["AXTextField", "AXTextArea", "AXComboBox"].contains(str($0, "AXRole") ?? "") }
    }
    guard let element else { fail("no text field named \(name)") }
    let error = AXUIElementSetAttributeValue(element, "AXValue" as CFString, arg(2) as CFString)
    if error != .success { fail("set failed: \(error.rawValue)") }
case "menus":
    let (app, _) = appElement(arg(0))
    guard let bar = attr(app, "AXMenuBar").map({ $0 as! AXUIElement }) else { fail("no menu bar") }
    _ = walk(bar, maxDepth: args.count > 1 ? Int(args[1]) ?? 3 : 3) { element, level in
        print(String(repeating: "  ", count: level) + label(element))
        return true
    }
case "watch":
    let (app, pid) = appElement(arg(0))
    let seconds = args.count > 1 ? Double(args[1]) ?? 10 : 10
    var observer: AXObserver?
    let callback: AXObserverCallback = { _, element, notification, _ in
        print("\(String(format: "%.2f", Date().timeIntervalSince1970.truncatingRemainder(dividingBy: 100))) \(notification) \(label(element))")
        fflush(stdout)
    }
    guard AXObserverCreate(pid, callback, &observer) == .success, let observer else { fail("no observer") }
    let names = ["AXWindowCreated", "AXSheetCreated", "AXDrawerCreated", "AXUIElementDestroyed", "AXCreated",
                 "AXValueChanged", "AXTitleChanged", "AXFocusedUIElementChanged", "AXFocusedWindowChanged",
                 "AXMainWindowChanged", "AXLayoutChanged", "AXLoadComplete", "AXSelectedChildrenChanged",
                 "AXRowCountChanged", "AXAnnouncementRequested", "AXElementBusyChanged", "AXMenuOpened",
                 "AXSelectedTextChanged", "AXLiveRegionChanged", "AXLiveRegionCreated", "AXExpandedChanged",
                 "AXRowExpanded", "AXRowCollapsed", "AXResized", "AXMoved"]
    for name in names { AXObserverAddNotification(observer, app, name as CFString, nil) }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), AXObserverGetRunLoopSource(observer), .defaultMode)
    CFRunLoopRunInMode(.defaultMode, seconds, false)
case "flag":
    let (app, _) = appElement(arg(0))
    let value: CFBoolean = (args.count > 2 && args[2] == "false") ? kCFBooleanFalse : kCFBooleanTrue
    let error = AXUIElementSetAttributeValue(app, arg(1) as CFString, value)
    if error != .success { fail("setting \(arg(1)) failed: \(error.rawValue)") }
case "alert":
    // UserNotificationCenter shows the alert, as it shows the system's permission prompts.
    var response: CFOptionFlags = 0
    CFUserNotificationDisplayAlert(args.count > 2 ? Double(args[2]) ?? 0 : 0, kCFUserNotificationCautionAlertLevel, nil, nil, nil,
                                   arg(0) as CFString, arg(1) as CFString, "OK" as CFString, "Cancel" as CFString, nil, &response)
    print(response)
case "attrs":
    let (app, _) = appElement(arg(0))
    let name = arg(1)
    let found = name == "-" ? app : find(app, includeMenus: true, { named($0, name) })
    guard let element = found else { fail("no element named \(name)") }
    var names: CFArray?
    AXUIElementCopyAttributeNames(element, &names)
    for key in (names as? [String]) ?? [] where !["AXChildren", "AXParent", "AXWindow", "AXTopLevelUIElement"].contains(key) {
        let value = attr(element, key)
        var text: String
        if let value, CFGetTypeID(value) == AXUIElementGetTypeID() { text = "<\(label(value as! AXUIElement))>" }
        else if let list = value as? [AnyObject], let first = list.first, CFGetTypeID(first) == AXUIElementGetTypeID() {
            text = "[" + list.map { label($0 as! AXUIElement) }.joined(separator: ", ") + "]"
        } else { text = value.map { "\($0)" } ?? "(none)" }
        print("\(key) = \(text.replacingOccurrences(of: "\n", with: " ").prefix(200))")
    }
    var actions: CFArray?
    AXUIElementCopyActionNames(element, &actions)
    print("actions: \((actions as? [String]) ?? [])")
case "activate":
    guard let app = runningApp(arg(0)) else { fail("no running application named \(arg(0))") }
    app.activate(options: [.activateAllWindows])
    usleep(300_000)
    print(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?")
case "wait":
    let (app, _) = appElement(arg(0))
    let deadline = Date().addingTimeInterval(args.count > 2 ? Double(args[2]) ?? 10 : 10)
    while Date() < deadline {
        if textOf(app).contains(arg(1)) { print("found"); exit(0) }
        usleep(200_000)
    }
    fail("\(arg(1)) did not appear")
default:
    fail("unknown command \(command)")
}
