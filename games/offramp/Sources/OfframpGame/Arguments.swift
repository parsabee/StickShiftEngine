// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// The `offramp` command line: the Rust demo's two flags (`main.rs`), plus the
// recorder (`examples/record_demo.rs`) and the parity trace
// (`examples/trace.rs`), which the Rust demo builds as separate examples.
//
// It parses as clap does for the Rust binary: flags in any order, `--name value`
// or `--name=value`, a flag given twice is an error, `-h`/`--help` prints the help
// and exits 0, and any error prints a message and the usage to stderr and exits 2.
// As in clap, an option never takes a next argument that looks like a flag (one
// that starts with `-` and is not `-` itself) as its value: `--record --smoke` is
// a missing value, and `--record=--smoke` is how to name such a directory.
// A single-dash argument is a cluster of short flags, so `-hx` prints the help
// and `-xh` is an unknown `-x`; `--` ends the options, and since there are no
// positionals, anything after it is unexpected.
// `--print-assets-dir` wins over the other modes, as it wins over `--smoke` in
// Rust; the other modes exclude each other.

/// What the `offramp` executable was asked to do.
public enum OfframpCommand: Equatable, Sendable {
    /// Play the game in a window.
    case windowed
    /// Run the headless smoke oracle and exit.
    case smoke
    /// Print the resolved asset root and exit.
    case printAssetsDir
    /// Record the autopilot run offscreen as `frames` PNG files in `directory`.
    case record(directory: String, frames: Int)
    /// Print the parity trace of `mode`, every line or only a summary.
    case trace(TraceMode, summary: Bool)
    /// Print the help and exit.
    case help
}

/// A command line `offramp` does not accept.
public struct UsageError: Error, Equatable, CustomStringConvertible {
    /// What is wrong, as one sentence.
    public let message: String

    /// What `offramp` prints to stderr before exiting with status 2, in clap's
    /// shape.
    public var description: String {
        "error: \(message)\n\nUsage: offramp [OPTIONS]\n\nFor more information, try '--help'.\n"
    }
}

/// The text `offramp --help` prints.
public let OFFRAMP_HELP = """
    Offramp: the Stick Shift Engine playable demo in Swift, over the C API

    Usage: offramp [OPTIONS]

    With no options, plays the game in a window.

    Options:
          --smoke             Run the headless smoke oracle: boot the game and drive a fixed frame loop, then exit 0
          --print-assets-dir  Print the resolved assets directory and exit (see `OFFRAMP_ASSETS`)
          --record <DIR>      Record the autopilot run offscreen (Metal) as numbered PNG frames in DIR, as record_demo does
          --frames <N>        With --record: how many frames to write [default: \(RecordDemo.defaultFrames)]
          --trace <MODE>      Print one JSON line of game state per frame of a headless run [possible values: smoke, autopilot, long]
          --summary           With --trace: print only every 600th line, the changes and a summary line
      -h, --help              Print help

    """

/// The long options `offramp` knows, and whether each takes a value.
private let LONG_OPTIONS: [String: Bool] = [
    "--smoke": false, "--print-assets-dir": false, "--summary": false, "--help": false,
    "--record": true, "--frames": true, "--trace": true,
]

/// `--name` of `--name=value`, and the value, for an argument that starts with
/// `--`.
private func splitLong(_ argument: String) -> (name: String, inline: String?) {
    guard argument.hasPrefix("--"), let equals = argument.firstIndex(of: "=") else {
        return (argument, nil)
    }
    return (String(argument[..<equals]), String(argument[argument.index(after: equals)...]))
}

/// The error clap reports for `next`, an argument that looks like a flag, when
/// it comes where an option's value was expected: an unknown flag, or a value
/// given to a flag that takes none. `nil` for a flag clap knows, whose turn
/// leaves the option without its value.
private func errorForFlag(_ next: String) -> UsageError? {
    if next == "--" {
        return nil
    }
    if next.hasPrefix("--") {
        let (name, inline) = splitLong(next)
        guard let takesValue = LONG_OPTIONS[name] else {
            return UsageError(message: "unexpected argument '\(name)' found")
        }
        if let inline, !takesValue {
            return UsageError(message: "unexpected value '\(inline)' for '\(name)' found; no more were expected")
        }
        return nil
    }
    // Short flags: only `-h` exists.
    let first = next.dropFirst().first.map(String.init) ?? ""
    return first == "h" ? nil : UsageError(message: "unexpected argument '-\(first)' found")
}

