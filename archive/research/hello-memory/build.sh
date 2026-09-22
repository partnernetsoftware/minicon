#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
mkdir -p target/hello-memory-track
clang -O2 archive/research/hello-memory/libc.c -o target/hello-memory-track/libc
clang -O2 archive/research/hello-memory/probe.m -framework Cocoa -o target/hello-memory-track/probe
for mode in modern compat; do
 clang -O2 archive/research/hello-memory/probe.m -framework Cocoa -Wl,-sectcreate,__TEXT,__info_plist,archive/research/hello-memory/$mode.plist -o target/hello-memory-track/probe-$mode
done
