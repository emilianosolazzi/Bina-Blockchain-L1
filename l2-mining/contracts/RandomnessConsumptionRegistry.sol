// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { IRandomnessRegistry } from "./interfaces/IRandomnessRegistry.sol";

/**
 * @title  RandomnessConsumptionRegistry
 * @notice On-chain registry that tracks which mined randomness outputs have
 *         been consumed, preventing double-spend of entropy across users.
 *
 *  Design principles:
 *    - Only authorized consumer contracts can mark outputs — prevents
 *      front-running and griefing by arbitrary callers.
 *    - Every consumption records who, when, and which pool — full
 *      audit trail for off-chain verification.
 *    - Owner can batch-backfill historical outputs and manage
 *      the set of authorized consumer contracts.
 *    - Gas-bounded batch operations with configurable cap.
 *
 *  Note to self:
 *    Deploy this contract first, then deploy TGBTConsumableRandomness
 *    (or any consumer) and call authorizeConsumer() to grant it
 *    marking rights.
 *
 *    ┌─────────────────────┐     markAsConsumed()     ┌──────────────────────────────┐
 *    │ TGBTConsumable-     │ ─────────────────────▶  │ RandomnessConsumption-       │
 *    │ Randomness          │                          │ Registry                     │
 *    │ (pays TGBT fee)     │ ◀────────────────────── │ (stores consumption records) │
 *    └─────────────────────┘     isConsumed()         └──────────────────────────────┘
 */
contract RandomnessConsumptionRegistry is IRandomnessRegistry, Ownable {

    // ── Constants ────────────────────────────────────────────
    uint256 public constant MAX_BATCH_SIZE = 500;

    // ── Storage ──────────────────────────────────────────────

    /// @dev output hash → true if consumed
    mapping(bytes32 => bool) private _consumed;

    /// @dev output hash → full consumption record
    mapping(bytes32 => ConsumptionRecord) private _records;

    /// @dev consumer contract address → authorized
    mapping(address => bool) private _authorizedConsumers;

    /// @dev Total number of unique outputs consumed (lifetime counter).
    uint256 public totalConsumed;

    // ── Constructor ──────────────────────────────────────────

    constructor() Ownable(msg.sender) {}

    // ── Access Control ───────────────────────────────────────

    modifier onlyAuthorized() {
        if (!_authorizedConsumers[msg.sender]) revert NotAuthorized();
        _;
    }

    /**
     * @notice Grant a contract the right to mark outputs as consumed.
     * @dev    Call this after deploying a consumer contract (e.g.
     *         TGBTConsumableRandomness) so it can record consumption.
     * @param consumer Address of the consumer contract to authorize.
     */
    function authorizeConsumer(address consumer) external onlyOwner {
        if (consumer == address(0)) revert ZeroAddress();
        _authorizedConsumers[consumer] = true;
        emit ConsumerAuthorized(consumer);
    }

    /**
     * @notice Revoke a consumer contract's authorization.
     * @param consumer Address of the consumer contract to revoke.
     */
    function revokeConsumer(address consumer) external onlyOwner {
        if (consumer == address(0)) revert ZeroAddress();
        _authorizedConsumers[consumer] = false;
        emit ConsumerRevoked(consumer);
    }

    // ── Core: Mark as Consumed ───────────────────────────────

    /**
     * @notice Mark a single mined output as consumed.
     * @dev    Only callable by authorized consumer contracts.
     *         The consumer contract is responsible for collecting fees
     *         and verifying the caller is a legitimate user.
     * @param output  The mined randomness output hash.
     * @param poolId  The mining pool that produced this output.
     * @param endUser The actual end-user address consuming the output.
     */
    function markAsConsumed(
        bytes32 output,
        uint8   poolId,
        address endUser
    ) external override onlyAuthorized {
        if (output == bytes32(0)) revert ZeroOutput();
        if (_consumed[output]) revert AlreadyConsumed(output);

        _consumed[output] = true;
        _records[output] = ConsumptionRecord({
            consumer:   endUser,
            consumedAt: uint64(block.timestamp),
            poolId:     poolId
        });

        unchecked { ++totalConsumed; }

        emit OutputConsumed(output, endUser, poolId, uint64(block.timestamp));
    }

    /**
     * @notice Batch-mark multiple outputs as consumed (admin backfill).
     * @dev    Skips outputs that are already consumed (no revert).
     *         Bounded by MAX_BATCH_SIZE to prevent gas bombs.
     * @param outputs Array of mined output hashes.
     * @param poolId  The mining pool for all outputs in this batch.
     */
    function batchMarkConsumed(
        bytes32[] calldata outputs,
        uint8 poolId
    ) external override onlyOwner {
        uint256 count = outputs.length;
        if (count > MAX_BATCH_SIZE) revert BatchTooLarge(count, MAX_BATCH_SIZE);

        uint256 consumed;
        uint256 skipped;

        for (uint256 i; i < count;) {
            bytes32 output = outputs[i];

            if (output != bytes32(0) && !_consumed[output]) {
                _consumed[output] = true;
                _records[output] = ConsumptionRecord({
                    consumer:   msg.sender,
                    consumedAt: uint64(block.timestamp),
                    poolId:     poolId
                });

                emit OutputConsumed(output, msg.sender, poolId, uint64(block.timestamp));

                unchecked { ++consumed; }
            } else {
                unchecked { ++skipped; }
            }

            unchecked { ++i; }
        }

        unchecked { totalConsumed += consumed; }

        emit BatchConsumed(consumed, skipped);
    }

    // ── Views ────────────────────────────────────────────────

    /// @inheritdoc IRandomnessRegistry
    function isConsumed(bytes32 output) external view override returns (bool) {
        return _consumed[output];
    }

    /// @inheritdoc IRandomnessRegistry
    function getConsumptionRecord(bytes32 output)
        external
        view
        override
        returns (ConsumptionRecord memory)
    {
        return _records[output];
    }

    /// @inheritdoc IRandomnessRegistry
    function isAuthorizedConsumer(address consumer)
        external
        view
        override
        returns (bool)
    {
        return _authorizedConsumers[consumer];
    }
}
