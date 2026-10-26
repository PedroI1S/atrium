// Captures the app's window over the synthetic backdrop window with ScreenCaptureKit, using a
// filter that INCLUDES ONLY those two processes' windows: nothing else on the user's screen can
// appear in the image, whatever is in front. Crops to the app window plus a margin.
// Usage: capture <app-pid> <backdrop-pid> <largest|smallest> <out.png> [margin-pt]
import AppKit
import ScreenCaptureKit

let a = CommandLine.arguments
guard a.count >= 5, let appPid = Int32(a[1]), let backdropPid = Int32(a[2]) else {
    FileHandle.standardError.write("usage: capture app-pid backdrop-pid largest|smallest out.png [margin]\n".data(using: .utf8)!)
    exit(2)
}
let smallest = a[3] == "smallest"
let out = URL(fileURLWithPath: a[4])
let margin = a.count > 5 ? CGFloat(Double(a[5]) ?? 32) : 32

func fail(_ msg: String) -> Never {
    FileHandle.standardError.write("capture: \(msg)\n".data(using: .utf8)!)
    exit(1)
}

Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(true, onScreenWindowsOnly: true)
        let mine = content.windows.filter { $0.owningApplication?.processID == appPid && $0.windowLayer == 0 && $0.frame.width * $0.frame.height > 10_000 }
        let backdrop = content.windows.filter { $0.owningApplication?.processID == backdropPid }
        guard let target = (smallest ? mine.min { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height } : mine.max { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }) else { fail("no app window") }
        guard !backdrop.isEmpty else { fail("no backdrop window") }
        guard let display = content.displays.first(where: { $0.frame.intersects(target.frame) }) else { fail("no display") }
        let filter = SCContentFilter(display: display, including: backdrop + mine)
        let config = SCStreamConfiguration()
        let scale = CGFloat(filter.pointPixelScale)
        let local = target.frame.offsetBy(dx: -display.frame.minX, dy: -display.frame.minY).insetBy(dx: -margin, dy: -margin)
        config.sourceRect = local
        config.width = Int(local.width * scale)
        config.height = Int(local.height * scale)
        config.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
        let rep = NSBitmapImageRep(cgImage: image)
        guard let png = rep.representation(using: .png, properties: [:]) else { fail("encode") }
        try png.write(to: out)
        exit(0)
    } catch {
        fail("\(error)")
    }
}
RunLoop.main.run()
