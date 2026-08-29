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
