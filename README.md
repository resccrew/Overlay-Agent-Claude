# Desktop Companion

Always-on-top floating character for macOS that reflects live Claude Code session status
(idle / thinking / typing / waiting / error / done), with a click-to-open chat popup.

Plan and task tracking: Notion "Задачи" database (project: "Desktop-компаньон (macOS)").

## Status

Skeleton stage — floating `NSPanel` window with a placeholder view, `.accessory` activation
policy (no Dock icon), draggable, always on top across spaces. Character state enum defined
but not yet wired to real Claude Code activity or sprites.

## Running

This is a Swift Package (no `.xcodeproj` needed to build/run on macOS):

```sh
swift run
```

Requires macOS + Xcode command line tools. Not buildable on Linux (AppKit).

## Layout

- `Sources/DesktopCompanion/main.swift` — app entry point
- `Sources/DesktopCompanion/AppDelegate.swift` — app lifecycle, activation policy
- `Sources/DesktopCompanion/CompanionPanel.swift` — the floating NSPanel + placeholder view
- `Sources/DesktopCompanion/CompanionState.swift` — character state enum

## Next up (see Notion for full breakdown)

- Wire `CompanionState` to a real state machine driving view appearance
- Find and tail the Claude Code session source (JSONL transcript / hook events)
- Replace placeholder circle with pixel-art sprites per state
