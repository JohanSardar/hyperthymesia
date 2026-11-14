// Hyperthymesia setup guide. Runs as a child of Hyperthymesia.app so macOS
// attributes every privacy request and check to the app. It shows one docked
// panel beside the System Settings pane or browser window the person has to act
// in, moves on when the real state changes, and records each step's outcome in
// <support>/setup/state.json for `hx-server setup` to report.
//
// It never clicks, approves or types anything into a system prompt.
import AppKit
import ApplicationServices
import CoreGraphics

// MARK: Files shared with `hx-server setup`

struct Request: Codable {
    let run: String
    let steps: [String]
    let signin_title: String?
}
struct SigninStatus: Codable {
    let run: String
    let url: String?
    let state: String  // waiting | done | failed
    let error: String?
}
struct StepState: Codable {
    var id: String
    var state: String
    var detail: String?
    var changed_at: String
}
struct RunState: Codable {
    var run: String
    var status: String  // running | finished | interrupted
    var steps: [StepState]
    var updated_at: String
}

let support: URL = {
    if let dir = ProcessInfo.processInfo.environment["HX_SETUP_DIR"] { return URL(fileURLWithPath: dir) }
    return FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/Hyperthymesia/setup")
}()
let requestURL = support.appendingPathComponent("request.json")
let stateURL = support.appendingPathComponent("state.json")
let signinURL = support.appendingPathComponent("signin.json")

func now() -> String { ISO8601DateFormatter().string(from: Date()) }
func readJSON<T: Decodable>(_ url: URL, _ type: T.Type) -> T? {
    guard let data = try? Data(contentsOf: url) else { return nil }
    return try? JSONDecoder().decode(type, from: data)
}
func writeAtomically<T: Encodable>(_ value: T, to url: URL) {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    guard let data = try? encoder.encode(value) else { return }
    let tmp = url.deletingLastPathComponent().appendingPathComponent(".\(url.lastPathComponent).\(getpid())")
    FileManager.default.createFile(atPath: tmp.path, contents: data, attributes: [.posixPermissions: 0o600])
    _ = rename(tmp.path, url.path)
}

// One guide at a time, also across a relaunched app.
let lockFD = open(support.appendingPathComponent("guide.lock").path, O_CREAT | O_RDWR, 0o600)
if lockFD < 0 || flock(lockFD, LOCK_EX | LOCK_NB) != 0 { exit(0) }

guard let request = readJSON(requestURL, Request.self) else {
    FileHandle.standardError.write(Data("setup-guide: no request\n".utf8))
    exit(2)
}

// MARK: Real checks

struct Permissions: Decodable { let accessibility: Bool; let screen: Bool; let input: Bool }
struct AgentStatus: Decodable { let permissions: Permissions }

/// A fresh child process reads the current grants; long-lived processes can keep
/// a cached Screen Recording answer until they restart.
/// Asks a fresh `hx-agent` process, because macOS caches some answers per process.
func probePermissions() -> Permissions {
    let agent = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
        .appendingPathComponent("hx-agent")
    let task = Process()
    task.executableURL = agent
    task.arguments = ["status"]
    let pipe = Pipe()
    task.standardOutput = pipe
    task.standardError = FileHandle.nullDevice
    if (try? task.run()) != nil {
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        task.waitUntilExit()
        if let status = try? JSONDecoder().decode(AgentStatus.self, from: data) { return status.permissions }
    }
    return Permissions(accessibility: AXIsProcessTrusted(), screen: CGPreflightScreenCaptureAccess(),
                       input: CGPreflightListenEventAccess())
}

/// Keeps the slow check off the main thread so the panel never stalls.
final class PermissionProbe {
    private(set) var latest = probePermissions()
    private var running = false
    private let queue = DispatchQueue(label: "permissions")
    func refresh() {
        guard !running else { return }
        running = true
        queue.async {
            let p = probePermissions()
            DispatchQueue.main.async { self.latest = p; self.running = false }
        }
    }
}
let permissionProbe = PermissionProbe()
/// The latest background result; starts the next check.
func currentPermissions() -> Permissions {
    permissionProbe.refresh()
    return permissionProbe.latest
}

