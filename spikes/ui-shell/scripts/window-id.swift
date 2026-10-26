// Prints "<CGWindowID> <x> <y> <w> <h>" of an on-screen window owned by the given pid:
// the largest one (default, the main window) or the smallest (a pop-out).
// Usage: window-id <pid> [largest|smallest]      (compiled by scripts/screenshots.sh)
import CoreGraphics
import Foundation

let args = CommandLine.arguments.dropFirst()
let pid = Int(args.first ?? "") ?? -1
let smallest = args.dropFirst().first == "smallest"
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
var best: (id: Int, b: [String: CGFloat], area: CGFloat)?
for w in list {
    guard (w[kCGWindowOwnerPID as String] as? Int) == pid, (w[kCGWindowLayer as String] as? Int) == 0 else { continue }
    let b = w[kCGWindowBounds as String] as? [String: CGFloat] ?? [:]
    let area = (b["Width"] ?? 0) * (b["Height"] ?? 0)
    guard area > 10_000 else { continue }
    if best == nil || (smallest ? area < best!.area : area > best!.area) {
        best = (w[kCGWindowNumber as String] as? Int ?? 0, b, area)
    }
}
if let b = best {
    print("\(b.id) \(Int(b.b["X"] ?? 0)) \(Int(b.b["Y"] ?? 0)) \(Int(b.b["Width"] ?? 0)) \(Int(b.b["Height"] ?? 0))")
    exit(0)
}
exit(1)
