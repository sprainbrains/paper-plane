#!/usr/bin/env bash
#
# Development automation driver for Paper Plane (Devel builds only).
#
# Drives the running Devel flatpak over its D-Bus action group and reads back
# artifacts (screenshots, message dumps, the chat list) from the app cache dir.
# This is the host-side counterpart to `src/automation.rs`; together they form a
# headless act -> screenshot -> inspect loop for development.
#
# Usage:
#   build-aux/dev-drive.sh run-headless        # start the app under broadway (no display)
#   build-aux/dev-drive.sh screenshot [OUT]    # capture window -> OUT (default: /tmp/pp-shot.png)
#   build-aux/dev-drive.sh list-chats          # dump loaded chats -> stdout
#   build-aux/dev-drive.sh describe MSG CHAT    # dump a message's full TDLib object -> stdout
#   build-aux/dev-drive.sh new-test-client     # begin a test-DC login
#   build-aux/dev-drive.sh phone NUMBER        # submit phone number (test-DC: e.g. 9996612345)
#   build-aux/dev-drive.sh code CODE           # submit login code   (test-DC: DC digit x5, e.g. 11111)
#   build-aux/dev-drive.sh password PW         # submit 2FA password
#   build-aux/dev-drive.sh open CHAT           # open a chat by id
#   build-aux/dev-drive.sh send CHAT TEXT      # send a text message
#   build-aux/dev-drive.sh react CHAT MSG EMOJI# add a reaction
#
# Environment:
#   APP_ID   application id (default app.drey.PaperPlane.Devel)

set -euo pipefail

APP_ID="${APP_ID:-app.drey.PaperPlane.Devel}"
OBJ_PATH="/$(echo "$APP_ID" | tr '.' '/')"
CACHE_DIR="$HOME/.var/app/$APP_ID/cache/automation"

activate() {
    # $1 = action name, $2 = GVariant parameter array text (e.g. "[<int64 5>]"), default "[]"
    gdbus call --session \
        --dest "$APP_ID" \
        --object-path "$OBJ_PATH" \
        --method org.gtk.Actions.Activate \
        "$1" "${2:-[]}" "{}" >/dev/null
}

# Wait for an artifact file to be (re)written, then cat/copy it.
wait_for() {
    # $1 = file under CACHE_DIR, $2 = timeout seconds
    local f="$CACHE_DIR/$1" deadline=$(( SECONDS + ${2:-5} ))
    local before=0
    [ -f "$f" ] && before=$(stat -c %Y "$f" 2>/dev/null || echo 0)
    while [ "$SECONDS" -lt "$deadline" ]; do
        if [ -f "$f" ]; then
            local now
            now=$(stat -c %Y "$f" 2>/dev/null || echo 0)
            [ "$now" -gt "$before" ] && { echo "$f"; return 0; }
        fi
        sleep 0.2
    done
    echo "timed out waiting for $f" >&2
    return 1
}

cmd="${1:-}"; shift || true
case "$cmd" in
    run-headless)
        # Start a headless broadway display and launch the Devel app on it.
        broadwayd :5 &
        sleep 1
        GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 \
            flatpak run "$APP_ID" &
        echo "app starting under broadway :5 (pid $!)"
        ;;
    screenshot)
        out="${1:-/tmp/pp-shot.png}"
        activate auto-screenshot
        f=$(wait_for screenshot.png 8)
        cp "$f" "$out"
        echo "$out"
        ;;
    list-chats)
        activate auto-list-chats
        f=$(wait_for chats.txt 5)
        cat "$f"
        ;;
    describe)
        msg="$1"; chat="$2"
        activate auto-describe-message "[<(int64 $chat, int64 $msg)>]"
        f=$(wait_for message.txt 8)
        cat "$f"
        ;;
    new-test-client) activate auto-new-test-client ;;
    phone)    activate auto-submit-phone    "[<'$1'>]" ;;
    code)     activate auto-submit-code     "[<'$1'>]" ;;
    password) activate auto-submit-password "[<'$1'>]" ;;
    open)     activate auto-open-chat       "[<int64 $1>]" ;;
    send)     activate auto-send-text       "[<(int64 $1, '$2')>]" ;;
    react)    activate auto-react           "[<(int64 $1, int64 $2, '$3')>]" ;;
    *)
        grep '^#' "$0" | sed 's/^# \{0,1\}//'
        exit 1
        ;;
esac
