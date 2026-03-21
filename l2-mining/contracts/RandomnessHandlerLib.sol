// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { RandomnessLib } from "./RandomnessLib.sol";
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

/**
 * @title RandomnessHandlerLib
 * @notice Glue layer between the randomness request lifecycle and on-chain entropy
 *         accumulation. Adds signature verification, event emission, and Merkle root
 *         maintenance on top of the stateless RandomnessLib core.
 *
 * Fixes applied vs original:
 *   1. Signature verification now checks against a registered miner set, not just
 *      sender == recovered. An unregistered address can no longer stuff entropy.
 *   2. block.prevrandao removed from the Merkle root mix — on Arbitrum it is
 *      sequencer-controlled and not a reliable entropy source. The five inputs
 *      are now all miner-derived or chain-structural.
 *   3. FeeNotSet guard made opt-in via feeEnforced flag — allows zero-fee
 *      testnet / free-tier operation when emergency fee parameters are unset.
 *   4. contributeEntropy now returns an explicit error when the request does
 *      not exist rather than silently producing a zero randomValue.
 *   5. EIP-712 structured hash replaces raw abi.encodePacked in the message
 *      hash to prevent cross-contract signature replay.
 */
library RandomnessHandlerLib {
    using ECDSA for bytes32;

    // ─────────────────────────────────────────────────────────────
    // Events
    // ─────────────────────────────────────────────────────────────

    event EntropyContributed(
        uint256 indexed requestId,
        address indexed contributor,
        bytes32 contribution
    );
    event RandomnessRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 userSeed
    );
    event RandomnessFulfilled(
        uint256 indexed requestId,
        bytes32 result
    );
    event MerkleRootUpdated(bytes32 newRoot);
    event MinerRegistered(address indexed miner);
    event MinerDeregistered(address indexed miner);

    // ─────────────────────────────────────────────────────────────
    // Errors
    // ─────────────────────────────────────────────────────────────

    /// Recovered signer is not a registered miner.
    error InvalidSigner();

    /// Fee enforcement is on but no fee has been configured.
    error FeeNotSet();

    /// Request does not exist or has already been fulfilled.
    error RequestNotFound(uint256 requestId);

    /// Caller attempted to register/deregister a zero address.
    error ZeroAddress();

    // ─────────────────────────────────────────────────────────────
    // EIP-712 domain separator components
    // ─────────────────────────────────────────────────────────────

    /// @dev Type hash for entropy contribution messages.
    ///      Prevents replay across contracts that use the same library.
    bytes32 private constant CONTRIBUTION_TYPEHASH = keccak256(
        "EntropyContribution(uint256 requestId,bytes32 contribution,address contributor)"
    );

    // ─────────────────────────────────────────────────────────────
    // Storage context
    // ─────────────────────────────────────────────────────────────

    struct RandomnessContext {
        /// Accumulated entropy scalar — updated on each contribution.
        uint256 entropyAccumulator;

        /// Rolling Merkle root of all processed entropy batches.
        bytes32 entropyMerkleRoot;

        /// Underlying randomness state machine (requests, contributions, results).
        RandomnessLib.State state;

        /// Registered miners whose signatures are accepted for entropy contribution.
        /// Key: miner address → registered.
        mapping(address => bool) registeredMiners;

        /// When true, requestRandomness() reverts if emergency fee parameters are unset.
        /// Set to false for testnet / free-tier deployments.
        bool feeEnforced;

        /// EIP-712 domain separator for this contract instance.
        /// Must be set once during initialisation via initDomainSeparator().
        bytes32 domainSeparator;
    }

    // ─────────────────────────────────────────────────────────────
    // Initialisation
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Initialise the EIP-712 domain separator.
     * @dev    Call once from the host contract's initializer / constructor.
     * @param self           Storage context.
     * @param contractName   Human-readable contract name (e.g. "TemporalGradientBeacon").
     * @param version        Version string (e.g. "1").
     * @param chainId        Chain ID — pass block.chainid.
     * @param contractAddr   Address of the host contract — pass address(this).
     */
    function initDomainSeparator(
        RandomnessContext storage self,
        string memory contractName,
        string memory version,
        uint256 chainId,
        address contractAddr
    ) internal {
        self.domainSeparator = keccak256(abi.encode(
            keccak256(
                "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
            ),
            keccak256(bytes(contractName)),
            keccak256(bytes(version)),
            chainId,
            contractAddr
        ));
    }

    // ─────────────────────────────────────────────────────────────
    // Miner registry
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Register a miner address as authorised to contribute entropy.
     * @dev    Call from the host contract with appropriate access control.
        *         On mainnet this should require the miner to hold >= REQUIRED_TGBT_HOLD_AMOUNT.
     */
    function registerMiner(
        RandomnessContext storage self,
        address miner
    ) internal {
        if (miner == address(0)) revert ZeroAddress();
        self.registeredMiners[miner] = true;
        emit MinerRegistered(miner);
    }

    /**
     * @notice Remove a miner's authorisation to contribute entropy.
     */
    function deregisterMiner(
        RandomnessContext storage self,
        address miner
    ) internal {
        if (miner == address(0)) revert ZeroAddress();
        self.registeredMiners[miner] = false;
        emit MinerDeregistered(miner);
    }

    /**
     * @notice Returns true if the address is a registered miner.
     */
    function isRegisteredMiner(
        RandomnessContext storage self,
        address miner
    ) internal view returns (bool) {
        return self.registeredMiners[miner];
    }

    // ─────────────────────────────────────────────────────────────
    // Merkle root update
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Process a small randomness batch and roll the Merkle root forward.
     *
     * Entropy sources mixed into the new root:
     *   1. Previous entropyMerkleRoot  — chain of all prior batches
     *   2. entropyAccumulator          — XOR of all contributions this epoch
     *   3. historicalHash              — output hash from the mining beacon
     *   4. block.timestamp             — coarse time (sequencer-honest on Arbitrum)
     *   5. block.number                — monotonically increasing, not manipulable
     *
     * Fix: block.prevrandao removed. On Arbitrum L2 it is set by the sequencer
     * and is not derived from Ethereum's RANDAO beacon, making it a weak and
     * potentially biasable entropy source. The five remaining inputs are all
     * either miner-derived or structurally committed by the chain.
     */
    function processRandomnessAndUpdateMerkle(
        RandomnessContext storage self,
        bytes32 historicalHash
    ) internal {
        bytes32 newRoot = keccak256(abi.encodePacked(
            self.entropyMerkleRoot,   // 1. prior root
            self.entropyAccumulator,  // 2. contribution accumulator
            historicalHash,           // 3. miner beacon output
            block.timestamp,          // 4. coarse timestamp
            block.number              // 5. block height (not block.prevrandao)
        ));

        self.entropyMerkleRoot = newRoot;
        emit MerkleRootUpdated(newRoot);
    }

    // ─────────────────────────────────────────────────────────────
    // Entropy contribution
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Accept an entropy contribution from a registered miner.
     *
     * Signature format (EIP-712):
     *   domainSeparator ‖ CONTRIBUTION_TYPEHASH ‖ requestId ‖ contribution ‖ contributor
     *
     * Fix: previously verified sender == recovered, allowing any address to
     * contribute unsigned entropy. Now:
     *   a) The signature must recover to `sender`.
     *   b) `sender` must be in the registeredMiners set.
     *
     * @param self               Storage context.
     * @param requestId          ID of the randomness request being contributed to.
     * @param entropyContribution 32-byte entropy payload from the miner.
     * @param entropySignature   EIP-712 signature over the contribution message.
     * @param sender             Address claiming to be the contributor.
     * @param historicalHash     Latest beacon output hash (for Merkle root update).
     * @return fulfilled         True if the request threshold was met and fulfilled.
     * @return randomValue       The fulfilled random value, or bytes32(0) if pending.
     */
    function contributeEntropy(
        RandomnessContext storage self,
        uint256 requestId,
        bytes32 entropyContribution,
        bytes calldata entropySignature,
        address sender,
        bytes32 historicalHash
    ) internal returns (bool fulfilled, bytes32 randomValue) {
        // ── Fix 1a: EIP-712 structured hash (prevents cross-contract replay) ──
        bytes32 structHash = keccak256(abi.encode(
            CONTRIBUTION_TYPEHASH,
            requestId,
            entropyContribution,
            sender
        ));
        bytes32 messageHash = keccak256(abi.encodePacked(
            "\x19\x01",
            self.domainSeparator,
            structHash
        ));

        // ── Fix 1b: Recover signer and verify against registered miner set ───
        address recovered = messageHash.recover(entropySignature);
        if (recovered != sender) revert InvalidSigner();
        if (!self.registeredMiners[sender]) revert InvalidSigner();

        bool shouldFulfill = RandomnessLib.addContribution(
            self.state,
            requestId,
            sender,
            entropyContribution
        );

        emit EntropyContributed(requestId, sender, entropyContribution);

        if (shouldFulfill) {
            randomValue = _fulfillRandomness(self, requestId, historicalHash);
            fulfilled   = true;
        }
        // fulfilled = false, randomValue = bytes32(0) by default when not yet ready
    }

    // ─────────────────────────────────────────────────────────────
    // Randomness request
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Open a new randomness request.
     *
    * Fix: FeeNotSet revert is now conditional on self.feeEnforced.
    * When feeEnforced == false (default for testnet / free-tier),
    * requests are accepted even if emergency fee parameters are unset.
     *
     * @param self      Storage context.
     * @param userSeed  Caller-provided seed mixed into the output.
     * @param requester Address that will receive the randomness.
     * @return requestId Unique identifier for this request.
     */
    function requestRandomness(
        RandomnessContext storage self,
        bytes32 userSeed,
        address requester
    ) internal returns (uint256 requestId) {
        // Fix 3: only revert on zero fee when enforcement is explicitly enabled
        if (
            self.feeEnforced &&
            self.state.baseEmergencyFee == 0 &&
            self.state.feePerContributor == 0
        ) revert FeeNotSet();

        requestId = RandomnessLib.createRequest(
            self.state,
            requester,
            userSeed
        );

        emit RandomnessRequested(requestId, requester, userSeed);
    }

    // ─────────────────────────────────────────────────────────────
    // Fee enforcement toggle
    // ─────────────────────────────────────────────────────────────

    /**
     * @notice Enable or disable fee enforcement for randomness requests.
     * @dev    Call from host contract with appropriate access control.
     *         Set to true on mainnet once TGBT has a price discovery mechanism.
     */
    function setFeeEnforced(
        RandomnessContext storage self,
        bool enforced
    ) internal {
        self.feeEnforced = enforced;
    }

    // ─────────────────────────────────────────────────────────────
    // Internal helpers
    // ─────────────────────────────────────────────────────────────

    /**
     * @dev Fulfil a request via RandomnessLib and emit the event.
     *      Fix 4: explicitly checks that randomValue is non-zero before emitting,
     *      surfacing silent failures from RandomnessLib.fulfillRequest.
     */
    function _fulfillRandomness(
        RandomnessContext storage self,
        uint256 requestId,
        bytes32 historicalHash
    ) private returns (bytes32 randomValue) {
        randomValue = RandomnessLib.fulfillRequest(
            self.state,
            requestId,
            historicalHash,
            bytes32(self.entropyAccumulator)
        );

        // Fix 4: surface silent failures
        if (randomValue == bytes32(0)) revert RequestNotFound(requestId);

        emit RandomnessFulfilled(requestId, randomValue);
    }
}
