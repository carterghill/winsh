#!/usr/bin/env winsh
# Run from PowerShell: cargo run -- examples/greet.sh Carter "another name"

greet() {
    local name=${1:-world}
    printf 'Hello, %s!\n' "$name"
}

if [ "$#" -eq 0 ]; then
    greet
else
    for name in "$@"; do
        greet "$name"
    done
fi
