// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./modules/ModuleBase.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import { IRandomnessRegistry } from "./interfaces/IRandomnessRegistry.sol";

/**
 * @title  TGBTConsumableRandomness
 * @notice Pay-to-consume gateway for mined randomness outputs.
 *         Users pay a TGBT fee to consume (claim exclusive use of) a
 *         previously mined randomness output. The consumption is
 *         recorded in the RandomnessConsumptionRegistry, preventing
 *         anyone else from using the same output.
 *
 *  Flow:
 *    1. Miner produces output via MiningModule/BatchMiningModule → stored in Core history
 *    2. User identifies an output they want to use
 *    3. User approves TGBT to this contract
 *    4. User calls consumeRandomness(output, poolId)
 *    5. Contract checks the output hasn't been consumed yet
 *    6. Contract marks it consumed in the registry (CEI: state before transfer)
 *    7. Contract pulls TGBT fee and splits between burn + treasury
 *    8. Output is now exclusively claimed by the user
 *
 *  Revenue split (same model as RandomnessShop):
 *    ┌────────────────────────────────────────────────┐
 *    │  User pays TGBT fee                            │
 *    │         │                                      │
 *    │    ┌────┴────┐                                 │
 *    │  burn%     treasury%                           │
 *    │  (reduces  (protocol                           │
 *    │   supply)   funding)                           │
 *    └────────────────────────────────────────────────┘
 *
 *  Follows existing architecture:
 *    - ModuleBase integration (reads Core output history for validation)
 *    - ReentrancyGuard on all external mutations
 *    - SafeERC20 for token transfers
 *    - Governance-gated config with ossification support
 *    - whenSystemActive gate (respects Core pause)
 */