/// From macOS 26, Accessibility also allows listening to input, and Input
/// Monitoring cannot be granted on its own while Accessibility is off.
let inputFollowsAccessibility = ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 26

func granted(_ id: String, _ p: Permissions) -> Bool {
    switch id {
    case "accessibility": return p.accessibility
    case "input": return p.input
    case "screen": return p.screen
    default: return false
    }
}

struct WindowInfo { let owner: String; let frame: CGRect }
func windows() -> [WindowInfo] {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] else { return [] }
    return list.compactMap { w in
        guard let owner = w[kCGWindowOwnerName as String] as? String,
              let layer = w[kCGWindowLayer as String] as? Int,
              let b = w[kCGWindowBounds as String] as? [String: CGFloat],
              let x = b["X"], let y = b["Y"], let width = b["Width"], let height = b["Height"],
              width > 120, height > 80 else { return nil }
        // System prompts float above normal windows; keep them for prompt detection.
        if layer != 0 && !systemOwners.contains(owner) { return nil }
        return WindowInfo(owner: owner, frame: CGRect(x: x, y: y, width: width, height: height))
    }
}
/// The processes that draw macOS's password and Touch ID confirmation.
let promptOwners: Set<String> = ["SecurityAgent", "coreautha", "LocalAuthenticationRemoteService",
                                 "LocalAuthenticationUIService"]
/// Floating system windows the guide reacts to.
let systemOwners: Set<String> = promptOwners.union(["UserNotificationCenter", "universalAccessAuthWarn"])
/// macOS 26 draws the password sheet as a second, small System Settings window.
func authPromptVisible() -> Bool {
    let all = windows()
    if all.contains(where: { promptOwners.contains($0.owner) }) { return true }
    let settings = all.filter { $0.owner == "System Settings" }
    return settings.count > 1 && settings.contains { $0.frame.width <= 420 && $0.frame.height <= 600 && $0.frame.height >= 120 }
}
/// A small second System Settings window: a password or "Quit & Reopen" sheet.
func settingsSheetVisible() -> Bool {
    let settings = windows().filter { $0.owner == "System Settings" }
    return settings.count > 1 && settings.contains { $0.frame.width <= 420 && $0.frame.height <= 600 && $0.frame.height >= 120 }
}
/// macOS's "would like to record this computer's screen" alert.
func alertVisible() -> Bool { windows().contains { $0.owner == "universalAccessAuthWarn" } }
/// macOS's one-time "requesting to bypass the private window picker" check.
func capturePromptVisible() -> Bool { windows().contains { $0.owner == "UserNotificationCenter" } }

// MARK: Copy

struct Copy { let title: String; let action: String; let why: String; let done: String; let symbol: String }
let signinTitle = request.signin_title ?? "your model provider"
func stepCopy(_ id: String) -> Copy {
    switch id {
    case "accessibility":
        return Copy(title: "Accessibility", action: "Switch on Hyperthymesia.",
                    why: "So it can see which app and window you’re working in.",
                    done: "Accessibility is on", symbol: "accessibility")
    case "input":
        return Copy(title: "Input Monitoring", action: "Switch on Hyperthymesia.",
                    why: "So it can tell when you’re active. It counts keys, never which ones.",
                    done: "Input Monitoring is on", symbol: "keyboard")
    case "screen":
        return Copy(title: "Screen Recording", action: "Switch on Hyperthymesia.",
                    why: "So it can read text from the window you’re in. Screen images are never saved.",
                    done: "Screen Recording is on", symbol: "record.circle")
    default:
        return Copy(title: "Sign in to \(signinTitle)", action: "Sign in on the page that just opened.",
                    why: "So it can write your day pages from what it recorded.",
                    done: "Signed in", symbol: "person.crop.circle")
    }
}
let pane: [String: String] = [
    "accessibility": "Privacy_Accessibility",
    "input": "Privacy_ListenEvent",
    "screen": "Privacy_ScreenCapture",
]

