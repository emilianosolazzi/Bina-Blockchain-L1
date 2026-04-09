// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { TGL3EpochSettlement } from "./TGL3EpochSettlement.sol";
import { TGL3Treasury } from "./TGL3Treasury.sol";

/// @title TGL3ProofMarket
/// @notice Minimal proof-purchase settlement scaffold for the first L3 devnet.
/// @dev Scaffold only. Records economic receipts and references settled epochs.
contract TGL3ProofMarket is Ownable {
    enum ProofTier {
        Standard,
        Anchored,
        Enterprise
    }

    struct ProofReceipt {
        address buyer;
        uint256 epochId;
        ProofTier tier;
        bytes32 proofHash;
        bytes32 anchorId;
        uint256 feePaid;
        uint64 purchasedAt;
        string receiptUri;
    }

    uint256 public nextReceiptId;
    bool public paused;

    TGL3EpochSettlement public immutable epochSettlement;
    TGL3Treasury public immutable treasury;

    mapping(ProofTier => uint256) public tierFee;
    mapping(uint256 => ProofReceipt) public receipts;

    error MarketPaused();
    error InvalidProofHash();
    error InvalidTier();
    error UnknownEpoch();
    error AnchorRequired();

    event TierFeeUpdated(ProofTier indexed tier, uint256 amount);
    event MarketPauseUpdated(bool paused);
    event ProofPurchased(
        uint256 indexed receiptId,
        uint256 indexed epochId,
        address indexed buyer,
        ProofTier tier,
        bytes32 proofHash,
        bytes32 anchorId,
        uint256 feePaid,
        string receiptUri
    );

    constructor(
        address initialOwner,
        address epochSettlementAddress,
        address treasuryAddress,
        uint256 standardFee,
        uint256 anchoredFee,
        uint256 enterpriseFee
    ) Ownable(initialOwner) {
        epochSettlement = TGL3EpochSettlement(epochSettlementAddress);
        treasury = TGL3Treasury(treasuryAddress);
        tierFee[ProofTier.Standard] = standardFee;
        tierFee[ProofTier.Anchored] = anchoredFee;
        tierFee[ProofTier.Enterprise] = enterpriseFee;
    }

    function setPaused(bool paused_) external onlyOwner {
        paused = paused_;
        emit MarketPauseUpdated(paused_);
    }

    function setTierFee(ProofTier tier, uint256 amount) external onlyOwner {
        tierFee[tier] = amount;
        emit TierFeeUpdated(tier, amount);
    }

    function buyProof(
        uint256 epochId,
        ProofTier tier,
        bytes32 proofHash,
        bytes32 anchorId,
        string calldata receiptUri
    ) external returns (uint256 receiptId) {
        if (paused) revert MarketPaused();
        if (!epochSettlement.epochExists(epochId)) revert UnknownEpoch();
        if (proofHash == bytes32(0)) revert InvalidProofHash();

        uint256 fee = tierFee[tier];
        if (fee == 0) revert InvalidTier();
        if ((tier == ProofTier.Anchored || tier == ProofTier.Enterprise) && anchorId == bytes32(0)) {
            revert AnchorRequired();
        }

        receiptId = nextReceiptId++;
        bytes32 paymentReference = keccak256(abi.encodePacked("proof", receiptId, epochId, msg.sender, proofHash));
        treasury.collectPaymentFrom(msg.sender, fee, paymentReference);

        receipts[receiptId] = ProofReceipt({
            buyer: msg.sender,
            epochId: epochId,
            tier: tier,
            proofHash: proofHash,
            anchorId: anchorId,
            feePaid: fee,
            purchasedAt: uint64(block.timestamp),
            receiptUri: receiptUri
        });

        emit ProofPurchased(receiptId, epochId, msg.sender, tier, proofHash, anchorId, fee, receiptUri);
    }
}