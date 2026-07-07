# BINA Randomness API Stack Prep

This directory prepares the future `randomness.nativebtc.org` REST layer without registering any server endpoints yet.

The REST API should be a reader and derivation adapter for `BinaOracle`. It must not become the source of randomness. Consumers should be able to verify every response against the oracle state and the documented derivation formula.

## Current Status

- No Express server is created here.
- No runtime endpoints are wired into the L1 node.
- `openapi.json` defines the future HTTP contract.
- `src/bina-oracle-client.ts` contains reusable oracle-read and derivation helpers for the future server.
- `.env.example` lists the non-secret runtime configuration that the future server will need.

## Important Consumer Binding Rule

`BinaOracle.randomUint(purpose, salt, upperBound)` derives with `msg.sender` as the consumer. For a public REST API, that would usually bind results to the API server call context, not the non-Web3 user.

The API layer should use one of these explicit-consumer paths instead:

```text
BinaOracle.deriveWord(purpose, salt, pseudoConsumerAddress)
BinaOracle.randomUintFor(purpose, salt, pseudoConsumerAddress, upperBound)
```

The pseudo consumer address is deterministic:

```text
pseudoConsumerAddress = bytes20(keccak256("BINA_API_CONSUMER_V1" || consumerId))
```

This keeps results stable for Python, Unity, PHP, Excel, and other non-Web3 callers while still letting users independently reproduce the result.

## Future Endpoint Contract

The planned endpoints are specified in `openapi.json`:

```text
GET  /randomness/latest
GET  /randomness/number?max=100&salt=round-1&consumer=my-game
GET  /randomness/pick?items=Alice,Bob,Carol&salt=round-1&consumer=giveaway
GET  /randomness/shuffle?items=A,B,C,D&salt=round-1&consumer=tournament
POST /randomness/commit
GET  /randomness/fulfill/{id}
```

High-stakes flows should use commit/fulfill with a future `minHeight`. Low-stakes reads can derive from the latest available seed.

## Verification Shape

Every REST response should include enough proof material to reproduce it:

```json
{
  "oracle": "0x...",
  "chainId": 42161,
  "purpose": "BINA_GENERIC_UTILITY",
  "purposeHash": "0x...",
  "salt": "round-1",
  "saltHash": "0x...",
  "consumer": "my-game",
  "consumerAddress": "0x...",
  "seed": "0x...",
  "height": 123,
  "btcHeight": 957090,
  "blockHash": "0x...",
  "derivation": "keccak256(BINA_EVM_UTILITY_V1, chainId, oracle, seed, purpose, salt, consumerAddress, requestId)",
  "requestId": 0
}
```

For immediate/latest reads, use `requestId = 0`, matching `deriveWord(...)`. For on-chain utility requests, use the oracle request ID emitted by `UtilityRequested`.
