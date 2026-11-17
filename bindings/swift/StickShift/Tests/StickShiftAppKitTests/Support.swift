// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Foundation
import StickShift

/// An offscreen Metal context, or `nil` without a Metal device. With
/// `STICKSHIFT_REQUIRE_GPU=1` (CI) a missing device is a failure instead.
@MainActor
func metalContext(width: Int = 64, height: Int = 48) throws -> Context? {
    do {
        return try Context(offscreenWidth: width, height: height)
    } catch let error as StickShiftError where error.code == .noDevice {
        let required = ProcessInfo.processInfo.environment["STICKSHIFT_REQUIRE_GPU"]
        if required == "1" || required?.lowercased() == "true" {
            throw error
        }
        FileHandle.standardError.write(Data("SKIPPED: no Metal device\n".utf8))
        return nil
    }
}
