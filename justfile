default:
    @just --list

check:
    cargo xtask check

test:
    cargo xtask test

build-xiao-esp32c6:
    cargo xtask build-xiao-esp32c6

run-xiao-esp32c6:
    cargo xtask run-xiao-esp32c6

build-xiao-esp32c6-beacon:
    cargo xtask build-xiao-esp32c6-beacon

run-xiao-esp32c6-beacon:
    cargo xtask run-xiao-esp32c6-beacon

build-xiao-esp32c6-beacon-scanner:
    cargo xtask build-xiao-esp32c6-beacon-scanner

run-xiao-esp32c6-beacon-scanner:
    cargo xtask run-xiao-esp32c6-beacon-scanner
