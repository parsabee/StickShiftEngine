// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import Testing

@testable import OfframpGame

/// The `offramp` command line: the Rust binary's flags as clap parses them, and
/// the recorder's and the trace's.
@Suite("command line")
struct ArgumentsTests {
    private func parse(_ arguments: String...) -> Result<OfframpCommand, UsageError> {
        parseArguments(arguments)
    }

    /// The message of a refused command line.
    private func error(_ arguments: String...) -> String? {
        if case let .failure(e) = parseArguments(arguments) {
            return e.message
        }
        return nil
    }

    @Test func noOptionsPlaysInAWindow() {
        #expect(parse() == .success(.windowed))
    }

    @Test func theRustFlags() {
        #expect(parse("--smoke") == .success(.smoke))
        #expect(parse("--print-assets-dir") == .success(.printAssetsDir))
        // `--print-assets-dir` wins over `--smoke`, whichever comes first (main.rs:122-130).
        #expect(parse("--print-assets-dir", "--smoke") == .success(.printAssetsDir))
        #expect(parse("--smoke", "--print-assets-dir") == .success(.printAssetsDir))
        #expect(parse("-h") == .success(.help))
        #expect(parse("--help") == .success(.help))
        #expect(parse("--smoke", "--help") == .success(.help))
    }

    @Test func theRecorder() {
        #expect(parse("--record", "out") == .success(.record(directory: "out", frames: 80)))
        #expect(parse("--record=out") == .success(.record(directory: "out", frames: 80)))
        #expect(parse("--record", "out", "--frames", "3") == .success(.record(directory: "out", frames: 3)))
        #expect(parse("--frames=5", "--record", "a b") == .success(.record(directory: "a b", frames: 5)))
        #expect(RecordDemo.defaultFrames == 80, "record_demo writes every third of 240 frames")
    }

    @Test func theTrace() {
        for mode in TraceMode.allCases {
            #expect(parse("--trace", mode.rawValue) == .success(.trace(mode, summary: false)))
        }
        #expect(parse("--trace", "long", "--summary") == .success(.trace(.long, summary: true)))
        #expect(parse("--summary", "--trace=smoke") == .success(.trace(.smoke, summary: true)))
    }

    @Test func refusedCommandLines() {
        #expect(error("--bogus") == "unexpected argument '--bogus' found")
        #expect(error("--version") == "unexpected argument '--version' found", "no --version, as in Rust")
        #expect(error("smoke") == "unexpected argument 'smoke' found")
        #expect(error("--smoke", "--smoke") == "the argument '--smoke' cannot be used multiple times")
        #expect(error("--smoke=1") == "unexpected value '1' for '--smoke' found; no more were expected")
        #expect(error("--record") == "a value is required for '--record <DIR>' but none was supplied")
        #expect(error("--record=") == "a value is required for '--record <DIR>' but none was supplied")
        #expect(error("--trace") == "a value is required for '--trace <MODE>' but none was supplied")
        #expect(error("--trace", "fast") == "invalid value 'fast' for '--trace <MODE>'\n  [possible values: smoke, autopilot, long]")
        #expect(error("--record", "d", "--frames", "0")?.hasPrefix("invalid value '0' for '--frames <N>'") == true)
        #expect(error("--record", "d", "--frames", "x")?.hasPrefix("invalid value 'x' for '--frames <N>'") == true)
        #expect(error("--frames", "3") == "the argument '--frames <N>' can only be used with '--record <DIR>'")
        #expect(error("--summary") == "the argument '--summary' can only be used with '--trace <MODE>'")
        #expect(error("--smoke", "--trace", "long") == "the argument '--smoke' cannot be used with '--trace <MODE>'")
        #expect(error("--record", "d", "--trace", "long") == "the argument '--record <DIR>' cannot be used with '--trace <MODE>'")
        #expect(error("--bogus=3") == "unexpected argument '--bogus' found")
        #expect(error("--help=1") == "unexpected value '1' for '--help' found; no more were expected")
    }

