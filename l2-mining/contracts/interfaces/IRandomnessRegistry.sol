// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title  IRandomnessRegistry
 * @notice Interface for the on-chain consumption registry that prevents
 *         mined randomness outputs from being used more than once.
 *
 *  The registry tracks:
 *   - Whether a given bytes32 output has been consumed
 *   - Who consumed it (end-user address)
 *   - When it was consumed (block timestamp)
 *   - Which mining pool produced it (poolId)
 *
 *  Only authorized consumer contracts (e.g. TGBTConsumableRandomness,
 *  RandomnessShop) may call markAsConsumed(). This prevents griefing
 *  and front-running by arbitrary callers.
 */
interface IRandomnessRegistry {

    // ── Structs ──────────────────────────────────────────────

    struct ConsumptionRecord {
        address consumer;      // End-user who consumed the output
        uint64  consumedAt;    // Block timestamp of consumption
        uint8   poolId;        // Mining pool that produced the output
    }

    // ── Events ───────────────────────────────────────────────

    event OutputConsumed(
        bytes32 indexed output,
        address indexed consumer,
        uint8   indexed poolId,
        uint64  timestamp
    );

    event BatchConsumed(
        uint256 consumed,
        uint256 skipped
    );

    event ConsumerAuthorized(address indexed consumer);
    event ConsumerRevoked(address indexed consumer);

    // ── Errors ───────────────────────────────────────────────

    error AlreadyConsumed(bytes32 output);
    error NotAuthorized();
    error BatchTooLarge(uint256 provided, uint256 maximum);
    error ZeroOutput();
    error ZeroAddress();

    // ── Mutations ────────────────────────────────────────────

    /**
     * @notice Mark a single mined output as consumed.
     * @dev    Only callable by authorized consumer contracts.
     * @param output  The mined randomness output hash (bytes32).
     * @param poolId  The mining pool that produced this output.
     * @param endUser The actual end-user consuming the output (not msg.sender).
     */
    function markAsConsumed(bytes32 output, uint8 poolId, address endUser) external;

    /**
     * @notice Batch-mark multiple outputs as consumed.
     * @dev    Only callable by the contract owner (governance / admin backfill).
     * @param outputs Array of mined outputs to consume.
     * @param poolId  The mining pool for all outputs in this batch.
     */
    function batchMarkConsumed(bytes32[] calldata outputs, uint8 poolId) external;

    // ── Views ────────────────────────────────────────────────

    /// @notice Returns true if the output has already been consumed.
    function isConsumed(bytes32 output) external view returns (bool);

    /// @notice Returns the full consumption record for a given output.
    function getConsumptionRecord(bytes32 output) external view returns (ConsumptionRecord memory);

    /// @notice Returns true if the address is an authorized consumer contract.
    function isAuthorizedConsumer(address consumer) external view returns (bool);
}