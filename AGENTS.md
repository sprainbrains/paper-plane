# Paper Plane contributor guide

## Project overview

Paper Plane is a Telegram desktop client written in Rust with GTK 4,
libadwaita, Blueprint, and TDLib. The application uses GObject subclasses and
properties throughout its model and UI layers.

- `src/model/` owns TDLib-backed application state and `GListModel`
  implementations.
- `src/ui/` contains Rust widget subclasses and matching `.blp` templates.
- `src/automation.rs` and `docs/dev-automation.md` provide the Devel-only D-Bus
  bridge for driving and observing the running application.
- `data/resources/` contains styles, icons, and other packaged resources.
- `build-aux/app.drey.PaperPlane.Devel.json` is the Flatpak development
  manifest used by CI and GNOME Builder.

## Building and validation

Meson is the authoritative build entry point because it compiles Blueprint
resources, generates `src/config.rs`, and invokes Cargo with the required
environment. Prefer a fresh build directory if an existing one was generated
by an incompatible Meson version.

```sh
meson setup _build -Dprofile=development
meson compile -C _build
meson test -C _build --print-errorlogs
cargo +nightly fmt --all -- --check
```

The Meson test suite runs Rust tests and Clippy with warnings denied. For UI
changes, a clean build is only the first gate: exercise the affected workflow
in a Devel Flatpak. Use the automation bridge's act/screenshot/inspect loop
where it covers the behavior, and use Telegram's test data center rather than a
real account for automated login testing.

## Code conventions

- Follow `.editorconfig`: Rust uses four-space indentation; Blueprint, Meson,
  YAML, and CSS use two spaces.
- Keep each Rust widget paired with its Blueprint template and register new
  resources through the existing `src/ui` resource structure.
- Use GObject properties and notifications consistently. Emit explicit notify
  signals only after the backing value has changed, and disconnect stored
  signal handlers when retargeting or disposing an object.
- Update mutable `GListModel` storage before emitting `items-changed`. The
  reported position, removed count, and added count must exactly describe the
  changed range. Preserve object identity for moves when consumers bind to the
  item objects.
- Avoid holding `RefCell` borrows while calling code that emits signals,
  notifies properties, awaits, or can otherwise re-enter the object.
- Use weak captures for long-lived GTK signal handlers and spawned futures when
  the owner should not be kept alive by the callback.
- Keep user-visible strings translatable with the existing gettext helpers.
- Do not hand-edit generated `src/config.rs`.

## Changes and commits

Preserve unrelated working-tree changes. Keep fixes focused and add regression
coverage for model/state bugs when the behavior can be isolated from GTK.
Runtime rendering and interaction are required validation for UI fixes.

Commits follow Conventional Commits, with the description after the colon
capitalized, for example: `fix: Preserve chat rows while reordering`.