    @Test func anOptionDoesNotTakeAFlagAsItsValue() {
        // What clap 4 reports (checked against a clap binary declaring the same
        // options): a flag it knows leaves the option without a value...
        #expect(error("--record", "--smoke") == "a value is required for '--record <DIR>' but none was supplied")
        #expect(error("--frames", "--record", "x") == "a value is required for '--frames <N>' but none was supplied")
        #expect(error("--trace", "--summary") == "a value is required for '--trace <MODE>' but none was supplied")
        #expect(error("--record", "--record") == "a value is required for '--record <DIR>' but none was supplied")
        #expect(error("--record", "--frames=3") == "a value is required for '--record <DIR>' but none was supplied")
        #expect(error("--record", "--help") == "a value is required for '--record <DIR>' but none was supplied")
        #expect(error("--record", "-h") == "a value is required for '--record <DIR>' but none was supplied")
        #expect(error("--record", "--") == "a value is required for '--record <DIR>' but none was supplied")
        // ...one it does not know, or a value for a flag that takes none, is that error.
        #expect(error("--record", "--bogus") == "unexpected argument '--bogus' found")
        #expect(error("--record", "--bogus=3") == "unexpected argument '--bogus' found")
        #expect(error("--record", "-x") == "unexpected argument '-x' found")
        #expect(error("--record", "d", "--frames", "-5") == "unexpected argument '-5' found")
        #expect(error("--record", "--smoke=1") == "unexpected value '1' for '--smoke' found; no more were expected")
        // Such a value is given inline; a lone `-` is a value.
        #expect(parse("--record=--smoke") == .success(.record(directory: "--smoke", frames: 80)))
        #expect(parse("--record", "-") == .success(.record(directory: "-", frames: 80)))
        #expect(error("--trace", "-") == "invalid value '-' for '--trace <MODE>'\n  [possible values: smoke, autopilot, long]")
    }

    @Test func aSingleDashIsAClusterOfShortFlags() {
        // clap reads `-hx` as `-h -x` and acts on `-h` as soon as it reaches it
        // (checked against `offramp` and a clap binary declaring the same
        // options)...
        #expect(parse("-hh") == .success(.help))
        #expect(parse("-hx") == .success(.help))
        #expect(parse("-h1") == .success(.help))
        #expect(parse("-h=1") == .success(.help))
        #expect(parse("--trace", "long", "-hh") == .success(.help))
        #expect(parse("-hh", "--bogus") == .success(.help))
        // ...after any error before it...
        #expect(error("--bogus", "-hh") == "unexpected argument '--bogus' found")
        #expect(error("--smoke", "--smoke", "-h") == "the argument '--smoke' cannot be used multiple times")
        // ...and names any other short flag alone.
        #expect(error("-xh") == "unexpected argument '-x' found")
        #expect(error("-smoke") == "unexpected argument '-s' found")
        #expect(error("-5") == "unexpected argument '-5' found")
        #expect(error("-") == "unexpected argument '-' found", "a lone dash is a positional")
    }

    @Test func aDoubleDashEndsTheOptions() {
        // There are no positionals, so `--` changes nothing at the end of a line...
        #expect(parse("--") == .success(.windowed))
        #expect(parse("--smoke", "--") == .success(.smoke))
        #expect(parse("--print-assets-dir", "--") == .success(.printAssetsDir))
        #expect(parse("--record", "d", "--") == .success(.record(directory: "d", frames: 80)))
        #expect(error("--frames", "3", "--") == "the argument '--frames <N>' can only be used with '--record <DIR>'")
        // ...and anything after it is unexpected, named whole.
        #expect(error("--", "--smoke") == "unexpected argument '--smoke' found")
        #expect(error("--smoke", "--", "x") == "unexpected argument 'x' found")
        #expect(error("--", "-hh") == "unexpected argument '-hh' found")
        #expect(error("--", "--") == "unexpected argument '--' found")
    }

    @Test func aRefusalPrintsInClapsShape() {
        // The Rust binary's (`offramp --bogus`), with the name changed.
        #expect(
            UsageError(message: "unexpected argument '--bogus' found").description
                == "error: unexpected argument '--bogus' found\n\nUsage: offramp [OPTIONS]\n\nFor more information, try '--help'.\n"
        )
        #expect(OFFRAMP_HELP.hasPrefix("Offramp: "))
        #expect(OFFRAMP_HELP.contains("Usage: offramp [OPTIONS]\n"))
        for option in ["--smoke", "--print-assets-dir", "--record <DIR>", "--frames <N>", "--trace <MODE>", "--summary", "-h, --help"] {
            #expect(OFFRAMP_HELP.contains(option), "\(option) is in the help")
        }
    }
}
