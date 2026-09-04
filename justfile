default:
    @just --list

check:
    cargo xtask check

test:
    cargo xtask test

list:
    cargo xtask list

build firmware:
    cargo xtask build {{firmware}}

run firmware:
    cargo xtask run {{firmware}}

build-all:
    cargo xtask build-all

doctor board="":
    cargo xtask doctor {{board}}
