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

## Virtualized lists and off-screen work

`GtkListView`/`GtkGridView` virtualize, but you cannot assume an off-screen row
is cheap or gone. Two behaviors bite hard:

- On its **first layout** a `GtkListView` has no realized rows, so its unknown
  row-height estimate is `0` (median of an empty set). Every row is then placed
  at `y = 0`, the visible range spans the *entire* model, and it realizes **all**
  rows in one pass. This is normally transient, but with an incrementally
  populated model — and a `ScrolledWindow` that absorbs the list's growth, so it
  is never re-allocated to trigger a cull — it can stay massively over-realized.
- A row that is recycled or scrolled away may be **unbound but not immediately
  disposed** (held briefly by GTK internals or in-flight work). It is unparented,
  hence **unmapped**, but still alive.

**Rule: gate all per-row work on `map`/`unmap`, never on construct/dispose.**
Start preview decodes, media downloads, `add_tick_callback` animation, and any
recurring work when the row is **mapped**, and stop/cancel it on **unmap** (a
tick callback should check `is_mapped()` and return `Break`; async tasks should
check a shared demand flag that `unmap` clears). An off-screen cell must do zero
work. Do not start expensive work in a widget's constructor, and do not rely on
`dispose` (a leaked row never disposes) or on the ListView culling to bound cost.
This mirrors Telegram Desktop, where only on-screen cells load and animate. See
`StickerPickerRow` (`start_row_work`/`stop_row_work`) for the reference pattern.

Debugging note: overriding `WidgetImpl::size_allocate` on a widget that uses a
template `layout-manager` may not fire as expected — probe from a widget that is
actually in the tree (e.g. `connect_map`) or walk ancestors, not from an
allocation override.

## Changes and commits

Preserve unrelated working-tree changes. Keep fixes focused and add regression
coverage for model/state bugs when the behavior can be isolated from GTK.
Runtime rendering and interaction are required validation for UI fixes.

Commits follow Conventional Commits, with the description after the colon
capitalized, for example: `fix: Preserve chat rows while reordering`.
