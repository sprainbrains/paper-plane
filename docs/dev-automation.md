# Development automation bridge

Paper Plane's **Devel** builds ship a small in-app automation bridge that lets a
developer (or an AI assistant) drive the running client headlessly and inspect
what it renders — an *act → screenshot → inspect* loop, without a mouse or a
visible display.

It is **compiled into every build but only activated on the `Devel` profile**
(`config::PROFILE == "Devel"`); release builds register nothing.

## How it works

- `src/automation.rs` registers a set of `GAction`s on the `GApplication`.
  Because GTK automatically exports an application's action group on the session
  bus (`org.gtk.Actions`), these actions are invokable over D-Bus at the object
  path derived from the app id — `/app/drey/PaperPlane/Devel`.
- Actions that produce output write it to the app cache dir under
  `automation/`, which on the host is
  `~/.var/app/app.drey.PaperPlane.Devel/cache/automation/`:
  - `screenshot.png` — the current window, rendered via `GskRenderer`
    (works headless — no compositor screenshot tool needed).
  - `chats.txt` — `<id>\t<title>` per loaded chat.
  - `message.txt` — a message's full `getMessage` result, `Debug`-formatted
    (the same data as the "Copy Debug Entry" context-menu action).
- `build-aux/dev-drive.sh` is the host-side wrapper around `gdbus` + reading
  those artifacts.

## Running headless

```sh
# 1. Build & install the Devel flatpak.
flatpak-builder --user --install --force-clean _build_flatpak \
    build-aux/app.drey.PaperPlane.Devel.json

# 2. Launch it on a headless Broadway display.
build-aux/dev-drive.sh run-headless

# 3. Log in on Telegram's test data center (no real account needed).
build-aux/dev-drive.sh new-test-client
build-aux/dev-drive.sh screenshot           # verify we're at the phone screen
build-aux/dev-drive.sh phone 9996612345     # test-DC number: 99966 + DC digit + 4 digits
build-aux/dev-drive.sh code 11111           # test-DC code: the DC digit repeated 5x
# build-aux/dev-drive.sh password ...        # only if the account has 2FA

# 4. Drive & observe.
build-aux/dev-drive.sh list-chats
build-aux/dev-drive.sh open 123456789
build-aux/dev-drive.sh send 123456789 "hello from automation"
build-aux/dev-drive.sh screenshot /tmp/after-send.png
build-aux/dev-drive.sh react 123456789 987654321 👍
build-aux/dev-drive.sh describe 987654321 123456789
```

## Action reference

| Action | Parameter | Effect |
| --- | --- | --- |
| `auto-screenshot` | — | render window → `screenshot.png` |
| `auto-list-chats` | — | loaded chats → `chats.txt` |
| `auto-describe-message` | `(xx)` chat, message | full message object → `message.txt` |
| `auto-new-test-client` | — | begin a test-DC login |
| `auto-submit-phone` | `s` | submit phone number |
| `auto-submit-code` | `s` | submit login code |
| `auto-submit-password` | `s` | submit 2FA password |
| `auto-open-chat` | `x` chat | open a chat |
| `auto-send-text` | `(xs)` chat, text | send a text message |
| `auto-react` | `(xxs)` chat, message, emoji | add a reaction |

## Test data center

Telegram's test DC accepts synthetic phone numbers of the form `99966X YYYY`
where `X` is the DC id (1–3) and `YYYY` is arbitrary; the login code is the DC
digit repeated five times (e.g. DC 1 → `11111`). This lets the login flow run
fully unattended against isolated infrastructure, never touching a real account.
