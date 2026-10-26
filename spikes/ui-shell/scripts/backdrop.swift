// A full-screen synthetic wallpaper window, so opacity screenshots show real vibrancy and real
// window alpha over a known backdrop instead of whatever is on the user's screen.
// Usage: backdrop <light|dark>      (runs until killed; compiled by scripts/screenshots.sh)
import AppKit

let dark = CommandLine.arguments.dropFirst().first == "dark"

final class Wallpaper: NSView {
    func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
        CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
    }
    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        let W = bounds.width, H = bounds.height
        ctx.setFillColor(dark ? rgb(0x1b1a20) : rgb(0xe3ded6))
        ctx.fill(bounds)
        let blobs: [(CGFloat, CGFloat, UInt32)] = dark
            ? [(0.12, 0.85, 0x7a4a44), (0.88, 0.8, 0x34507e), (0.55, 0.05, 0x2f5a3e)]
            : [(0.12, 0.85, 0xefb08f), (0.88, 0.8, 0x95b3e2), (0.55, 0.05, 0x9cc7a5)]
        for (x, y, c) in blobs {
            let center = CGPoint(x: W * x, y: H * y)
            let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!, colors: [rgb(c), rgb(c, 0)] as CFArray, locations: [0, 1])!
            ctx.drawRadialGradient(g, startCenter: center, startRadius: 0, endCenter: center, endRadius: max(W, H) * 0.55, options: [])
        }
        // Text-like bars so see-through areas show structure (and the blur is visible).
        ctx.setFillColor(dark ? rgb(0xffffff, 0.16) : rgb(0x000000, 0.14))
        var y = H - 60
        var row = 0
        while y > 20 {
            let len: CGFloat = [0.62, 0.48, 0.7, 0.4, 0.66][row % 5]
            ctx.fill(CGRect(x: 60, y: y, width: W * len, height: 9))
            y -= 26
            row += 1
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let frame = NSScreen.main?.frame ?? .zero
let window = NSWindow(contentRect: frame, styleMask: .borderless, backing: .buffered, defer: false)
window.contentView = Wallpaper(frame: frame)
window.level = .normal
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
app.run()
