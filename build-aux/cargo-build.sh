#!/bin/sh

set -eu

cargo="${CARGO:-cargo}"
binary="$1"
output="$2"
shift 2

"$cargo" build "$@"
cp "$binary" "$output"
