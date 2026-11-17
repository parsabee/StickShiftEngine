// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

/// An error from the engine: the C API's result code and its description.
///
/// Every throwing call in this package throws one. Match on ``code``:
///
/// ```swift
/// do {
///     try frame.addLight(light)
/// } catch let error as StickShiftError where error.code == .limitExceeded {
///     // The frame already holds `Light.maxPerFrame` lights.
/// }
/// ```
public struct StickShiftError: Error, Equatable, Sendable, CustomStringConvertible {
    /// A C API result code (`STICKSHIFT_ERROR_*`).
    public struct Code: RawRepresentable, Hashable, Sendable, CustomStringConvertible {
        /// The raw `StickShiftResult`.
        public let rawValue: Int32

        /// Creates a code from a raw `StickShiftResult`.
        public init(rawValue: Int32) {
            self.rawValue = rawValue
        }

        /// A bad argument: a value out of range, sizes that don't match.
        public static let invalidArgument = Code(rawValue: STICKSHIFT_ERROR_INVALID_ARGUMENT)
        /// A destroyed handle, or another context's.
        public static let invalidHandle = Code(rawValue: STICKSHIFT_ERROR_INVALID_HANDLE)
        /// Not allowed now: no frame open, a frame already open.
        public static let wrongState = Code(rawValue: STICKSHIFT_ERROR_WRONG_STATE)
        /// Another call on the context is in progress (a second thread).
        public static let busy = Code(rawValue: STICKSHIFT_ERROR_BUSY)
        /// No Metal device.
        public static let noDevice = Code(rawValue: STICKSHIFT_ERROR_NO_DEVICE)
        /// Not supported by this context or build.
        public static let unsupported = Code(rawValue: STICKSHIFT_ERROR_UNSUPPORTED)
        /// A file could not be read.
        public static let io = Code(rawValue: STICKSHIFT_ERROR_IO)
        /// A glTF file or image could not be decoded.
        public static let parse = Code(rawValue: STICKSHIFT_ERROR_PARSE)
        /// An engine limit was hit.
        public static let limitExceeded = Code(rawValue: STICKSHIFT_ERROR_LIMIT_EXCEEDED)
        /// A bug in the engine. Destroy the context.
        public static let `internal` = Code(rawValue: STICKSHIFT_ERROR_INTERNAL)
        /// A wait gave up before its work finished.
        public static let timeout = Code(rawValue: STICKSHIFT_ERROR_TIMEOUT)
        /// An engine, or a context with an engine attached, used from a thread
        /// other than the engine's.
        public static let wrongThread = Code(rawValue: STICKSHIFT_ERROR_WRONG_THREAD)
        /// The game's init failed; the engine is failed.
        public static let aborted = Code(rawValue: STICKSHIFT_ERROR_ABORTED)

        /// The constant's name, for example `"STICKSHIFT_ERROR_BUSY"`.
        public var description: String {
            String(cString: stickshift_result_name(rawValue))
        }
    }

    /// What went wrong, as the C API reports it.
    public let code: Code
    /// The engine's description of this failure. May be empty.
    public let message: String

    /// Creates an error from a code and a message.
    public init(code: Code, message: String) {
        self.code = code
        self.message = message
    }

    public var description: String {
        message.isEmpty ? code.description : "\(code): \(message)"
    }
}

/// Throw for an error code, with this thread's last error message; return any
/// other result (`STICKSHIFT_OK`, `STICKSHIFT_FRAME_SKIPPED`, `STICKSHIFT_NOT_READY`).
@discardableResult
func check(_ result: StickShiftResult) throws(StickShiftError) -> StickShiftResult {
    guard result < 0 else { return result }
    throw StickShiftError(
        code: StickShiftError.Code(rawValue: result),
        message: String(cString: stickshift_last_error_message())
    )
}
