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
#   build-aux/dev-drive.sh list-messages CHAT  # dump newest messages of a chat -> stdout
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
GDBUS="${GDBUS:-gdbus}"

variant_string() {
    local value="${1-}" backslash=$'\\'
    value=${value//"$backslash"/"$backslash$backslash"}
    value=${value//\'/\\\'}
    value=${value//$'\n'/\\n}
    value=${value//$'\r'/\\r}
    value=${value//$'\t'/\\t}
    printf "'%s'" "$value"
}

variant_int64() {
    if [[ ! ${1-} =~ ^-?[0-9]+$ ]]; then
        echo "invalid int64: ${1-}" >&2
        return 1
    fi
    printf 'int64 %s' "$1"
}

activate() {
    # $1 = action name, $2 = GVariant parameter array text (e.g. "[<int64 5>]"), default "[]"
    "$GDBUS" call --session \
        --dest "$APP_ID" \
        --object-path "$OBJ_PATH" \
        --method org.gtk.Actions.Activate \
        "$1" "${2:-[]}" "{}" >/dev/null
}

# Remove an old artifact before activating its producer. Actions may complete
# synchronously, so freshness must be established before the D-Bus call.
request_artifact() {
    # $1 = file, $2 = timeout, $3 = action, $4 = optional parameters
    rm -f -- "$CACHE_DIR/$1"
    activate "$3" "${4:-[]}" || return
    wait_for "$1" "$2"
}

# Wait for an artifact file to be written, then return its path.
wait_for() {
    # $1 = file under CACHE_DIR, $2 = timeout seconds
    local f="$CACHE_DIR/$1" deadline=$(( SECONDS + ${2:-5} ))
    while [ "$SECONDS" -lt "$deadline" ]; do
        [ -f "$f" ] && { echo "$f"; return 0; }
        sleep 0.2
    done
    echo "timed out waiting for $f" >&2
    return 1
}

cmd="${1:-}"; shift || true
case "$cmd" in
    run-headless)
        # Start a headless broadway display and launch the Devel app on it.
        # Requires GTK built with -Dbroadway-backend=true (see the manifest).
        broadwayd :5 &
        sleep 1
        flatpak run \
            --filesystem="$XDG_RUNTIME_DIR" \
            --env=GDK_BACKEND=broadway \
            --env=BROADWAY_DISPLAY=:5 \
            "$APP_ID" &
        echo "app starting under broadway :5 (pid $!)"
        ;;
    screenshot)
        out="${1:-/tmp/pp-shot.png}"
        f=$(request_artifact screenshot.png 8 auto-screenshot)
        cp "$f" "$out"
        echo "$out"
        ;;
    list-chats)
        f=$(request_artifact chats.txt 5 auto-list-chats)
        cat "$f"
        ;;
    list-messages)
        chat=$(variant_int64 "$1")
        f=$(request_artifact messages.txt 8 auto-list-messages "[<$chat>]")
        cat "$f"
        ;;
    describe)
        msg=$(variant_int64 "$1"); chat=$(variant_int64 "$2")
        f=$(request_artifact message.txt 8 auto-describe-message "[<($chat, $msg)>]")
        cat "$f"
        ;;
    new-test-client) activate auto-new-test-client ;;
    phone)    value=$(variant_string "$1"); activate auto-submit-phone "[<$value>]" ;;
    code)     value=$(variant_string "$1"); activate auto-submit-code "[<$value>]" ;;
    password) value=$(variant_string "$1"); activate auto-submit-password "[<$value>]" ;;
    open)     chat=$(variant_int64 "$1"); activate auto-open-chat "[<$chat>]" ;;
    send)
        chat=$(variant_int64 "$1"); text=$(variant_string "$2")
        activate auto-send-text "[<($chat, $text)>]"
        ;;
    react)
        chat=$(variant_int64 "$1"); msg=$(variant_int64 "$2"); emoji=$(variant_string "$3")
        activate auto-react "[<($chat, $msg, $emoji)>]"
        ;;
    *)
        grep '^#' "$0" | sed 's/^# \{0,1\}//'
        exit 1
        ;;
esac
