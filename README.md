# Blaze IDE

A fast, agentic code editor powered by [BlazeAPI](https://blazeapi.org). One codebase builds native apps for macOS (Apple Silicon and Intel) and Windows.

- **Every BlazeAPI model** your account can use, with live prices, in Ask, Plan and Agent modes.
- **Sign in with BlazeAPI**: the browser does the sign-in; the app gets its own revocable key, kept in Windows Credential Manager, or on macOS in a private file in the app's settings folder readable only by your account (the Keychain asked for your password after every update of these builds, which are not Developer ID signed). Usage is billed to your BlazeAPI balance.
- Monaco editor, integrated terminal, project search and replace, live web preview with element comments, AI image and video generation into your project.
- **`blaze` in the terminal**: the same agent, models and chats in a full-screen terminal chat, or `blaze -p "…"` in scripts. Ships inside the app (Settings > Command line) and as its own download for macOS, Windows and Linux.
- **Continue on mobile**: open your chats on your phone, send messages and approve the agent's actions while away. Password sign-in (or a one-time QR), reachable over Wi-Fi, your own domain, a free Cloudflare quick tunnel, or the BlazeAPI relay.
- **BlBots**: a roster of AI bots, each with its own personality, model, folder, memory and routines; rooms where bots work together; bots on BlazeAPI models or on your own Claude Code / Codex / Grok CLI; an optional private Linux desktop per bot (Docker). In the IDE, the CLI and on your phone.

## Layout

| Path | What |
|---|---|
| `crates/blaze-core` | All logic in Rust, no GUI: project sandbox, files, search, terminal (PTY), file watching, BlazeAPI sign-in (OAuth + PKCE), models, AI streaming, agent tools. Unit-tested on every OS. |
| `crates/blaze-core/src/agent.rs` | The agent loop in Rust (used by the CLI, phone turns and BlBots). |
| `crates/blaze-core/src/remote` | Continue on mobile: phone server, sign-in, tunnels, relay client, the phone web app (`web/`). |
| `crates/blaze-core/src/bots` | BlBots: roster, rooms, routines, CLI engines, private desktop. |
| `crates/blaze-cli` | The `blaze` command. |
| `relay/` | The BlazeAPI relay server for Continue on mobile (m.blazeapi.org). |
| `src-tauri` | Tauri 2 shell: windows, menus, keychain, the `blazemedia:` file protocol, and the command layer the UI calls. |
| `src/renderer` | The UI (TypeScript, no framework). `bridge.ts` is its only door to the system. |
| `src/shared` | Types and the `VoidAPI` contract shared by UI and bridge. |

## Security model

- The UI can only call the commands listed in `src-tauri/build.rs` and granted in `src-tauri/capabilities/main.json`. No filesystem, shell or HTTP plugin is exposed to the page.
- Every path from the UI or the AI goes through `blaze-core::paths::Sandbox`: it must be inside a folder the user opened, after resolving `..` and symlinks.
- Deleting moves files to the OS trash. Agent shell commands run in the project root with a timeout and need approval unless auto-approve is on.
- Strict Content Security Policy (no inline or remote scripts). Navigation away from the app is blocked; links open in the browser.
- The BlazeAPI key never reaches the UI and is never written to disk by the app. Release builds talk to `https://blazeapi.org` only, over HTTPS (rustls).
- CI runs `cargo audit`, `npm audit`, clippy and the test suites, and the build fails if anything shaped like a credential lands in `dist/`.

## Develop

Requirements: Node 22, Rust stable, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```sh
npm ci
npm run dev        # run the app
cargo test -p blaze-core -p blaze-cli
cargo run -p blaze-cli -- --help   # the blaze command
npm run dist       # build installers (dmg / nsis + msi)
```

Debug builds can point at another backend with `BLAZE_IDE_API_BASE=http://127.0.0.1:7173`.

## Releases

CI builds a universal macOS `.dmg` and Windows `.exe`/`.msi` on every push. Code signing (Apple Developer ID + notarization, Windows Authenticode) is not configured yet, so unsigned builds show a Gatekeeper/SmartScreen warning.

## License

Apache License 2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE). You may use, modify and redistribute Blaze IDE, but you must keep the copyright and NOTICE credit to blazfxx / Blaze IDE, mark your changes, and may not present your fork as the official Blaze IDE or use the Blaze name or logo for it.

The optional ambient background track bundled with official releases is not part of this repository, so ambient audio is silent in a source build until you add `src/renderer/assets/babft-ambient.ogg` or pick your own file in Settings. Releases are built and signed privately.
