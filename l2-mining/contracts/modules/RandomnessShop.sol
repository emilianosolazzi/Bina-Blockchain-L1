// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./ModuleBase.sol";
import { ITGBT } from "../interfaces/ITGBT.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/**
 * @title  RandomnessShop — Pay-per-proof randomness marketplace
 * @notice Sells verifiable randomness proofs and accepts payment in TGBT.
 *         Revenue is split between direct burn support for miners and protocol
 *         treasury funding, giving TGBT a real utility sink.
 *
 *  Design rationale — why this helps miners:
 *
 *    ┌───────────────────────────────────────────────────────────────────┐
 *    │  Customer pays TGBT                                              │
 *    │         │                                                        │
 *    │         ▼                                                        │
 *    │  RandomnessShop collects fee                                     │
 *    │         │                                                        │
 *    │    ┌────┴────┐                                                   │
 *    │    │         │                                                   │
 *    │  70%       30%                                                   │
 *    │  miner     protocol                                              │
 *    │  share     treasury                                              │
 *    │    │                                                             │
 *    │    ▼                                                             │
 *    │  Burn miner share directly                                       │
 *    │  (reduces supply → token appreciates → miner rewards worth more)│
 *    └───────────────────────────────────────────────────────────────────┘
 *
 *  Key differences from the old RandomnessShop:
 *    ✗ Old: Unlimited TGBT mint for USDC (inflationary, bypasses mining)
 *    ✓ New: Sells randomness PROOFS, not tokens. Zero TGBT minting.
 *    ✗ Old: Owner-set fixed exchange rate. No market.
 *    ✓ New: Proofs are paid in TGBT directly, creating token utility.
 *    ✗ Old: Revenue to owner treasury only. Miners get nothing.
 *    ✓ New: Miner share is burned immediately, protocol share goes to treasury.
 *    ✗ Old: Not connected to any module. Standalone.
 *    ✓ New: ModuleBase integration, reads beacon outputs, issues receipts.
 *
 *  No TGBT minting.  No proxy upgrades.  No infinite supply backdoor.
 *  Revenue makes mining profitable.  Burns make TGBT deflationary.
 */
