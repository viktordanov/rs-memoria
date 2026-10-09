#!/bin/sh
set -eu
cargo build --release
scp ../target/release/ledger ledger.example.com:/srv/ledger/
