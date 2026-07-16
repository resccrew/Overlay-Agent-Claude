import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var panel: CompanionPanel?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory) // no Dock icon, no menu bar takeover

        let panel = CompanionPanel()
        panel.orderFrontRegardless()
        self.panel = panel
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}