contract RandomnessShop is ModuleBase, ReentrancyGuard {
    using Math for uint256;

    // ── Constants ────────────────────────────────────────────
    uint256 private constant BPS_SCALE    = 10_000;
    uint256 public  constant MAX_MINER_BPS = 9_000;  // max 90% to miners
    uint256 public  constant MIN_MINER_BPS = 5_000;  // min 50% to miners
    uint256 public  constant MAX_FEE       = 10_000 ether; // sanity cap per request
    uint256 public  constant MIN_FEE       = 1;            // minimum fee per tier (1 wei TGBT)

    // ── Proof tiers ──────────────────────────────────────────
    enum ProofTier {
        Standard,     // Latest beacon output + Merkle proof
        Anchored,     // Standard + Bitcoin UTXO anchor certificate
        Enterprise    // Anchored + storage attestation + SLA receipt
    }

    // ── State ────────────────────────────────────────────────
    ITGBT  public tgbtToken;
    address public protocolTreasury;
    address public burnAddress;     // address(0xdead) or actual burn contract

    uint256 public minerShareBps;   // basis points of revenue to miners (default 7000 = 70%)
    uint256 public totalRevenue;    // lifetime TGBT collected
    uint256 public totalBurned;     // lifetime TGBT sent to burn
    uint256 public totalProofsSold; // lifetime proof count

    // ── Ossification flags ───────────────────────────────────
    bool public configLocked;       // true → governance params frozen forever
    bool public burnAddressLocked;  // true → burnAddress can never change

    // Fee per tier (in TGBT wei units)
    mapping(ProofTier => uint256) public tierFee;

    // ── Proof receipt (on-chain record) ──────────────────────
    struct ProofReceipt {
        address     buyer;
        ProofTier   tier;
        bytes32     beaconOutput;     // the randomness value delivered
        bytes32     proofHash;        // keccak256 of the full Merkle proof blob
        bytes32     anchorId;         // Bitcoin anchor ID (0x0 for Standard tier)
        uint256     fee;              // TGBT paid
        uint256     blockNumber;      // Arbitrum block when purchased
        uint64      timestamp;
    }

    uint256 public nextReceiptId;
    mapping(uint256 => ProofReceipt) public receipts;
    mapping(address => uint256[])    public buyerReceipts; // buyer → receipt IDs

    // ── Events ───────────────────────────────────────────────
    event ProofPurchased(
        uint256 indexed receiptId,
        address indexed buyer,
        ProofTier       tier,
        bytes32         beaconOutput,
        bytes32         proofHash,
        bytes32         anchorId,
        uint256         fee
    );
    event MinerShareBurned(address indexed buyer, uint256 amount);
    event MinerShareUpdated(uint256 oldBps, uint256 newBps);
    event TierFeeUpdated(ProofTier tier, uint256 oldFee, uint256 newFee);
    event TreasuryUpdated(address oldTreasury, address newTreasury);
    event ProtocolWithdrawal(address token, uint256 amount, address to);
    event ConfigLockedForever();
    event BurnAddressLockedForever(address burnAddr);
    event BurnAddressUpdated(address oldBurn, address newBurn);

    // ── Errors ───────────────────────────────────────────────
    error ZeroFee();
    error InsufficientAllowance();
    error TransferFailed();
    error InvalidMinerShare();
    error InvalidFee();
    error ZeroAddress();
    error InvalidTier();
    error InvalidProofHash();
    error ConfigIsLocked();
    error BurnAddressIsLocked();
    error AnchorIdRequired();
    error BurnAddressNotDead();

    // ═══════════════════════════════════════════════════════════
    //  Initialization
    // ═══════════════════════════════════════════════════════════

    function initialize(
        address coreAddress,
        address _tgbtToken,
        address _protocolTreasury,
        address _burnAddress,
        uint256 _standardFee,
        uint256 _anchoredFee,
        uint256 _enterpriseFee
    ) external {
        __ModuleBase_init(coreAddress);

        if (_tgbtToken == address(0))        revert ZeroAddress();
        if (_protocolTreasury == address(0)) revert ZeroAddress();
        if (_burnAddress == address(0))      revert ZeroAddress();
        if (_standardFee == 0)               revert ZeroFee();

        // Burn address must be provably dead: 0xdead or any non-zero non-contract
        // address. Disallow the contract's own address and the treasury.
        if (_burnAddress == address(this))    revert BurnAddressNotDead();
        if (_burnAddress == _protocolTreasury) revert BurnAddressNotDead();

        tgbtToken        = ITGBT(_tgbtToken);
        protocolTreasury = _protocolTreasury;
        burnAddress      = _burnAddress;
        minerShareBps    = 7_000; // 70% to miners by default

        tierFee[ProofTier.Standard]   = _standardFee;
        tierFee[ProofTier.Anchored]   = _anchoredFee   > 0 ? _anchoredFee   : _standardFee * 3;
        tierFee[ProofTier.Enterprise] = _enterpriseFee  > 0 ? _enterpriseFee  : _standardFee * 10;
    }

    // ═══════════════════════════════════════════════════════════
    //  Purchase — pay TGBT, get proof receipt
    // ═══════════════════════════════════════════════════════════

    /**
     * @notice Buy a randomness proof at the specified tier.
     * @param tier        Proof quality tier (Standard / Anchored / Enterprise)
     * @param proofHash   keccak256 of the off-chain Merkle proof blob the buyer received
     * @param anchorId    Bitcoin anchor ID (required for Anchored/Enterprise, 0x0 for Standard)
     * @return receiptId  On-chain receipt ID
     *
     * Flow:
     *   1. Buyer calls the off-chain Randomness API to get a proof + anchor
     *   2. Buyer calls buyProof() with the proof hash to get an on-chain receipt
     *   3. Contract records the purchase, burns the miner share, and sends the protocol share to treasury
     */
    function buyProof(
        ProofTier tier,
        bytes32   proofHash,
        bytes32   anchorId
    ) external nonReentrant whenSystemActive returns (uint256 receiptId) {
        uint256 fee = tierFee[tier];
        if (fee == 0) revert InvalidTier();
        if (proofHash == bytes32(0)) revert InvalidProofHash();

        // Tier-specific validation: Anchored and Enterprise require a Bitcoin anchor
        if (tier == ProofTier.Anchored  && anchorId == bytes32(0)) revert AnchorIdRequired();
        if (tier == ProofTier.Enterprise && anchorId == bytes32(0)) revert AnchorIdRequired();

        if (IERC20(address(tgbtToken)).allowance(msg.sender, address(this)) < fee) revert InsufficientAllowance();

        // Pull payment in TGBT
        bool ok = IERC20(address(tgbtToken)).transferFrom(msg.sender, address(this), fee);
        if (!ok) revert TransferFailed();

        // Read latest beacon output
        bytes32 beaconOutput = _latestBeaconOutput();

        // Record receipt
        receiptId = nextReceiptId++;
        receipts[receiptId] = ProofReceipt({
            buyer:         msg.sender,
            tier:          tier,
            beaconOutput:  beaconOutput,
            proofHash:     proofHash,
            anchorId:      anchorId,
            fee:           fee,
            blockNumber:   block.number,
            timestamp:     uint64(block.timestamp)
        });
        buyerReceipts[msg.sender].push(receiptId);

        // Split revenue
        uint256 minerAmount    = Math.mulDiv(fee, minerShareBps, BPS_SCALE);
        uint256 protocolAmount = fee - minerAmount;

        totalRevenue     += fee;
        totalProofsSold  += 1;

        // Burn miner share immediately so value flows to all TGBT holders/miners
        if (minerAmount > 0) {
            bool burned = IERC20(address(tgbtToken)).transfer(burnAddress, minerAmount);
            if (!burned) revert TransferFailed();
            totalBurned += minerAmount;
            emit MinerShareBurned(msg.sender, minerAmount);
        }

        // Send protocol share immediately
        if (protocolAmount > 0) {
            bool ok2 = IERC20(address(tgbtToken)).transfer(protocolTreasury, protocolAmount);
            if (!ok2) revert TransferFailed();
        }

        emit ProofPurchased(receiptId, msg.sender, tier, beaconOutput, proofHash, anchorId, fee);
    }

    // ═══════════════════════════════════════════════════════════
    //  Views
    // ═══════════════════════════════════════════════════════════

    /**
     * @notice Get a quote for a proof tier (TGBT amount).
     */
    function getQuote(ProofTier tier) external view returns (uint256 fee, uint256 minerShare, uint256 protocolShare) {
        fee = tierFee[tier];
        minerShare = Math.mulDiv(fee, minerShareBps, BPS_SCALE);
        protocolShare = fee - minerShare;
    }

    /**
     * @notice Marketplace health metrics.
     */
    function getMarketplaceStats()
        external
        view
        returns (
            uint256 lifetimeRevenue,
            uint256 lifetimeBurned,
            uint256 lifetimeProofs,
            uint256 currentMinerBps,
            uint256 standardFee,
            uint256 anchoredFee,
            uint256 enterpriseFee
        )
    {
        return (
            totalRevenue,
            totalBurned,
            totalProofsSold,
            minerShareBps,
            tierFee[ProofTier.Standard],
            tierFee[ProofTier.Anchored],
            tierFee[ProofTier.Enterprise]
        );
    }

    /**
     * @notice Get all receipt IDs for a buyer.
     */
    function getBuyerReceiptIds(address buyer) external view returns (uint256[] memory) {
        return buyerReceipts[buyer];
    }

    /**
     * @notice Get a specific receipt.
     */
    function getReceipt(uint256 receiptId)
        external
        view
        returns (
            address buyer,
            ProofTier tier,
            bytes32 beaconOutput,
            bytes32 proofHash,
            bytes32 anchorId,
            uint256 fee,
            uint256 blk,
            uint64  ts
        )
    {
        ProofReceipt storage r = receipts[receiptId];
        return (r.buyer, r.tier, r.beaconOutput, r.proofHash, r.anchorId, r.fee, r.blockNumber, r.timestamp);
    }

    /**
     * @notice Verify a proof receipt exists and matches expected values.
     *         Useful for off-chain integrations to confirm on-chain record.
     */
    function verifyReceipt(uint256 receiptId, bytes32 expectedProofHash)
        external
        view
        returns (bool valid, bytes32 beaconOutput, uint64 timestamp)
    {
        ProofReceipt storage r = receipts[receiptId];
        valid = r.proofHash == expectedProofHash && r.buyer != address(0);
        beaconOutput = r.beaconOutput;
        timestamp = r.timestamp;
    }

    // ═══════════════════════════════════════════════════════════
    //  Governance — fee tuning (no minting, no upgrades)
    //  All config setters gated by configLocked.
    // ═══════════════════════════════════════════════════════════

    modifier whenConfigUnlocked() {
        if (configLocked) revert ConfigIsLocked();
        _;
    }

    function setMinerShare(uint256 newBps) external onlyGovernance whenConfigUnlocked {
        if (newBps < MIN_MINER_BPS || newBps > MAX_MINER_BPS) revert InvalidMinerShare();
        emit MinerShareUpdated(minerShareBps, newBps);
        minerShareBps = newBps;
    }

    function setTierFee(ProofTier tier, uint256 newFee) external onlyGovernance whenConfigUnlocked {
        if (newFee < MIN_FEE || newFee > MAX_FEE) revert InvalidFee();
        emit TierFeeUpdated(tier, tierFee[tier], newFee);
        tierFee[tier] = newFee;
    }

    function setTreasury(address newTreasury) external onlyGovernance whenConfigUnlocked {
        if (newTreasury == address(0)) revert ZeroAddress();
        if (newTreasury == burnAddress) revert ZeroAddress(); // treasury must differ from burn
        emit TreasuryUpdated(protocolTreasury, newTreasury);
        protocolTreasury = newTreasury;
    }

    /**
     * @notice Update the burn address. Only before burnAddressLocked.
     * @dev    New address must not be zero, this contract, or the treasury.
     */
    function setBurnAddress(address newBurn) external onlyGovernance whenConfigUnlocked {
        if (burnAddressLocked) revert BurnAddressIsLocked();
        if (newBurn == address(0))       revert ZeroAddress();
        if (newBurn == address(this))    revert BurnAddressNotDead();
        if (newBurn == protocolTreasury) revert BurnAddressNotDead();
        emit BurnAddressUpdated(burnAddress, newBurn);
        burnAddress = newBurn;
    }

    // ── Ossification ─────────────────────────────────────────

    /**
     * @notice Permanently lock the burn address. Irreversible.
     *         Call once you're confident the burn sink is correct.
     */
    function lockBurnAddress() external onlyGovernance {
        burnAddressLocked = true;
        emit BurnAddressLockedForever(burnAddress);
    }

    /**
     * @notice Permanently freeze ALL governance config (fees, shares, treasury,
     *         burn address). After this call, governance has zero power over
     *         contract parameters. Irreversible. Bitcoin-style ossification.
     * @dev    Emergency withdraw remains available (cannot strand tokens forever)
     *         but no parameters can ever change.
     */
    function lockConfig() external onlyGovernance {
        configLocked = true;
        burnAddressLocked = true; // implicitly lock burn address too
        emit ConfigLockedForever();
        emit BurnAddressLockedForever(burnAddress);
    }

    /**
     * @notice Withdraw accidentally stranded tokens from this contract.
     *         Normal purchases should leave no balance except accidental transfers.
     *         Note: This remains available even after lockConfig() — you cannot
     *         strand tokens forever. But it can only send to the treasury.
     */
    function emergencyWithdraw(address token, uint256 amount) external onlyGovernance {
        if (amount == 0) revert ZeroFee();
        bool ok = IERC20(token).transfer(protocolTreasury, amount);
        if (!ok) revert TransferFailed();
        emit ProtocolWithdrawal(token, amount, protocolTreasury);
    }

    // ═══════════════════════════════════════════════════════════
    //  Internal
    // ═══════════════════════════════════════════════════════════

    function _latestBeaconOutput() internal view returns (bytes32) {
        uint64 idx = _currentOutputIndex();
        if (idx == 0) return bytes32(0);
        bytes32[32] memory history = _outputHistory();
        return history[(idx - 1) % 32];
    }
}
