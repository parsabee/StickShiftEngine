// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import Testing

@testable import StickShiftAppKit

/// A clock the test moves by hand, in nanoseconds.
@MainActor
private final class FakeClock {
    var now: UInt64 = 1_000_000_000

    func advance(milliseconds: UInt64) {
        now += milliseconds * 1_000_000
    }
}

/// The watchdog on a fake clock: when the timer's ticks drive frames.
@MainActor
@Suite("frame watchdog")
struct FrameWatchdogTests {
    private func watchdog() -> (FrameWatchdog, FakeClock, () -> Int) {
        let clock = FakeClock()
        let watchdog = FrameWatchdog()
        watchdog.clock = { clock.now }
        var driven = 0
        watchdog.drive = { driven += 1 }
        return (watchdog, clock, { driven })
    }

    @Test func itDrivesFramesOnlyOnceTheDisplayLinkHasBeenSilentFor250ms() {
        let (watchdog, clock, driven) = watchdog()
        defer { watchdog.stop() }
        #expect(FrameWatchdog.stallNanoseconds == 250_000_000)
        #expect(FrameWatchdog.interval == 1.0 / 60.0)

        // Stopped, it never drives.
        clock.advance(milliseconds: 1000)
        #expect(!watchdog.timerFired())
        #expect(!watchdog.isRunning)

        // Started: the silence counts from now.
        watchdog.start()
        #expect(watchdog.isRunning)
        #expect(watchdog.lastDisplayFrame == clock.now)
        clock.advance(milliseconds: 249)
        #expect(!watchdog.timerFired())
        clock.advance(milliseconds: 1)
        #expect(watchdog.timerFired(), "250 ms of silence")
        // Every tick drives while the display link stays silent.
        clock.advance(milliseconds: 16)
        #expect(watchdog.timerFired())
        #expect(driven() == 2)

        // The display link fires: its frames are its own again.
        watchdog.displayLinkFired()
        clock.advance(milliseconds: 16)
        #expect(!watchdog.timerFired())
        clock.advance(milliseconds: 200)
        #expect(!watchdog.timerFired())
        watchdog.displayLinkFired()
        clock.advance(milliseconds: 249)
        #expect(!watchdog.timerFired())
        clock.advance(milliseconds: 1)
        #expect(watchdog.timerFired())
        #expect(driven() == 3)

        // A clock that went back drives nothing.
        watchdog.displayLinkFired()
        clock.now -= 1_000_000_000
        #expect(!watchdog.timerFired())

        // Stopped again: nothing, and the display link no longer arms it.
        watchdog.stop()
        #expect(!watchdog.isRunning)
        #expect(watchdog.lastDisplayFrame == nil)
        watchdog.displayLinkFired()
        clock.advance(milliseconds: 1000)
        #expect(!watchdog.timerFired())
        #expect(driven() == 3)
    }

    @Test func startingTwiceKeepsOneTimerAndTheFirstMark() {
        let (watchdog, clock, _) = watchdog()
        defer { watchdog.stop() }
        watchdog.start()
        let mark = watchdog.lastDisplayFrame
        clock.advance(milliseconds: 100)
        watchdog.start()
        #expect(watchdog.lastDisplayFrame == mark)
    }

    /// The timer is in the main run loop's common modes, so it fires while a
    /// menu is open (AppKit tracks a menu in the event-tracking mode) or a
    /// window is dragged. Checked on the run loop itself: spinning the main run
    /// loop here would run other tests inside this one.
    @Test func theTimerIsInTheCommonModesSoItFiresWhileAMenuIsTracking() throws {
        _ = NSApplication.shared
        let (watchdog, clock, driven) = watchdog()
        defer { watchdog.stop() }
        watchdog.start()
        let timer = try #require(watchdog.timer)
        #expect(timer.timeInterval == FrameWatchdog.interval)
        let main = CFRunLoopGetMain()
        let cfTimer = unsafeBitCast(timer, to: CFRunLoopTimer.self)
        #expect(CFRunLoopContainsTimer(main, cfTimer, .commonModes))
        #expect(CFRunLoopContainsTimer(main, cfTimer, CFRunLoopMode(RunLoop.Mode.eventTracking.rawValue as CFString)))
        #expect(CFRunLoopContainsTimer(main, cfTimer, .defaultMode))

        // Its firing is a timer tick.
        clock.advance(milliseconds: 1000)
        timer.fire()
        #expect(driven() == 1)
        watchdog.stop()
        #expect(!timer.isValid, "stop invalidates it")
    }

    @Test func aWatchdogDroppedWithoutStoppingEndsItsTimer() throws {
        var watchdog: FrameWatchdog? = FrameWatchdog()
        watchdog?.start()
        let timer = try #require(watchdog?.timer)
        watchdog = nil
        #expect(timer.isValid)
        timer.fire()
        #expect(!timer.isValid)
    }
}
