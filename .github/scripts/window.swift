// Prints the id and bounds (in points) of Sandhill's largest window on
// screen: "id x y width height".
import CoreGraphics

let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
let ours = windows.compactMap { w -> (Int, CGRect)? in
    guard let owner = w[kCGWindowOwnerName as String] as? String, owner.lowercased().contains("sandhill"),
          (w[kCGWindowLayer as String] as? Int) == 0,
          let id = w[kCGWindowNumber as String] as? Int,
          let bounds = w[kCGWindowBounds as String] as? [String: Any],
          let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { return nil }
    return (id, rect)
}
guard let (id, rect) = ours.max(by: { $0.1.width * $0.1.height < $1.1.width * $1.1.height }) else {
    print("no Sandhill window on screen")
    exit(1)
}
print(id, Int(rect.minX), Int(rect.minY), Int(rect.width), Int(rect.height))
