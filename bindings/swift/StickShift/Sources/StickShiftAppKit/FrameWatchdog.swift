// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Darwin
import Foundation

/// Keeps a game running while its view's display link is stopped.
///
/// AppKit stops a view's display link while the view is on no display: the
/// window minimised, the app hidden (⌘H), the display asleep. The Rust runner's
/// winit loop has no such pause: it asks for the next redraw after every frame,
/// visible or not, and each frame runs its fixed updates and then renders, with
/// or without a drawable. So a Rust game goes on while its window is hidden.
///
/// A watchdog does the same for a view: a timer at ``interval`` (60 Hz), in the
/// main run loop's common modes, so it also fires while a menu is open or a
/// window is being dragged. Once the display link has been silent for
/// ``stallNanoseconds`` (a quarter of a second: many frames at any refresh
/// rate, so a running display link never shares its frames), every timer tick
/// runs ``drive``, until the display link fires again. ``EngineView`` drives an
/// engine tick there, on the same clock as its display link's, so the game keeps
/// its pace; the first tick after the stall catches up at most
/// ``Engine/maxStepsPerFrame`` steps, as any long frame does.
@MainActor
public final class FrameWatchdog {
    /// How long the display link may be silent before the timer drives the
    /// frames: 250 ms.
    public static let stallNanoseconds: UInt64 = 250_000_000
    /// The timer's interval, in seconds.
    public static let interval: TimeInterval = 1.0 / 60.0

    /// The monotonic clock, in nanoseconds, that does not advance while the
    /// machine sleeps: `CLOCK_UPTIME_RAW`, what Rust's `Instant` reads on macOS.
    public nonisolated static func monotonicNanoseconds() -> UInt64 {
        clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
    }

    /// Runs one frame, for each timer tick while the display link is silent.
    public var drive: (() -> Void)?
    /// The clock the silence is measured on. Tests swap in their own.
    var clock: () -> UInt64
    /// The clock at the display link's last frame, or when the watchdog
    /// started; `nil` while it is stopped.
    public private(set) var lastDisplayFrame: UInt64?
    /// The timer, while the watchdog runs.
    private(set) var timer: Timer?

    /// A watchdog that runs `drive` for each frame it drives.
    public init(drive: (() -> Void)? = nil) {
        self.drive = drive
        clock = { FrameWatchdog.monotonicNanoseconds() }
    }

    /// Whether the timer is scheduled: between ``start()`` and ``stop()``.
    public var isRunning: Bool { timer != nil }

    /// Frames have started: count the display link's silence from now, and
    /// schedule the timer. Does nothing if it is already running.
    public func start() {
        guard timer == nil else { return }
        lastDisplayFrame = clock()
        let timer = Timer(timeInterval: Self.interval, repeats: true) { [weak self] timer in
            // A watchdog dropped without `stop()` leaves its timer behind: end it here.
            guard let self else {
                timer.invalidate()
                return
            }
            // The main run loop fires it, on the main thread.
            MainActor.assumeIsolated { _ = self.timerFired() }
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    /// Stop the timer. No frame is driven until ``start()``.
    public func stop() {
        timer?.invalidate()
        timer = nil
        lastDisplayFrame = nil
    }

    /// The display link drove a frame: the silence restarts now.
    public func displayLinkFired() {
        if lastDisplayFrame != nil {
            lastDisplayFrame = clock()
        }
    }

    /// A tick of the timer: run ``drive`` if the watchdog is running and the
    /// display link has been silent for at least ``stallNanoseconds``.
    /// Returns whether it did.
    @discardableResult
    func timerFired() -> Bool {
        guard let last = lastDisplayFrame else { return false }
        let now = clock()
        guard now >= last, now - last >= Self.stallNanoseconds else { return false }
        drive?()
        return true
    }
}