// MARK: Panel

/// Never takes the keyboard: the person types their password into macOS's
/// prompt right next to it, and every keystroke must reach that prompt.
final class Panel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

final class Guide: NSObject {
    var state: RunState
    let panel: Panel
    let icon = NSImageView()
    let counter = NSTextField(labelWithString: "")
    let title = NSTextField(labelWithString: "")
    let action = NSTextField(wrappingLabelWithString: "")
    let why = NSTextField(wrappingLabelWithString: "")
    let notNow = NSButton(title: "Not now", target: nil, action: nil)
    var index = 0
    var promptSeen = false
    var cancelled = false
    var openedSignin: String?
    var timer: Timer?
    var advancing = false

    init(request: Request) {
        // Resume the same run, keeping finished steps; a new run starts fresh.
        if let saved = readJSON(stateURL, RunState.self), saved.run == request.run, saved.status != "finished" {
            state = saved
            state.status = "running"
        } else {
            state = RunState(run: request.run, status: "running",
                             steps: request.steps.map { StepState(id: $0, state: "pending", detail: nil, changed_at: now()) },
                             updated_at: now())
        }
        panel = Panel(contentRect: NSRect(x: 0, y: 0, width: 300, height: 220),
                      styleMask: [.titled, .fullSizeContentView, .nonactivatingPanel], backing: .buffered, defer: false)
        super.init()
        build()
    }

    func build() {
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovableByWindowBackground = true
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.hidesOnDeactivate = false
        panel.backgroundColor = .windowBackgroundColor
        panel.standardWindowButton(.closeButton)?.isHidden = true
        panel.standardWindowButton(.miniaturizeButton)?.isHidden = true
        panel.standardWindowButton(.zoomButton)?.isHidden = true

        icon.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 26, weight: .regular)
        icon.contentTintColor = .labelColor
        counter.font = .systemFont(ofSize: 12, weight: .medium)
        counter.textColor = .labelColor
        title.font = .systemFont(ofSize: 17, weight: .semibold)
        title.textColor = .labelColor
        action.font = .systemFont(ofSize: 13, weight: .medium)
        action.textColor = .labelColor
        why.font = .systemFont(ofSize: 13)
        why.textColor = .labelColor
        for label in [action, why] { label.preferredMaxLayoutWidth = 252 }
        notNow.bezelStyle = .rounded
        notNow.target = self
        notNow.action = #selector(skip)

