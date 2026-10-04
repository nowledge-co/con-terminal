#!/usr/bin/env swift
// Usage: swift scripts/macos/simulate-remote-input.swift <con-pid> [text]
// Focus a disposable terminal pane first. Every character uses virtual key 0,
// reproducing remote clients that supply Unicode with a placeholder A keycode.
import AppKit

guard CommandLine.arguments.count >= 2,
      let pid = Int32(CommandLine.arguments[1]), pid > 0 else {
    fputs("Usage: simulate-remote-input.swift <con-pid> [text]\n", stderr)
    exit(2)
}
guard CGPreflightPostEventAccess() else {
    fputs("Event posting is unavailable. Run from a terminal with Accessibility access.\n", stderr)
    exit(1)
}

let text = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "utioweqbz 123 ABC 你好😀"
guard text.unicodeScalars.allSatisfy({ !CharacterSet.controlCharacters.contains($0) }) else {
    fputs("Only printable text is accepted; control characters could submit a shell command.\n", stderr)
    exit(2)
}
for character in text {
    let utf16 = Array(String(character).utf16)
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down) else {
            fatalError("Could not create keyboard event")
        }
        event.flags = []
        utf16.withUnsafeBufferPointer {
            event.keyboardSetUnicodeString(stringLength: $0.count, unicodeString: $0.baseAddress!)
        }
        event.postToPid(pid)
    }
    print("keycode=0 modifiers=0 text=\(String(character).debugDescription)")
    Thread.sleep(forTimeInterval: 0.05)
}
