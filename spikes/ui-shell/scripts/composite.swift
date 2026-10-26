// Composites a window capture (PNG with alpha, from `screencapture -o -l`) over a synthetic
// wallpaper, so translucency is visible without capturing anything else on the user's screen,
// then writes a downscaled JPEG.
// Usage: composite <in.png> <out.jpg> <light|dark> [max-width]
import AppKit
import CoreGraphics
import Foundation

let a = CommandLine.arguments
guard a.count >= 4, let src = NSImage(contentsOfFile: a[1]), let cg = src.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    FileHandle.standardError.write("usage: composite in.png out.jpg light|dark [max-width]\n".data(using: .utf8)!)
    exit(2)
}
let dark = a[3] == "dark"
let maxWidth = a.count > 4 ? Double(a[4]) ?? 1440 : 1440
let margin = 48.0
let scale = min(1, maxWidth / (Double(cg.width) + 2 * margin))
let W = Int((Double(cg.width) + 2 * margin) * scale)
let H = Int((Double(cg.height) + 2 * margin) * scale)
let ctx = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func rgb(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255, blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
}
// Wallpaper: three soft colour blobs on a neutral ground (like the prototype's desk), plus a
// few text-like bars so see-through areas show structure behind the window.
ctx.setFillColor(dark ? rgb(0x1b1a20) : rgb(0xe3ded6))
ctx.fill(CGRect(x: 0, y: 0, width: W, height: H))
let blobs: [(Double, Double, UInt32)] = dark
    ? [(0.12, 0.85, 0x5b3c39), (0.88, 0.8, 0x2e3e5d), (0.55, 0.05, 0x2c4535)]
    : [(0.12, 0.85, 0xefc5ac), (0.88, 0.8, 0xb1c5e4), (0.55, 0.05, 0xb6d0bb)]
for (x, y, c) in blobs {
    let center = CGPoint(x: Double(W) * x, y: Double(H) * y)
    let g = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!, colors: [rgb(c), rgb(c, 0)] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(g, startCenter: center, startRadius: 0, endCenter: center, endRadius: Double(max(W, H)) * 0.55, options: [])
}
ctx.setFillColor(dark ? rgb(0xffffff, 0.10) : rgb(0x000000, 0.08))
for row in 0..<28 {
    let y = Double(H) - 80 * scale - Double(row) * 34 * scale
    let len = [0.62, 0.48, 0.7, 0.4, 0.66][row % 5]
    ctx.fill(CGRect(x: 90 * scale, y: y, width: Double(W) * len, height: 10 * scale))
}
ctx.interpolationQuality = .high
ctx.draw(cg, in: CGRect(x: margin * scale, y: margin * scale, width: Double(cg.width) * scale, height: Double(cg.height) * scale))
let out = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: out)
let data = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.72])!
try! data.write(to: URL(fileURLWithPath: a[2]))
