import AppKit

final class CompanionPanel: NSPanel {

    init() {
        let size = NSSize(width: 64, height: 64)
        super.init(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.nonactivatingPanel, .borderless],
            backing: .buffered,
            defer: false
        )

        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        isMovableByWindowBackground = true
        ignoresMouseEvents = false // flip to true for full click-through mode

        let view = CompanionView(frame: NSRect(origin: .zero, size: size))
        contentView = view

        setFrameOrigin(defaultOrigin(for: size))
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    private func defaultOrigin(for size: NSSize) -> NSPoint {
        guard let screen = NSScreen.main?.visibleFrame else { return .zero }
        let x = screen.maxX - size.width - 40
        let y = screen.minY + 80
        return NSPoint(x: x, y: y)
    }
}

/// Placeholder view for the character sprite. Task 6 (pixel-art sprites) replaces
/// the fill with actual frames driven by CompanionState.
final class CompanionView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        NSColor.systemBlue.withAlphaComponent(0.8).setFill()
        let path = NSBezierPath(ovalIn: bounds.insetBy(dx: 4, dy: 4))
        path.fill()
    }

    override func mouseDown(with event: NSEvent) {
        // Placeholder click handler — task 9 (chat popup) wires this up for real.
        print("Companion clicked")
    }
}
