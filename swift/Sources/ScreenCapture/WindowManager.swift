import Foundation
import ScreenCaptureKit
import CoreGraphics
import AppKit

/// Manages window listing and screen capture using ScreenCaptureKit.
/// Must run on MainActor — ScreenCaptureKit requires CoreGraphics server init (main thread only).
@MainActor
public class WindowManager {

    /// Ensures the window server (CGS) connection is established.
    /// CLI apps don't get this automatically — AppKit apps do via NSApplication.
    private static let _ensureCGS: Void = { _ = NSApplication.shared }()

    public init() {
        _ = Self._ensureCGS
    }

    // MARK: - Window Listing

    /// List all on-screen windows with their metadata
    public func listWindows() async throws -> [WindowInfo] {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: true
        )

        return content.windows.compactMap { window in
            // Skip very small windows (menu bar items, etc.)
            guard window.frame.width > 50, window.frame.height > 50 else { return nil }

            return WindowInfo(
                windowID: window.windowID,
                title: window.title,
                appName: window.owningApplication?.applicationName,
                bundleId: window.owningApplication?.bundleIdentifier,
                frame: window.frame
            )
        }
    }

    // MARK: - Window Capture

    /// Capture a window by its ID
    public func captureWindow(windowId: UInt32) async throws -> (CGImage, WindowInfo) {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: true
        )

        guard let window = content.windows.first(where: { $0.windowID == windowId }) else {
            throw CaptureError.windowNotFound("No window with ID \(windowId)")
        }

        let info = WindowInfo(
            windowID: window.windowID,
            title: window.title,
            appName: window.owningApplication?.applicationName,
            bundleId: window.owningApplication?.bundleIdentifier,
            frame: window.frame
        )

        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        config.width = Int(window.frame.width) * 2 // Retina
        config.height = Int(window.frame.height) * 2

        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter, configuration: config
        )

        return (image, info)
    }

    /// Capture a window by title substring match
    public func captureByTitle(_ title: String) async throws -> (CGImage, WindowInfo) {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: true
        )

        let titleLower = title.lowercased()
        guard let window = content.windows.first(where: {
            let winTitle = ($0.title ?? "").lowercased()
            let appName = ($0.owningApplication?.applicationName ?? "").lowercased()
            // Match against title, app name, or "app — title" combined
            return winTitle.contains(titleLower)
                || "\(appName) — \(winTitle)".contains(titleLower)
        }) else {
            throw CaptureError.windowNotFound("No window matching title '\(title)'")
        }

        let info = WindowInfo(
            windowID: window.windowID,
            title: window.title,
            appName: window.owningApplication?.applicationName,
            bundleId: window.owningApplication?.bundleIdentifier,
            frame: window.frame
        )

        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        config.width = Int(window.frame.width) * 2
        config.height = Int(window.frame.height) * 2

        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter, configuration: config
        )

        return (image, info)
    }

    /// Capture a window by application/process name
    public func captureByProcess(_ name: String) async throws -> (CGImage, WindowInfo) {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: true
        )

        let nameLower = name.lowercased()
        guard let window = content.windows.first(where: {
            ($0.owningApplication?.applicationName ?? "").lowercased().contains(nameLower)
        }) else {
            throw CaptureError.windowNotFound("No window for process '\(name)'")
        }

        let info = WindowInfo(
            windowID: window.windowID,
            title: window.title,
            appName: window.owningApplication?.applicationName,
            bundleId: window.owningApplication?.bundleIdentifier,
            frame: window.frame
        )

        let filter = SCContentFilter(desktopIndependentWindow: window)
        let config = SCStreamConfiguration()
        config.width = Int(window.frame.width) * 2
        config.height = Int(window.frame.height) * 2

        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter, configuration: config
        )

        return (image, info)
    }

    // MARK: - Utility

    /// Crop a CGImage using normalized coordinates (0.0–1.0).
    public static func cropCGImage(
        _ image: CGImage, x: Double, y: Double, w: Double, h: Double
    ) -> CGImage? {
        let imgW = Double(image.width)
        let imgH = Double(image.height)
        let rect = CGRect(
            x: (x * imgW).rounded(),
            y: (y * imgH).rounded(),
            width: (w * imgW).rounded(),
            height: (h * imgH).rounded()
        )
        return image.cropping(to: rect)
    }

    /// Convert a CGImage to base64-encoded PNG string
    public static func cgImageToBase64(_ image: CGImage) -> String? {
        let rep = NSBitmapImageRep(cgImage: image)
        guard let pngData = rep.representation(using: .png, properties: [:]) else {
            return nil
        }
        return pngData.base64EncodedString()
    }

    // MARK: - Errors

    public enum CaptureError: Error, CustomStringConvertible {
        case windowNotFound(String)
        case captureFailed(String)

        public var description: String {
            switch self {
            case .windowNotFound(let msg): return msg
            case .captureFailed(let msg): return msg
            }
        }
    }
}
