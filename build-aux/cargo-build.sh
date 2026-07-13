#!/bin/sh

set -eu

cargo="$1"
binary="$2"
output="$3"
shift 3

"$cargo" build "$@"
cp "$binary" "$output"
