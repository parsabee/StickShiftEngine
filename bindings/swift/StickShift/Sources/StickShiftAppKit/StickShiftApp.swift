// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

import AppKit
import StickShift

/// A macOS app that runs one game in one window, as the Rust runner
/// (`stickshift_core::run`) runs a Rust game.
///
/// ```swift
/// import StickShiftAppKit
///
/// StickShiftApp.run(title: "Cubes") { Cubes() }
/// ```
public enum StickShiftApp {
    /// Run `makeGame`'s game in a window until the player quits. Never returns.
    ///
    /// - An executable outside a bundle becomes a regular application, with a
    ///   Dock icon and a menu bar; a bundled one keeps its `Info.plist`'s
    ///   policy (`LSUIElement`, `LSBackgroundOnly`). Either way it takes the
    ///   keyboard from whatever launched it (a terminal included). All as winit
    ///   does.
    /// - The menu bar has winit's application menu (About, Services, Hide, Hide
    ///   Others, Show All, Quit), and the engine's Graphics menu
    ///   (``GraphicsMenuBar``) once the game has initialized.
    /// - The window is `title`d, `size` points, closable, miniaturizable and
    ///   resizable, and centred. Its content is an ``EngineView``, whose engine
    ///   has `options`: by default ``Engine/Options/app`` (the sound device,
    ///   the player's graphics settings restored and saved where the Rust
    ///   runner keeps them, and Escape quitting).
    /// - The game's ``Game/load(_:)`` phase runs, then the view's
    ///   ``EngineView/run()``: the game's ``Game/initialize(_:)``, then the frames.
    /// - Closing the window, or Escape with ``Engine/Options/quitsOnEscape``,
    ///   quits, as does the game's ``EngineContext/requestExit()``.
    ///
    /// An error making the game, the view or the engine, or from the game's
    /// `load` or `initialize`, is printed to standard error, and the process
    /// exits with status 1.
    @MainActor
    public static func run<G: Game>(
        title: String,
        size: CGSize = CGSize(width: 800, height: 600),
        options: Engine.Options = .app,
        makeGame: @escaping @MainActor () throws -> G
    ) -> Never {
        let app = NSApplication.shared
        let delegate = AppDelegate(title: title, size: size, options: options) { size, options in
            try EngineView(size: size, game: makeGame(), options: options)
        }
        app.delegate = delegate
        // The application keeps its delegate weakly.
        withExtendedLifetime(delegate) {
            app.run()
        }
        exit(0)
    }

    /// Whether the app sets the regular activation policy: only outside a
    /// bundle (no `bundleIdentifier`), as winit does with no policy given, so
    /// a bundle's `LSUIElement` or `LSBackgroundOnly` is kept.
    static func setsRegularPolicy(bundleIdentifier: String?) -> Bool {
        bundleIdentifier == nil
    }

    /// winit's application menu (`platform_impl/macos/menu.rs`): About,
    /// Services, Hide, Hide Others, Show All and Quit, titled with the
    /// process's name. Its Services submenu becomes the application's.
    @MainActor
    static func applicationMenu(named name: String = ProcessInfo.processInfo.processName) -> NSMenu {
        let bar = NSMenu()
        let appItem = NSMenuItem()
        bar.addItem(appItem)

        let menu = NSMenu()
        menu.addItem(
            withTitle: "About \(name)", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
            keyEquivalent: ""
        )
        menu.addItem(.separator())
        let services = NSMenu()
        let servicesItem = NSMenuItem(title: "Services", action: nil, keyEquivalent: "")
        servicesItem.submenu = services
        menu.addItem(servicesItem)
        menu.addItem(withTitle: "Hide \(name)", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let hideOthers = menu.addItem(
            withTitle: "Hide Others", action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h"
        )
        hideOthers.keyEquivalentModifierMask = [.option, .command]
        menu.addItem(
            withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: ""
        )
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit \(name)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = menu

        NSApp.servicesMenu = services
        return bar
    }

    /// The window `StickShiftApp` runs a game in: `title`d, `size` points,
    /// titled, closable, miniaturizable and resizable.
    @MainActor
    static func makeWindow(title: String, size: CGSize) -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = title
        window.isReleasedWhenClosed = false
        return window
    }
}

/// The application's delegate and the window's: what `StickShiftApp.run` owns.
@MainActor
private final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private let title: String
    private let size: CGSize
    private let options: Engine.Options
    private let makeView: @MainActor (CGSize, Engine.Options) throws -> EngineView
    private var window: NSWindow?
    private var view: EngineView?

    init(
        title: String, size: CGSize, options: Engine.Options,
        makeView: @escaping @MainActor (CGSize, Engine.Options) throws -> EngineView
    ) {
        self.title = title
        self.size = size
        self.options = options
        self.makeView = makeView
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // An executable outside a bundle gets no menu bar, Dock icon or
        // keyboard unless it asks to be a regular app; a bundled one keeps the
        // policy its Info.plist gives it. Both as winit does, with no policy
        // set (`app_state.rs`). Then it takes the keyboard from whatever
        // launched it, as winit does (`activateIgnoringOtherApps:YES`): the
        // cooperative `activate()` is refused when a terminal, which never
        // yields, started the process.
        if StickShiftApp.setsRegularPolicy(bundleIdentifier: NSRunningApplication.current.bundleIdentifier) {
            NSApp.setActivationPolicy(.regular)
        }
        NSApp.activate(ignoringOtherApps: true)
        NSApp.mainMenu = StickShiftApp.applicationMenu()

        let window = StickShiftApp.makeWindow(title: title, size: size)
        let view: EngineView
        do {
            view = try makeView(size, options)
        } catch {
            fail(error)
        }
        view.onExit = { NSApp.terminate(nil) }
        window.contentView = view
        window.delegate = self
        window.center()
        self.window = window
        self.view = view
        window.makeKeyAndOrderFront(nil)

        Task { await self.start() }
    }

    /// The game's load phase, then its initialize and the frames; the
    /// Graphics menu goes in once the game has initialized, as in Rust.
    private func start() async {
        guard let view else { return }
        do {
            try await view.engine.load()
            try view.run()
        } catch {
            fail(error)
        }
        if !view.hasExited, let menuBar = NSApp.mainMenu {
            view.makeMenuBar().install(in: menuBar)
        }
    }

    /// Closing the window asks the engine, which always answers exit: the
    /// view's ``EngineView/onExit`` terminates the app.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        view?.push(.close)
        return true
    }

    func windowWillClose(_ notification: Notification) {
        NSApp.terminate(nil)
    }

    private func fail(_ error: any Error) -> Never {
        FileHandle.standardError.write(Data("error: \(error)\n".utf8))
        exit(1)
    }
}