        let stack = NSStackView(views: [icon, counter, title, action, why, notNow])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.setCustomSpacing(12, after: icon)
        stack.setCustomSpacing(2, after: counter)
        stack.setCustomSpacing(14, after: why)
        stack.edgeInsets = NSEdgeInsets(top: 30, left: 24, bottom: 20, right: 24)
        stack.translatesAutoresizingMaskIntoConstraints = false
        let content = NSView()
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            stack.topAnchor.constraint(equalTo: content.topAnchor),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            content.widthAnchor.constraint(equalToConstant: 300),
        ])
        panel.contentView = content
    }

    // MARK: Flow

    /// `act`: the person switches it on. `alert`: macOS's own "would like to
    /// record" alert is up. `confirm`: Screen Recording is on and the restarted
    /// app is taking its first screenshot, which macOS may confirm once more.
    var phase = "act"
    var restartedAt: Date?
    var grantedThisRun = false
    var stepStarted = Date()

    func start() {
        // Skip what is already true, so a rerun only shows what is left.
        let p = probePermissions()
        for i in state.steps.indices where state.steps[i].state == "pending" && granted(state.steps[i].id, p) {
            set(i, "already_done")
        }
        grantedThisRun = state.steps.contains { $0.state == "done" && pane[$0.id] != nil }
        save()
        next()
        timer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: true) { [weak self] _ in self?.tick() }
    }

    let open: Set<String> = ["pending", "waiting_user", "waiting_system"]

    /// On current macOS, Accessibility also allows counting input, so Input
    /// Monitoring is only counted when Accessibility was not switched on.
    func counted(_ i: Int) -> Bool {
        let s = state.steps[i]
        if s.state == "already_done" || s.detail == "came with Accessibility" { return false }
        if s.id == "input" && inputFollowsAccessibility { return false }
        if s.id == "input", let ax = state.steps.first(where: { $0.id == "accessibility" }), open.contains(ax.state) {
            return false
        }
        return true
    }

    func next() {
        guard let i = state.steps.indices.first(where: { open.contains(state.steps[$0].state) }) else {
            restartIfNeeded()
            finish()
            return
        }
        index = i
        promptSeen = false
        cancelled = false
        advancing = false
        phase = "act"
        stepStarted = Date()
        let id = state.steps[i].id
        if id == "screen" && state.steps[i].detail == "confirming" {
            // Resumed after the app restarted for its first screenshot.
            phase = "confirm"
            showConfirm()
            return
        }
        if id == "input" && inputFollowsAccessibility,
           let ax = state.steps.first(where: { $0.id == "accessibility" }),
           ["refused", "skipped", "failed"].contains(ax.state) {
            set(i, "skipped", detail: "follows Accessibility")
            next()
            return
        }
        if pane[id] != nil && granted(id, probePermissions()) {
            // An earlier step switched this on too; nothing for the person to do.
            let ax = state.steps.first { $0.id == "accessibility" }?.state == "done"
            set(i, "done", detail: id == "input" && ax ? "came with Accessibility" : nil)
            grantedThisRun = true
            next()
            return
        }
        if pane[id] == nil { restartIfNeeded() }
        set(i, "waiting_user")
        show(id)
        if let anchor = pane[id] {
            requestListing(id)
            if id == "screen" {
                // The first request shows macOS's own alert; its button opens the pane.
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) {
                    if alertVisible() {
                        self.phase = "alert"
                        self.set(i, "waiting_user", detail: "macos alert")
                        self.action.stringValue = "Click Open System Settings."
                    }
                    else { self.openPane(anchor) }
                }
            } else {
                openPane(anchor)
            }
        }
        tick()
    }

    func openPane(_ anchor: String) {
        NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)")!)
    }

    /// Asking once puts Hyperthymesia in the list, so the person only switches it on.
    func requestListing(_ id: String) {
        switch id {
        case "accessibility":
            let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
            _ = AXIsProcessTrustedWithOptions([key: false] as CFDictionary)
        case "input": _ = CGRequestListenEventAccess()
        case "screen": _ = CGRequestScreenCaptureAccess()
        default: break
        }
    }

    /// New grants take effect in a fresh app process. The guide runs in its own
    /// process group, so it keeps going while launchd restarts the app.
    func restartIfNeeded() {
        guard grantedThisRun, restartedAt == nil else { return }
        restartedAt = Date()
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        task.arguments = ["kickstart", "-k", "gui/\(getuid())/com.hyperthymesia.agent"]
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice
        try? task.run()
        task.waitUntilExit()
    }

    func tick() {
        guard !advancing, index < state.steps.count else { return }
        let id = state.steps[index].id
        if id == "signin" { tickSignin(); dock(); return }
        if phase == "confirm" { tickConfirm(); dock(); return }
        if phase == "alert" {
            if !alertVisible() {
                phase = "act"
                set(index, "waiting_user")
                action.stringValue = stepCopy(id).action
                openPane(pane[id]!)
            }
            dock()
            return
        }
        if granted(id, currentPermissions()) {
            grantedThisRun = true
            if id == "screen" {
                set(index, "waiting_system", detail: "confirming")
                phase = "confirm"
                restartedAt = nil
                restartIfNeeded()
                showConfirm()
            } else {
                complete()
            }
            return
        }
        if authPromptVisible() {
            if !promptSeen { promptSeen = true; set(index, "waiting_system") }
            action.stringValue = "Confirm with Touch ID or your password."
        } else if promptSeen {
            // The prompt closed and the switch is still off: it was cancelled.
            promptSeen = false
            cancelled = true
            set(index, "waiting_user", detail: "password prompt cancelled")
            action.stringValue = "Nothing changed. Switch on Hyperthymesia to try again."
        }
        dock()
    }

    /// Done when the restarted app has read screen text.
    func tickConfirm() {
        if capturePromptVisible() {
            set(index, "waiting_system", detail: "macos asks")
            action.stringValue = "Click Allow."
            why.stringValue = "macOS checks once before the first screenshot."
            return
        }
        if settingsSheetVisible() {
            // "Quit & Reopen / Later": the guide already restarted the app.
            action.stringValue = "Click Later."
            why.stringValue = "Hyperthymesia has already restarted itself."
            return
        }
        why.stringValue = "macOS may ask once more before the first screenshot."
        if state.steps[index].detail == "macos asks" { set(index, "waiting_system", detail: "confirming") }
        action.stringValue = "Waiting for the first screen text…"
        let health = support.deletingLastPathComponent().appendingPathComponent("health.json")
        guard let data = try? Data(contentsOf: health),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let media = json["media"] as? [String: Any] else { return }
        let since = restartedAt ?? stepStarted
        let last = (media["last_observed_at"] as? String).flatMap { iso($0) }
        if media["status"] as? String == "running", let last, last >= since.addingTimeInterval(-1) {
            complete()
        } else if Date().timeIntervalSince(since) > 90 {
            set(index, "done", detail: "switched on; first screen text not seen yet")
            advance()
        }
    }

    /// The app writes nanosecond timestamps; whole seconds are enough here.
    func iso(_ s: String) -> Date? {
        let plain = s.replacingOccurrences(of: #"\.\d+"#, with: "", options: .regularExpression)
        return ISO8601DateFormatter().date(from: plain)
    }

    func showConfirm() {
        show("screen")
        title.stringValue = "Screen Recording"
        action.stringValue = "Waiting for the first screen text…"
        why.stringValue = "macOS may ask once more before the first screenshot."
    }

    func tickSignin() {
        guard let s = readJSON(signinURL, SigninStatus.self), s.run == state.run else {
            action.stringValue = "Opening the sign-in page…"
            return
        }
        switch s.state {
        case "done": complete()
        case "failed":
            set(index, "failed", detail: s.error ?? "sign-in failed")
            advance()
        default:
            if let url = s.url, openedSignin != url, let target = URL(string: url) {
                openedSignin = url
                NSWorkspace.shared.open(target)
            }
            action.stringValue = stepCopy("signin").action
        }
    }

    func complete() {
        set(index, "done")
        let c = stepCopy(state.steps[index].id)
        icon.image = NSImage(systemSymbolName: "checkmark.circle.fill", accessibilityDescription: "Done")
        icon.contentTintColor = .systemGreen
        title.stringValue = c.done
        action.isHidden = true
        why.isHidden = true
        notNow.isHidden = true
        advance()
    }

    func advance() {
        advancing = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.9) { self.next() }
    }

    @objc func skip() {
        guard !advancing else { return }
        if phase == "confirm" {
            set(index, "done", detail: "switched on; first screen text not confirmed")
            next()
            return
        }
        set(index, cancelled ? "refused" : "skipped", detail: cancelled ? "password prompt cancelled" : "chose Not now")
        next()
    }

    func finish() {
        timer?.invalidate()
        state.status = "finished"
        save()
        panel.orderOut(nil)
        exit(0)
    }

    func set(_ i: Int, _ value: String, detail: String? = nil) {
        if state.steps[i].state != value || state.steps[i].detail != detail {
            state.steps[i].state = value
            state.steps[i].detail = detail
            state.steps[i].changed_at = now()
        }
        save()
    }
    func save() {
        state.updated_at = now()
        writeAtomically(state, to: stateURL)
    }

    // MARK: Drawing

    func show(_ id: String) {
        let c = stepCopy(id)
        let order = state.steps.indices.filter { counted($0) || $0 == index }
        let position = (order.firstIndex(of: index) ?? 0) + 1
        counter.stringValue = order.count > 1 ? "Step \(position) of \(order.count)" : ""
        counter.isHidden = order.count <= 1
        title.stringValue = c.title
        action.stringValue = c.action
        why.stringValue = c.why
        action.isHidden = false
        why.isHidden = false
        notNow.isHidden = false
        icon.contentTintColor = .labelColor
        if id == "signin", let browser = NSWorkspace.shared.urlForApplication(toOpen: URL(string: "https://example.com")!) {
            icon.image = NSWorkspace.shared.icon(forFile: browser.path)
            icon.image?.size = NSSize(width: 34, height: 34)
        } else {
            icon.image = NSImage(systemSymbolName: c.symbol, accessibilityDescription: c.title)
        }
        panel.contentView?.layoutSubtreeIfNeeded()
        panel.setContentSize(panel.contentView!.fittingSize)
        dock()
        panel.orderFrontRegardless()
    }

    /// Sit beside the window the person has to act in, never on top of it.
    func dock() {
        let id = index < state.steps.count ? state.steps[index].id : ""
        let browser = NSWorkspace.shared.urlForApplication(toOpen: URL(string: "https://example.com")!)
            .flatMap { Bundle(url: $0)?.object(forInfoDictionaryKey: "CFBundleName") as? String }
        let owners: Set<String> = id == "signin" ? Set([browser].compactMap { $0 }) : ["System Settings"]
        guard let screen = NSScreen.screens.first else { return }
        let size = panel.frame.size
        let visible = screen.visibleFrame
        var origin = NSPoint(x: visible.minX + 16, y: visible.maxY - size.height - 16)
        if let target = windows().filter({ owners.contains($0.owner) }).max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }) {
            let top = screen.frame.maxY - target.frame.minY  // CoreGraphics is top-left based
            let y = min(max(top - size.height, visible.minY + 8), visible.maxY - size.height)
            if target.frame.minX - size.width - 12 >= visible.minX {
                origin = NSPoint(x: target.frame.minX - size.width - 12, y: y)
            } else if target.frame.maxX + 12 + size.width <= visible.maxX {
                origin = NSPoint(x: target.frame.maxX + 12, y: y)
            } else if id == "signin" {
                // A full-width browser: sit in the page's empty left margin.
                origin = NSPoint(x: visible.minX + 16, y: max(top - size.height - 90, visible.minY + 16))
            } else {
                // A narrow screen: cover only the Settings sidebar, never the list.
                let x = max(visible.minX + 12, target.frame.minX + 210 - size.width)
                origin = NSPoint(x: x, y: max(top - size.height - 52, visible.minY + 16))
            }
        }
        // Never cover macOS's own alert or sheet: move below it when they overlap.
        let flip = screen.frame.maxY
        for w in windows() where systemOwners.contains(w.owner)
            || (w.owner == "System Settings" && w.frame.width <= 420 && w.frame.height >= 120) {
            let mine = CGRect(x: origin.x, y: flip - origin.y - size.height, width: size.width, height: size.height)
            if mine.intersects(w.frame) {
                let below = flip - (w.frame.maxY + 12) - size.height
                if below >= visible.minY { origin.y = below }
            }
        }
        if panel.frame.origin != origin { panel.setFrameOrigin(origin) }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
signal(SIGTERM) { _ in
    // Quitting halfway leaves the run resumable.
    if var s = readJSON(stateURL, RunState.self), s.status == "running" {
        s.status = "interrupted"
        s.updated_at = now()
        writeAtomically(s, to: stateURL)
    }
    exit(0)
}
let guide = Guide(request: request)
guide.start()
app.run()
