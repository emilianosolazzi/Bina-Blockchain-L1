# BINA Wallet

Native BINA wallets use a hybrid signing key: Ed25519 plus Falcon-512. Both signatures must verify for native wallet messages and mined block claims. Value-transfer transactions also accept a provisional FastPath Ed25519-only sender so FastPath-derived addresses can sweep received BINA into a sovereign `l1-wallet.exe` wallet before deterministic Falcon derivation is available.

## Build the wallet CLI

From the repository root:

```powershell
cd l1-chain
cargo build -p l1-node --bin l1-wallet
```

The debug binary is created at:

```text
l1-chain/target/debug/l1-wallet.exe
```

You can also run the wallet directly through Cargo:

```powershell
cargo run -p l1-node --bin l1-wallet -- <command>
```

## Create a wallet

Default wallet location:

```text
%USERPROFILE%\.bina\wallet.json
```

Create a new wallet:

```powershell
.\target\debug\l1-wallet.exe generate
```

Or with Cargo:

```powershell
cargo run -p l1-node --bin l1-wallet -- generate
```

The command refuses to overwrite an existing wallet. If you intentionally want a new wallet, move or delete the old file first.

To create a wallet at a custom path:

```powershell
.\target\debug\l1-wallet.exe generate --path .\wallet.json
```

## Show the wallet address

```powershell
.\target\debug\l1-wallet.exe show
.\target\debug\l1-wallet.exe address
.\target\debug\l1-wallet.exe public-key
```

`show` prints the address and public key summary. `address` prints only the 20-byte BINA address, useful for scripts. `public-key` prints the full public key hex for verifiers.

## Manually sign a message

```powershell
.\target\debug\l1-wallet.exe sign "hello from BINA"
```

Output shape:

```text
address  : <40 hex chars>
message  : hello from BINA
sig_hex  : <hybrid signature hex>
ed25519  : <ed25519 signature hex>
falcon   : <falcon signature prefix>...  (<bytes> bytes)
sig_bytes: <total serialized signature bytes>
```

`sig_hex` is the portable signature. It serializes:

```text
u16_le(falcon_signature_length) || ed25519_signature_64_bytes || falcon_signature_bytes
```

## Verify a manual signature

To verify your own signature with your local wallet file, copy the `sig_hex` value from `sign`, then run:

```powershell
.\target\debug\l1-wallet.exe verify "hello from BINA" "<sig_hex>"
```

To let another user or service verify the signature without your secret wallet file, send them:

```text
message
public_key
sig_hex
```

They verify with:

```powershell
.\target\debug\l1-wallet.exe verify-public "hello from BINA" "<public_key_hex>" "<sig_hex>"
```

A valid signature prints:

```text
VALID - both Ed25519 and Falcon-512 verified
```

## FastPath-derived BINA identity

FastPathIdentity can register a separate BINA key for an EVM-controlled hash160 identity. That key is not derived by `l1-wallet.exe` and is not derived in the browser from a MetaMask signature.

FastPath-derived BINA keys belong to the server-assisted FastPath wallet pipeline:

```text
seed = deriveSeed(evmAddress, "bina-identity", network)
ed25519_keypair = deriveEd25519(seed)
provisional_bina_address = keccak256(ed25519_pk)[0..20]
```

The provisional FastPath address can receive BINA on L1 immediately. It can sign value-transfer transactions with the Ed25519 private key and sweep funds to a sovereign `l1-wallet.exe` address. It cannot mine blocks or sign hybrid wallet messages.

When deterministic Falcon derivation is added, the full FastPath hybrid address becomes:

```text
falcon_keypair = deriveFalcon512(seed)
bina_address = BLAKE3("BINA-ADDR-v1" || ed25519_pk || falcon_pk)[0..20]
```

FastPathIdentityV2 registration on Arbitrum is optional for L1 coin movement. The FastPath server can compute the public fields and `binaDerivationCommitment(...)`, but the current contract requires the active EVM controller to submit `registerBinaKey(...)` or `rotateBinaKey(...)`.

There are two valid BINA key types:

```text
FastPath-derived BINA key
	Source:  salt + pepper + evmAddress through the FastPath server pipeline
	Purpose: provisional L1 receive/sweep address; optional FastPathIdentityV2 registry and cross-chain identity backup
	Custody: server-assisted, exportable with explicit consent

l1-wallet.exe BINA key
	Source:  fresh local randomness from l1-wallet.exe generate
	Purpose: native BINA L1 transactions, mining claims, and receiving BINA coin
	Custody: user-sovereign, not reconstructable by the FastPath server
```

These keys may be different, and the UI must show them separately. A user can unify them only by explicitly exporting the FastPath-derived BINA key into `l1-wallet.exe`, or by registering their existing `l1-wallet.exe` public key in FastPathIdentityV2.

## Sign a BINA transaction

A value-transfer transaction is signed over this digest:

```text
tx_digest = blake3("BINA-TX-v1" || from || to || amount_le64 || nonce_le64 || fee_le64)
signature = hybrid_sign(tx_digest) for native wallets
signature = ed25519_sign(tx_digest) for provisional FastPath wallets
```

Transaction fields:

```text
from    20-byte BINA sender address, derived from the signing wallet
to      20-byte BINA recipient address
amount  u64 base units
nonce   u64 replay-protection nonce for the sender
fee     u64 miner incentive
```

Sign a transaction:

```powershell
.\target\debug\l1-wallet.exe sign-tx "<to_address>" 25 0 1
```

The output is signed transaction JSON:

```json
{
	"version": 1,
	"from": "<sender_address>",
	"to": "<recipient_address>",
	"amount": 25,
	"nonce": 0,
	"fee": 1,
	"tx_digest": "<blake3 transaction digest>",
	"tx_id": "<signed transaction id>",
	"public_key": "<hybrid public key hex or 32-byte Ed25519 public key hex>",
	"signature": "<hybrid signature hex or 64-byte Ed25519 signature hex>"
}
```

Submit a signed transaction to the local node:

```powershell
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:8181/tx/submit -ContentType 'application/json' -InFile .\signed-tx.json
```

Save a signed transaction to a file:

```powershell
.\target\debug\l1-wallet.exe sign-tx "<to_address>" 25 0 1 > signed-tx.json
```

Verify a signed transaction:

```powershell
.\target\debug\l1-wallet.exe verify-tx .\signed-tx.json
```

The ledger accepts a signed transaction only when:

```text
hybrid signatures verify OR the Ed25519-only signature verifies
public_key derives the from address:
	hybrid:   BLAKE3("BINA-ADDR-v1" || ed25519_pk || falcon_pk)[0..20]
	Ed25519:  keccak256(ed25519_pk)[0..20]
amount is non-zero
from != to
nonce equals the sender's next ledger nonce
sender balance >= amount + fee
```

## Mining signatures

Manual signing is for messages and user-facing actions. Mining does not require the user to paste signatures manually. The node loads `%USERPROFILE%\.bina\wallet.json`, signs each mined block claim with the same hybrid wallet, and broadcasts the signed claim to peers.

## Security

Never commit or share `wallet.json`. The `secret_key` field is the full wallet secret. Anyone who has it can sign as that BINA address.

If a `secret_key` was pasted into chat, uploaded, or committed, treat that wallet as compromised and generate a new one.
