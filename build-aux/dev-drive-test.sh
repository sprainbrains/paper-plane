#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
DRIVER="$SCRIPT_DIR/dev-drive.sh"
TEST_HOME=$(mktemp -d)
trap 'rm -rf -- "$TEST_HOME"' EXIT

APP_ID=app.drey.PaperPlane.Devel
MOCK_CACHE_DIR="$TEST_HOME/.var/app/$APP_ID/cache/automation"
mkdir -p -- "$MOCK_CACHE_DIR"

mock_gdbus() {
    if [[ ${9-} != "$EXPECTED_ACTION" || ${10-} != "$EXPECTED_PARAMETERS" ]]; then
        printf 'unexpected activation:\n  action: %q\n  parameters: %q\n' "${9-}" "${10-}" >&2
        return 1
    fi
    if [[ $EXPECTED_ACTION == auto-list-chats ]]; then
        printf '123\tA chat\n' >"$MOCK_CACHE_DIR/chats.txt"
    fi
}
export -f mock_gdbus
export APP_ID MOCK_CACHE_DIR

run_driver() {
    HOME="$TEST_HOME" GDBUS=mock_gdbus \
        EXPECTED_ACTION="$1" EXPECTED_PARAMETERS="$2" \
        "$DRIVER" "${@:3}"
}

input="apostrophe: ' backslash: \\ Unicode: Grüße 東京"
input+=$'\nline\ttab'
encoded="'apostrophe: \\' backslash: \\\\ Unicode: Grüße 東京\\nline\\ttab'"
run_driver auto-submit-password "[<$encoded>]" password "$input"
run_driver auto-send-text "[<(int64 -42, $encoded)>]" send -42 "$input"
run_driver auto-react "[<(int64 42, int64 -7, $encoded)>]" react 42 -7 "$input"

printf 'stale\n' >"$MOCK_CACHE_DIR/chats.txt"
for _ in 1 2; do
    output=$(run_driver auto-list-chats '[]' list-chats)
    [[ $output == $'123\tA chat' ]]
done

printf 'dev-drive tests passed\n'
