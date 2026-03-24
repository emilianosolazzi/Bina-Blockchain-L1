> **NOTE**: The addresses in this document are from the original **Sepolia testnet** deployment.
> For the current **Arbitrum mainnet** deployment, see the address table below.

## Current Arbitrum Deployment (Production)

| Contract / Module | Address |
|---|---|
| TemporalGradientCore | `0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6` |
| TGBT Token | `0x31228eE520e895DA19f728DE5459b1b317d9b8D8` |
| MINING_MODULE | `0x56C458a06FB104cb31820856fCe42E1f6926CBDD` |
| BATCH_MINING_MODULE | `0xAf07E37D104E9be17639FE7a51B36972D4738651` |
| RANDOMNESS_MODULE | `0x583863CFC5EFc0106886BA485e1b67F0966584f9` |
| TOKENOMICS_MODULE | `0xF6069614FE09B91e5B00DA0a13A11B2BFcCabC36` |
| RATE_LIMIT_MODULE | `0x61dEEEf2B2956db3AD291c639939669cD5399c1B` |
| STALE_BLOCK_MODULE | `0xC4A16a11a8C61eA06F194f1EeD1d08a362fe986F` |
| Admin wallet | `0xd28E6a7AD806E85BD0544ed443D25E48f52c06c3` (Ledger) |
| Miner wallet | `0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe` |

---

## Original Sepolia Batch Module Redeployment (Historical)

Do this exactly. Do not call initialize() on the Core again.

Use these addresses (Sepolia only — see table above for Arbitrum):

Core live address: 0x843fAc753610163776374Ab0261029BAEA0251b7
TGBT: 0x496598fDeab78fb2986e89d396249779595418E9
TokenomicsModule: 0xcf0a632A88D759f4A4ad0eA0317B5BE5A10638A5
Old BatchMiningModule (DISCARD): 0xFf75dc4415EE11228697276CfBF550D0eb344dFC
Admin wallet: 0x3058bd411b9ec0dF6C7d0b04914C9bd2934b7fb3
Deployer wallet: 0xF11676bc166E2427c8Ecf134911572cb5aEe6c52

The old BatchMiningModule does not have recordStorageAttestation().
You must deploy a fresh one from the updated source.

Use the functions defined in:

BatchMiningModule.sol:96-99
TemporalGradientCore.sol:76-90
TGBT_Token.sol:23 (MINTER_ROLE), :76 (mint), :223 (addMinter/grantRole)


1) Deploy the new BatchMiningModule

In Remix, compile contracts/modules/BatchMiningModule.sol with Solidity 0.8.28 (optimizer 200).
Switch to Deployer wallet 0xF11676bc166E2427c8Ecf134911572cb5aEe6c52.
Deploy BatchMiningModule (no constructor arguments).
Copy the new address. This is NEW_BATCH below.


2) Initialize the new BatchMiningModule

On the NEW_BATCH contract you just deployed, call:

initialize(
  0x843fAc753610163776374Ab0261029BAEA0251b7,
  0x496598fDeab78fb2986e89d396249779595418E9
)

Meaning:

coreAddress = Core
stakeTokenAddress = TGBT (same token the old batch used)


3) Register the new BatchMiningModule in Core

Switch to Admin wallet 0x3058bd411b9ec0dF6C7d0b04914C9bd2934b7fb3.
Only this wallet has GOVERNANCE_ROLE. If you call from the deployer, it will revert.

On Core 0x843fAc753610163776374Ab0261029BAEA0251b7, call:

setModule(
  0x874922d3c48d591ce2c027cf2e1ab8e8bce4a1f4d93c1f05d0801410005ccaf2,
  NEW_BATCH
)

The first argument is keccak256("BATCH_MINING_MODULE").
Or call BATCH_MINING_MODULE() on the Core and copy the returned bytes32.


4) Grant MINTER_ROLE to TokenomicsModule on TGBT

Stay on Admin wallet 0x3058bd411b9ec0dF6C7d0b04914C9bd2934b7fb3.

On TGBT 0x496598fDeab78fb2986e89d396249779595418E9, call:

grantRole(
  0x9f2df0fed2c77648de5860a4cc508cd0818c85b8b8a1ab4ceeef8d981c8956a6,
  0xcf0a632A88D759f4A4ad0eA0317B5BE5A10638A5
)

The first argument is keccak256("MINTER_ROLE").
The second argument is the TokenomicsModule address.

This was previously granted to the deployer wallet by mistake.
Without this, every epoch finalization will revert at tgbt.mint().


5) Verify

On Core 0x843fAc753610163776374Ab0261029BAEA0251b7:

moduleAddress(0x874922d3c48d591ce2c027cf2e1ab8e8bce4a1f4d93c1f05d0801410005ccaf2)
  → should return NEW_BATCH

On TGBT 0x496598fDeab78fb2986e89d396249779595418E9:

hasRole(0x9f2df0fed2c77648de5860a4cc508cd0818c85b8b8a1ab4ceeef8d981c8956a6, 0xcf0a632A88D759f4A4ad0eA0317B5BE5A10638A5)
  → should return true

On NEW_BATCH:

core()
  → should return 0x843fAc753610163776374Ab0261029BAEA0251b7

stakeToken()
  → should return 0x496598fDeab78fb2986e89d396249779595418E9


6) Update off-chain config

Set BATCH_CONTRACT=NEW_BATCH in the epoch-builder env.
In l2-mining/js/remix-helper.js, change the batch address to NEW_BATCH.
Restart server.js, then epoch-builder.js.


Do not do these

- do not call Core.initialize() again
- do not use old batch 0xFf75dc4415EE11228697276CfBF550D0eb344dFC for anything — it has no attestation support
- do not call setModule from the deployer wallet — it will revert
- do not grant MINTER_ROLE to the deployer wallet — grant it to TokenomicsModule 0xcf0a632A88D759f4A4ad0eA0317B5BE5A10638A5

If one of these calls reverts, send the exact revert/error and stop there.