/// Parse `offramp`'s arguments (without the program name).
public func parseArguments(_ arguments: [String]) -> Result<OfframpCommand, UsageError> {
    var seen: Set<String> = []
    var smoke = false
    var printAssetsDir = false
    var summary = false
    var record: String?
    var frames: Int?
    var trace: TraceMode?

    var rest = arguments[...]
    while let argument = rest.popFirst() {
        if argument == "-h" || argument == "--help" {
            return .success(.help)
        }
        // `--` ends the options; what follows would be positionals, and there are
        // none, so clap names the first of them.
        if argument == "--" {
            if let next = rest.first {
                return .failure(UsageError(message: "unexpected argument '\(next)' found"))
            }
            break
        }
        // A single dash starts a cluster of short flags (`-hx` is `-h -x`), which
        // clap reads one at a time: `-h` prints the help at once, and any other
        // is unknown, named alone (`-x`).
        if argument.hasPrefix("-"), !argument.hasPrefix("--"), let first = argument.dropFirst().first {
            if first == "h" {
                return .success(.help)
            }
            return .failure(UsageError(message: "unexpected argument '-\(first)' found"))
        }
        // `--name=value` is `--name value`.
        let (name, inline) = splitLong(argument)
        if name == "--help", let inline {
            return .failure(UsageError(message: "unexpected value '\(inline)' for '--help' found; no more were expected"))
        }
        let display: String
        switch name {
        case "--smoke", "--print-assets-dir", "--summary": display = name
        case "--record": display = "--record <DIR>"
        case "--frames": display = "--frames <N>"
        case "--trace": display = "--trace <MODE>"
        default: return .failure(UsageError(message: "unexpected argument '\(name)' found"))
        }
        guard seen.insert(name).inserted else {
            return .failure(UsageError(message: "the argument '\(display)' cannot be used multiple times"))
        }

        /// The option's value: inline, or the next argument unless it looks like
        /// a flag.
        func value() -> Result<String, UsageError> {
            if let inline {
                return .success(inline)
            }
            let missing = UsageError(message: "a value is required for '\(display)' but none was supplied")
            guard let next = rest.first else {
                return .failure(missing)
            }
            if next.hasPrefix("-") && next != "-" {
                return .failure(errorForFlag(next) ?? missing)
            }
            rest.removeFirst()
            return .success(next)
        }

        switch name {
        case "--smoke", "--print-assets-dir", "--summary":
            if let inline {
                return .failure(UsageError(message: "unexpected value '\(inline)' for '\(name)' found; no more were expected"))
            }
            switch name {
            case "--smoke": smoke = true
            case "--print-assets-dir": printAssetsDir = true
            default: summary = true
            }
        case "--record":
            switch value() {
            case let .success(v) where !v.isEmpty: record = v
            case .success: return .failure(UsageError(message: "a value is required for '\(display)' but none was supplied"))
            case let .failure(e): return .failure(e)
            }
        case "--frames":
            switch value() {
            case let .success(v):
                guard let n = Int(v), n > 0 else {
                    return .failure(UsageError(message: "invalid value '\(v)' for '\(display)': a whole number of frames, at least 1"))
                }
                frames = n
            case let .failure(e): return .failure(e)
            }
        default:  // --trace
            switch value() {
            case let .success(v):
                guard let mode = TraceMode(rawValue: v) else {
                    let possible = TraceMode.allCases.map(\.rawValue).joined(separator: ", ")
                    return .failure(UsageError(message: "invalid value '\(v)' for '\(display)'\n  [possible values: \(possible)]"))
                }
                trace = mode
            case let .failure(e): return .failure(e)
            }
        }
    }

    if frames != nil && record == nil {
        return .failure(UsageError(message: "the argument '--frames <N>' can only be used with '--record <DIR>'"))
    }
    if summary && trace == nil {
        return .failure(UsageError(message: "the argument '--summary' can only be used with '--trace <MODE>'"))
    }
    let modes = [smoke ? "--smoke" : nil, record.map { _ in "--record <DIR>" }, trace.map { _ in "--trace <MODE>" }]
        .compactMap { $0 }
    if modes.count > 1 {
        return .failure(UsageError(message: "the argument '\(modes[0])' cannot be used with '\(modes[1])'"))
    }

    if printAssetsDir {
        return .success(.printAssetsDir)
    }
    if smoke {
        return .success(.smoke)
    }
    if let record {
        return .success(.record(directory: record, frames: frames ?? RecordDemo.defaultFrames))
    }
    if let trace {
        return .success(.trace(trace, summary: summary))
    }
    return .success(.windowed)
}
