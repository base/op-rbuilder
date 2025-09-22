# Build and run op-rbuilder in playground mode for testing
run-playground:
    cargo build --bin op-rbuilder -p op-rbuilder
    ./target/debug/op-rbuilder node --builder.playground --flashblocks.enabled --datadir ~/.playground/devnet/rbuilder

sequencer_url := "http://localhost:8547"
builder_url := "http://localhost:2222"
ingress_url := "http://localhost:8080"

get-blocks:
    echo "Sequencer"
    cast bn -r {{ sequencer_url }}
    echo "Builder"
    cast bn -r {{ builder_url }}

sender := "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
sender_key := "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

send-txn:
    #!/usr/bin/env bash
    set -euxo pipefail
    echo "sending txn"
    nonce=$(cast nonce {{ sender }} -r {{ builder_url }})
    txn=$(cast mktx --private-key {{ sender_key }} 0x0000000000000000000000000000000000000000 --value 0.01ether --nonce $nonce --chain-id 13 -r {{ builder_url }})
    hash=$(curl -s {{ ingress_url }} -X POST   -H "Content-Type: application/json" --data "{\"method\":\"eth_sendRawTransaction\",\"params\":[\"$txn\"],\"id\":1,\"jsonrpc\":\"2.0\"}" | jq -r ".result")
    cast receipt $hash -r {{ sequencer_url }} | grep status
    cast receipt $hash -r {{ builder_url }} | grep status


# Run the complete test suite (genesis generation, build, and tests)
run-tests:
  just generate-test-genesis
  just build-op-rbuilder
  just run-tests-op-rbuilder

# Download `op-reth` binary
download-op-reth:
  ./scripts/ci/download-op-reth.sh

# Generate a genesis file (for tests)
generate-test-genesis:
  cargo run -p op-rbuilder --features="testing" --bin tester -- genesis --output genesis.json


# Build the op-rbuilder binary
build-op-rbuilder:
  cargo build -p op-rbuilder --bin op-rbuilder

# Run the integration tests
run-tests-op-rbuilder:
  PATH=$PATH:$(pwd) cargo test --package op-rbuilder --lib
