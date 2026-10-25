// Prints the CGWindowID of the largest on-screen window owned by the given pid.
// Usage: window-id <pid>      (compiled from this file by scripts/screenshots.sh)
import CoreGraphics
import Foundation

let pid = Int(CommandLine.arguments.dropFirst().first ?? "") ?? -1
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
var best: (id: Int, area: CGFloat)?
for w in list {
    guard (w[kCGWindowOwnerPID as String] as? Int) == pid, (w[kCGWindowLayer as String] as? Int) == 0 else { continue }
    let b = w[kCGWindowBounds as String] as? [String: CGFloat] ?? [:]
    let area = (b["Width"] ?? 0) * (b["Height"] ?? 0)
    if area > (best?.area ?? 0) { best = (w[kCGWindowNumber as String] as? Int ?? 0, area) }
}
if let b = best { print(b.id); exit(0) }
exit(1)
