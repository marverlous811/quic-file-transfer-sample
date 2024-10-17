#!/bin/bash
TARGET=x86_64-unknown-linux-gnu

mkdir -p tmp/bin
cross build --release --target=${TARGET} --bin=server
cross build --release --target=${TARGET} --bin=client
cross build --release --target=${TARGET} --bin=p2p-sample

cp target/${TARGET}/release/server tmp/bin/server
cp target/${TARGET}/release/client tmp/bin/client
cp target/${TARGET}/release/p2p-sample tmp/bin/p2p-sample

chmod +x tmp/bin/server
chmod +x tmp/bin/client
chmod +x tmp/bin/p2p-sample