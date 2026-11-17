// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import CStickShift

// The C engine behind an `Engine` or an `EngineContext`, for code that makes a C
// call the Swift API does not wrap: the package's `StickShiftAppKit` host, and
// examples. Not public API: `@_spi(CInterop) import StickShift` reaches it, and
// it may change or go without notice. A call through it follows the C API's
// rules (`docs/C_API.md`): the same scopes, the same thread.

extension Engine {
    /// The C engine (`StickShiftEngine *`), valid while this engine lives.
    @_spi(CInterop) public var cEngine: OpaquePointer { raw }
}

extension EngineContext {
    /// The C engine as this hook sees it (`StickShiftEngine *`), valid only
    /// during the hook, as the context is.
    @_spi(CInterop) public var cEngine: OpaquePointer {
        handle.check("EngineContext.cEngine")
        return handle.raw
    }
}
