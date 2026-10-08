# Contributing

Dot Calendar is a native Rust/Win32 app for Windows x64. Small, focused issues and pull requests are welcome.

## Build and check

Install Rust and a Windows resource compiler. For MSVC, use the Visual Studio C++ Build Tools and Windows SDK from a developer PowerShell session. For GNU, provide an x64 MinGW linker and `x86_64-w64-mingw32-windres`; `WINDRES` can select the resource compiler. The default local build script uses an existing optional `.tools` toolchain when present.

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
./scripts/build.ps1
./scripts/smoke.ps1
```

The smoke script uses isolated data beneath `qa-data`. For manual UI tests, set `DOT_CALENDAR_DATA_DIR` to a separate test directory and `DOT_CALENDAR_WINDOWED=1` for a normal window. Never use personal calendar data in tests, issues, screenshots, or commits.

## Design constraints

- Preserve the desktop attachment and `WS_EX_LAYERED` creation order. Verify that the calendar stays visible when showing the desktop.
- Keep idle work event-driven. Avoid resident servers, polling loops, duplicate note caches, or new frameworks for simple UI changes.
- Use stable event IDs and the store's conflict checks. Opening or cancelling a view must not change the calendar.
- Preserve Google visibility filtering, recurrence semantics, and retry IDs.
- For UI changes, verify empty and crowded dates, long Korean text, keyboard navigation, and more than 100 entries.
- Validate Google account or device sync explicitly before claiming that it works with a live account.

## Website

The GitHub Pages site is plain HTML, CSS, and JavaScript in `web/`. It needs no build step or external font service. Use fictional events in demos. The Pages workflow publishes only this directory; release ZIP files are hosted by GitHub Releases.

Contributions are made under the project's MIT License. Third-party dependencies retain their own licenses.
