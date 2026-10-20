#!/usr/bin/env bash
# Build the wasm core and vendor the P2P signaling library into web/.
set -euo pipefail
cd "$(dirname "$0")"
# The deck lives in core/src/shamrock.json; regenerate its Rust form.
python3 - <<'PY'
import json
cards = json.load(open('core/src/shamrock.json'))
lines = ["//! The deck: %d cards, four words each (top, right, bottom, left)." % len(cards),
         "//! Generated from `shamrock.json` (from jmandel/shamrock) by `build.sh`; edit the JSON, not this file.", "",
         "pub const CARDS: [[&str; 4]; %d] = [" % len(cards)]
lines += ["    [" + ", ".join(json.dumps(w) for w in c) + "]," for c in cards]
lines.append("];")
open('core/src/words.rs', 'w').write("\n".join(lines) + "\n")
PY
(cd core && cargo build --release --target wasm32-unknown-unknown)
cp core/target/wasm32-unknown-unknown/release/cloverleaf.wasm web/core.wasm
if [ -d node_modules/trystero ]; then
  node scripts/build-nostr.mjs
  bun build node_modules/@trystero-p2p/ws-relay/dist/index.mjs --target=browser --format=esm --minify --outfile web/vendor/trystero-ws.js
fi
ls -l web/core.wasm web/vendor/
