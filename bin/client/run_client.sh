RUST_LOG=info \
BIND_ADDR=127.0.0.1:8081 \
SERVER_ADDR=127.0.0.1:8080 \
TRANSFER_FILE=../../tmp/test.ifc \
cargo run --