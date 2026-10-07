import AppKit
import ObjectiveC.runtime

@MainActor
func getCursor(_ name: String) -> NSCursor? {
    let selector = NSSelectorFromString(name)
    guard let method = class_getClassMethod(NSCursor.self, selector),
          method_getNumberOfArguments(method) == 2 else { return nil }
    let encoding = method_copyReturnType(method)
    defer { free(encoding) }
    guard String(cString: encoding).hasPrefix("@") else { return nil }
    typealias Factory = @convention(c) (AnyClass, Selector) -> Unmanaged<AnyObject>?
    let factory = unsafeBitCast(method_getImplementation(method), to: Factory.self)
    return factory(NSCursor.self, selector)?.takeUnretainedValue() as? NSCursor
}

@MainActor
final class CursorTile: NSView {
    let selectorName: String
    var cursor: NSCursor?
    var didResolveCursor = false
    var hoverArea: NSTrackingArea?
    let preview = NSImageView()
    let label: NSTextField

    init(_ name: String) {
        selectorName = name
        label = NSTextField(wrappingLabelWithString: name)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.borderWidth = 1
        toolTip = "Click to copy selector"

        preview.imageScaling = .scaleProportionallyDown
        preview.translatesAutoresizingMaskIntoConstraints = false
        label.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        label.textColor = .labelColor
        label.cell?.lineBreakMode = .byCharWrapping
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(preview)
        addSubview(label)
        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: 72),
            preview.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            preview.centerYAnchor.constraint(equalTo: centerYAnchor),
            preview.widthAnchor.constraint(equalToConstant: 36),
            preview.heightAnchor.constraint(equalToConstant: 36),
            label.leadingAnchor.constraint(equalTo: preview.trailingAnchor, constant: 10),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is unsupported") }

    override func updateLayer() {
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        layer?.borderColor = NSColor.separatorColor.cgColor
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateTrackingAreas() {
        if let hoverArea { removeTrackingArea(hoverArea) }
        let area = NSTrackingArea(rect: .zero,
                                  options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                  owner: self, userInfo: nil)
        hoverArea = area
        addTrackingArea(area)
        super.updateTrackingAreas()
    }

    override func mouseEntered(with event: NSEvent) {
        if !didResolveCursor {
            didResolveCursor = true
            NSLog("Loading cursor %@", selectorName)
            cursor = getCursor(selectorName)
            if let cursor {
                NSLog("Loading image %@", selectorName)
                preview.image = cursor.image
            } else {
                label.stringValue = "\(selectorName)\nUnavailable"
                label.textColor = .secondaryLabelColor
            }
            NSLog("Finished cursor %@", selectorName)
            window?.invalidateCursorRects(for: self)
        }
        (cursor ?? .arrow).set()
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        addCursorRect(bounds, cursor: cursor ?? .arrow)
    }

    override func mouseDown(with event: NSEvent) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(selectorName, forType: .string)
    }
}

@MainActor
final class DemoDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        let content = NSStackView()
        content.orientation = .vertical
        content.alignment = .leading
        content.spacing = 10
        content.edgeInsets = NSEdgeInsets(top: 16, left: 16, bottom: 16, right: 16)
        content.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = content
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            content.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            content.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
        ])
        let selectors = """
arrowCursor
IBeamCursor
pointingHandCursor
closedHandCursor
openHandCursor
resizeLeftCursor
resizeRightCursor
resizeLeftRightCursor
resizeUpCursor
resizeDownCursor
resizeUpDownCursor
crosshairCursor
disappearingItemCursor
operationNotAllowedCursor
busyButClickableCursor
contextualMenuCursor
IBeamCursorForVerticalLayout
currentCursor
currentSystemCursor
dragCopyCursor
_copyDragCursor
dragLinkCursor
_genericDragCursor
_handCursor
_closedHandCursor
_moveCursor
_waitCursor
_crosshairCursor
_horizontalResizeCursor
_verticalResizeCursor
_bottomLeftResizeCursor
_topLeftResizeCursor
_bottomRightResizeCursor
_topRightResizeCursor
_resizeLeftCursor
_resizeRightCursor
_resizeLeftRightCursor
_zoomInCursor
_zoomOutCursor
_windowResizeEastCursor
_windowResizeWestCursor
_windowResizeEastWestCursor
_windowResizeNorthCursor
_windowResizeSouthCursor
_windowResizeNorthSouthCursor
_windowResizeNorthEastCursor
_windowResizeNorthWestCursor
_windowResizeSouthEastCursor
_windowResizeSouthWestCursor
_windowResizeNorthEastSouthWestCursor
_windowResizeNorthWestSouthEastCursor
""".split(separator: "\n").map(String.init)
        let groups = [
            ("Window resize", selectors.filter { $0.hasPrefix("_windowResize") }),
            ("Other cursors", selectors.filter { !$0.hasPrefix("_windowResize") }),
        ]
        for (title, names) in groups {
            let heading = NSTextField(labelWithString: title)
            heading.font = .systemFont(ofSize: 16, weight: .semibold)
            content.addArrangedSubview(heading)
            for start in stride(from: 0, to: names.count, by: 3) {
                let row = NSStackView()
                row.orientation = .horizontal
                row.distribution = .fillEqually
                row.spacing = 8
                for name in names[start..<min(start + 3, names.count)] {
                    row.addArrangedSubview(CursorTile(name))
                }
                while row.arrangedSubviews.count < 3 { row.addArrangedSubview(NSView()) }
                content.addArrangedSubview(row)
                row.widthAnchor.constraint(equalTo: content.widthAnchor, constant: -32).isActive = true
            }
        }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 760),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable],
                              backing: .buffered, defer: false)
        window.title = "Cursor Demo"
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 800, height: 360)
        window.contentView = scroll
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
        NSApp.activate(ignoringOtherApps: true)
        NSLog("Cursor Demo window ready")
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

MainActor.assumeIsolated {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let menu = NSMenu()
    let appItem = NSMenuItem()
    let appMenu = NSMenu()
    appMenu.addItem(withTitle: "Quit Cursor Demo", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
    appItem.submenu = appMenu
    menu.addItem(appItem)
    app.mainMenu = menu
    let delegate = DemoDelegate()
    app.delegate = delegate
    withExtendedLifetime(delegate) { app.run() }
}
