#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
mkdir -p target/rss-ledger
clang -O2 -Wall -Wextra -Werror -dynamiclib research/rss-ledger/ledger.c -o target/rss-ledger/ledger.dylib
clang -O2 -Wall -Wextra -Werror research/rss-ledger/control.c -o target/rss-ledger/control