contract TGBTConsumableRandomness is ModuleBase, ReentrancyGuard {
    using SafeERC20 for IERC20;

    // ── Constants ────────────────────────────────────────────
    uint256 private constant BPS_SCALE = 10_000;
    uint256 public  constant MAX_BURN_BPS = 10_000;  // up to 100% burn
    uint256 public  constant MAX_FEE = 100_000 ether; // sanity cap
    uint256 public  constant MIN_FEE = 1;             // minimum 1 wei TGBT

    uint256 private constant OUTPUT_HISTORY_SIZE = 32;

    // ── State ────────────────────────────────────────────────
    ITGBT public tgbtToken;
    IRandomnessRegistry public registry;

    address public protocolTreasury;
    address public burnAddress;

    uint256 public consumeFee;
    uint256 public burnShareBps;   // basis points burned (default 7000 = 70%)

    bool public configLocked;
    bool public validateOutputExists; // when true, checks Core output history

    // ── Counters ─────────────────────────────────────────────
    uint256 public totalConsumed;
    uint256 public totalFeesCollected;
    uint256 public totalBurned;

    // ── Events ───────────────────────────────────────────────
    event RandomnessConsumed(
        bytes32 indexed output,
        address indexed user,
        uint8   indexed poolId,
        uint256 feePaid,
        uint256 burnAmount,
        uint256 treasuryAmount
    );

    event ConsumeFeeUpdated(uint256 oldFee, uint256 newFee);
    event BurnShareUpdated(uint256 oldBps, uint256 newBps);
    event TreasuryUpdated(address oldTreasury, address newTreasury);
    event BurnAddressUpdated(address oldBurn, address newBurn);
    event OutputValidationToggled(bool enabled);
    event ConfigLockedForever();

    // ── Errors ───────────────────────────────────────────────
    error OutputAlreadyConsumed(bytes32 output);
    error OutputNotInHistory(bytes32 output);
    error InvalidFee();
    error InvalidBurnShare();
    error ZeroAddress();
    error ConfigIsLocked();
    error ZeroOutput();

    // ── Modifiers ────────────────────────────────────────────

    modifier whenConfigUnlocked() {
        if (configLocked) revert ConfigIsLocked();
        _;
    }

    // ── Initialization (ModuleBase pattern) ──────────────────

    /**
     * @notice One-shot initializer following the ModuleBase pattern.
     * @param coreAddress         TemporalGradientCore address.
     * @param _tgbtToken          TGBT token address.
     * @param _registry           RandomnessConsumptionRegistry address.
     * @param _protocolTreasury   Treasury address for protocol share.
     * @param _burnAddress        Burn sink address (e.g. 0xdead).
     * @param _consumeFee         Initial fee in TGBT wei per consumption.
     * @param _burnShareBps       Initial burn share in basis points (e.g. 7000 = 70%).
     */
    function initialize(
        address coreAddress,
        address _tgbtToken,
        address _registry,
        address _protocolTreasury,
        address _burnAddress,
        uint256 _consumeFee,
        uint256 _burnShareBps
    ) external {
        __ModuleBase_init(coreAddress);

        if (_tgbtToken == address(0))        revert ZeroAddress();
        if (_registry == address(0))         revert ZeroAddress();
        if (_protocolTreasury == address(0)) revert ZeroAddress();
        if (_burnAddress == address(0))      revert ZeroAddress();
        if (_consumeFee < MIN_FEE || _consumeFee > MAX_FEE) revert InvalidFee();
        if (_burnShareBps > MAX_BURN_BPS)    revert InvalidBurnShare();

        tgbtToken        = ITGBT(_tgbtToken);
        registry         = IRandomnessRegistry(_registry);
        protocolTreasury = _protocolTreasury;
        burnAddress      = _burnAddress;
        consumeFee       = _consumeFee;
        burnShareBps     = _burnShareBps;

        // Off by default — enable once Core has accumulated output history
        validateOutputExists = false;
    }

    // ══════════════════════════════════════════════════════════
    //  CORE: Consume a mined randomness output
    // ══════════════════════════════════════════════════════════

    /**
     * @notice Consume a mined randomness output, paying a TGBT fee.
     * @dev    The caller must have approved this contract for at least
     *         `consumeFee` TGBT before calling.
     *
     *         Follows Checks-Effects-Interactions:
     *           1. Check: output not yet consumed
     *           2. Check: output exists in Core history (optional)
     *           3. Effect: mark consumed in registry
     *           4. Interaction: pull TGBT fee + distribute
     *
     * @param output  The mined randomness bytes32 output.
     * @param poolId  The mining pool ID that produced the output.
     */
    function consumeRandomness(
        bytes32 output,
        uint8   poolId
    ) external nonReentrant whenSystemActive {
        if (output == bytes32(0)) revert ZeroOutput();

        // ── 1. Check: not already consumed ───────────────────
        if (registry.isConsumed(output))
            revert OutputAlreadyConsumed(output);

        // ── 2. Check: output exists in beacon history (optional) ─
        if (validateOutputExists) {
            bool found = _outputExistsInHistory(output);
            if (!found) revert OutputNotInHistory(output);
        }

        // ── 3. Effect: mark consumed BEFORE external calls ───
        registry.markAsConsumed(output, poolId, msg.sender);

        // ── 4. Interaction: pull TGBT and distribute ─────────
        uint256 fee = consumeFee;
        IERC20 token = IERC20(address(tgbtToken));
        token.safeTransferFrom(msg.sender, address(this), fee);

        uint256 burnAmount;
        uint256 treasuryAmount;

        if (burnShareBps > 0) {
            burnAmount = (fee * burnShareBps) / BPS_SCALE;
        }
        treasuryAmount = fee - burnAmount;

        if (burnAmount > 0) {
            token.safeTransfer(burnAddress, burnAmount);
            unchecked { totalBurned += burnAmount; }
        }
        if (treasuryAmount > 0) {
            token.safeTransfer(protocolTreasury, treasuryAmount);
        }

        unchecked {
            ++totalConsumed;
            totalFeesCollected += fee;
        }

        emit RandomnessConsumed(
            output, msg.sender, poolId, fee, burnAmount, treasuryAmount
        );
    }

    // ══════════════════════════════════════════════════════════
    //  Views
    // ══════════════════════════════════════════════════════════

    /**
     * @notice Get a fee quote for consuming one output.
     * @return fee           Total TGBT cost.
     * @return burnShare     Amount that will be burned.
     * @return treasuryShare Amount that goes to treasury.
     */
    function getQuote()
        external
        view
        returns (uint256 fee, uint256 burnShare, uint256 treasuryShare)
    {
        fee = consumeFee;
        burnShare = (fee * burnShareBps) / BPS_SCALE;
        treasuryShare = fee - burnShare;
    }

    /**
     * @notice Marketplace health metrics.
     */
    function getStats()
        external
        view
        returns (
            uint256 lifetimeConsumed,
            uint256 lifetimeFees,
            uint256 lifetimeBurned,
            uint256 currentFee,
            uint256 currentBurnBps
        )
    {
        return (
            totalConsumed,
            totalFeesCollected,
            totalBurned,
            consumeFee,
            burnShareBps
        );
    }

    // ══════════════════════════════════════════════════════════
    //  Governance — config tuning
    // ══════════════════════════════════════════════════════════

    function setConsumeFee(uint256 newFee) external onlyGovernance whenConfigUnlocked {
        if (newFee < MIN_FEE || newFee > MAX_FEE) revert InvalidFee();
        emit ConsumeFeeUpdated(consumeFee, newFee);
        consumeFee = newFee;
    }

    function setBurnShare(uint256 newBps) external onlyGovernance whenConfigUnlocked {
        if (newBps > MAX_BURN_BPS) revert InvalidBurnShare();
        emit BurnShareUpdated(burnShareBps, newBps);
        burnShareBps = newBps;
    }

    function setTreasury(address newTreasury) external onlyGovernance whenConfigUnlocked {
        if (newTreasury == address(0)) revert ZeroAddress();
        emit TreasuryUpdated(protocolTreasury, newTreasury);
        protocolTreasury = newTreasury;
    }

    function setBurnAddress(address newBurn) external onlyGovernance whenConfigUnlocked {
        if (newBurn == address(0)) revert ZeroAddress();
        emit BurnAddressUpdated(burnAddress, newBurn);
        burnAddress = newBurn;
    }

    function setOutputValidation(bool enabled) external onlyGovernance {
        validateOutputExists = enabled;
        emit OutputValidationToggled(enabled);
    }

    /**
     * @notice Permanently freeze all configuration. Irreversible.
     *         Bitcoin-style ossification — after this call, governance
     *         has zero power over contract parameters.
     */
    function lockConfig() external onlyGovernance {
        configLocked = true;
        emit ConfigLockedForever();
    }

    /**
     * @notice Emergency withdraw stranded tokens to treasury.
     * @dev    Available even after lockConfig() — cannot strand tokens forever.
     */
    function emergencyWithdraw(address token, uint256 amount) external onlyGovernance {
        IERC20(token).safeTransfer(protocolTreasury, amount);
    }

    // ══════════════════════════════════════════════════════════
    //  Internal
    // ══════════════════════════════════════════════════════════

    /**
     * @dev Check if a given output exists in the Core's 32-entry ring buffer.
     *      This provides on-chain validation that the output was actually mined.
     */
    function _outputExistsInHistory(bytes32 output) internal view returns (bool) {
        bytes32[32] memory history = _outputHistory();
        for (uint256 i; i < OUTPUT_HISTORY_SIZE;) {
            if (history[i] == output) return true;
            unchecked { ++i; }
        }
        return false;
    }
}
