// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./ModuleBase.sol";
import { ITGBT } from "../interfaces/ITGBT.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";
import { MerkleProof } from "@openzeppelin/contracts/utils/cryptography/MerkleProof.sol";
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { EIP712 } from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import { ReentrancyGuard } from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/**
 * @title UniversalMinerGasPool
 * @notice Shared ETH reimbursement vault for miners on Arbitrum L2.
 *
 * Security / decentralization design:
 * - Sponsors deposit ETH into a shared pool and receive proportional shares.
 * - The contract does NOT blindly forward mining calls. That pattern is unsafe for
 *   this system because current mining modules rely on `msg.sender` being the miner.
 * - Instead, reimbursement claims are posted as Merkle epochs signed by a threshold
 *   set of attestors. Attestors observe real Arbitrum transactions and publish a
 *   reimbursement root with a bounded budget and claim deadline.
 * - Miners claim ETH refunds directly by proving inclusion in an epoch.
 * - Optional TGBT fees can be collected from claimants and distributed pro-rata to
 *   sponsors, creating a sustainable sponsorship market.
 * - Governance can configure the attestor set, target allowlist, and threshold,
 *   then permanently lock those powers.
 */
contract UniversalMinerGasPool is ModuleBase, EIP712("TGBTGasPool", "1"), ReentrancyGuard {
    using SafeERC20 for IERC20;
    using Math for uint256;

    uint256 private constant REWARD_PRECISION = 1e24;

    bytes32 public constant MODULE_MINING = keccak256("MINING_MODULE");
    bytes32 public constant MODULE_BATCH_MINING = keccak256("BATCH_MINING_MODULE");
    bytes32 public constant MODULE_STALE_BLOCK = keccak256("STALE_BLOCK_MODULE");

    bytes32 private constant EPOCH_TYPEHASH = keccak256(
        "ReimbursementEpoch(uint256 epochId,bytes32 merkleRoot,uint256 budgetWei,uint64 claimDeadline)"
    );

    ITGBT public tgbtToken;

    uint256 public totalShares;
    mapping(address => uint256) public sponsorShares;

    uint256 public accTgbtPerShare;
    mapping(address => uint256) public rewardDebt;
    mapping(address => uint256) public unclaimedTgbt;

    mapping(address => bool) public isAttestor;
    address[] private _attestors;
    uint256 public attestorThreshold;
    bool public attestorsLocked;

    mapping(address => bool) public approvedTarget;
    mapping(address => mapping(bytes4 => bool)) public approvedSelector;
    bool public targetsLocked;

    uint256 public maxRefundPerClaim;
    uint256 public maxTgbtFeePerClaim;
    uint64  public minClaimWindow;
    uint256 public reservedEth;
    uint256 public totalEthDeposited;
    uint256 public totalEthRefunded;
    uint256 public totalSponsoredClaims;
    uint256 public totalTgbtFeesCollected;

    struct ReimbursementEpoch {
        bytes32 merkleRoot;
        uint256 budgetWei;
        uint256 claimedWei;
        uint64 claimDeadline;
        uint64 postedAt;
        bool released;
    }

    mapping(uint256 => ReimbursementEpoch) public epochs;
    mapping(bytes32 => bool) public claimedLeaf;
    mapping(bytes32 => bool) public claimedTx;

    event SponsorDeposited(address indexed sponsor, uint256 amount, uint256 sharesMinted);
    event SponsorWithdrawn(address indexed sponsor, uint256 ethAmount, uint256 sharesBurned);
    event ReimbursementEpochPosted(
        uint256 indexed epochId,
        bytes32 indexed merkleRoot,
        uint256 budgetWei,
        uint64 claimDeadline,
        address indexed caller
    );
    event ReimbursementClaimed(
        uint256 indexed epochId,
        address indexed miner,
        address indexed target,
        bytes4 selector,
        bytes32 txHash,
        uint256 refundWei,
        uint256 tgbtFee
    );
    event ReimbursementBudgetReleased(uint256 indexed epochId, uint256 releasedWei);
    event AttestorSetLocked(uint256 attestorCount, uint256 threshold);
    event TargetsLocked();
    event AttestorUpdated(address indexed attestor, bool allowed);
    event AttestorThresholdUpdated(uint256 oldThreshold, uint256 newThreshold);
    event TargetApprovalSet(address indexed target, bool approved);
    event SelectorApprovalSet(address indexed target, bytes4 indexed selector, bool approved);
    event PolicyUpdated(uint256 maxRefundPerClaim, uint256 maxTgbtFeePerClaim, uint64 minClaimWindow);
    event TgbtFeesDistributed(uint256 amount);
    event TgbtClaimed(address indexed sponsor, uint256 amount);
    event DonationReceived(address indexed from, uint256 amount);

    error ZeroAddress();
    error ZeroAmount();
    error PoolEmpty();
    error NoShares();
    error NoRewards();
    error InvalidThreshold();
    error AttestorsAreLocked();
    error TargetsAreLocked();
    error EpochAlreadyExists(uint256 epochId);
    error EpochNotFound(uint256 epochId);
    error EpochExpired(uint256 epochId);
    error EpochNotExpired(uint256 epochId);
    error InvalidBudget();
    error InvalidClaimWindow();
    error InvalidTarget();
    error UnsupportedAction(address target, bytes4 selector);
    error InvalidSignature();
    error DuplicateSigner();
    error ClaimAlreadyUsed();
    error RefundTooLarge();
    error TgbtFeeTooLarge();
    error TransferFailed();

    function initialize(
        address coreAddress,
        address tgbtTokenAddress,
        uint256 initialMaxRefundPerClaim,
        uint256 initialMaxTgbtFeePerClaim,
        uint64 initialMinClaimWindow,
        address[] calldata initialAttestors,
        uint256 initialThreshold
    ) external {
        __ModuleBase_init(coreAddress);

        if (tgbtTokenAddress == address(0)) revert ZeroAddress();
        if (initialMaxRefundPerClaim == 0) revert InvalidBudget();
        if (initialMinClaimWindow == 0) revert InvalidClaimWindow();

        tgbtToken = ITGBT(tgbtTokenAddress);
        maxRefundPerClaim = initialMaxRefundPerClaim;
        maxTgbtFeePerClaim = initialMaxTgbtFeePerClaim;
        minClaimWindow = initialMinClaimWindow;

        _setAttestors(initialAttestors, initialThreshold);
    }

    function deposit() external payable nonReentrant whenSystemActive returns (uint256 sharesMinted) {
        if (msg.value == 0) revert ZeroAmount();

        _harvest(msg.sender);

        uint256 freeBefore = _freeLiquidity();
        uint256 poolBefore = freeBefore - msg.value;
        sharesMinted = totalShares == 0 || poolBefore == 0
            ? msg.value
            : Math.mulDiv(msg.value, totalShares, poolBefore);

        sponsorShares[msg.sender] += sharesMinted;
        totalShares += sharesMinted;
        rewardDebt[msg.sender] = Math.mulDiv(sponsorShares[msg.sender], accTgbtPerShare, REWARD_PRECISION);
        totalEthDeposited += msg.value;

        emit SponsorDeposited(msg.sender, msg.value, sharesMinted);
    }

    function withdraw(uint256 shareAmount) external nonReentrant returns (uint256 ethAmount) {
        if (shareAmount == 0) revert ZeroAmount();
        if (sponsorShares[msg.sender] < shareAmount) revert NoShares();

        _harvest(msg.sender);

        ethAmount = previewWithdraw(msg.sender, shareAmount);
        if (ethAmount == 0) revert PoolEmpty();

        sponsorShares[msg.sender] -= shareAmount;
        totalShares -= shareAmount;
        rewardDebt[msg.sender] = Math.mulDiv(sponsorShares[msg.sender], accTgbtPerShare, REWARD_PRECISION);

        (bool sent, ) = payable(msg.sender).call{value: ethAmount}("");
        if (!sent) revert TransferFailed();

        emit SponsorWithdrawn(msg.sender, ethAmount, shareAmount);
    }

    function claimTgbt() external nonReentrant returns (uint256 amount) {
        _harvest(msg.sender);
        amount = unclaimedTgbt[msg.sender];
        if (amount == 0) revert NoRewards();

        unclaimedTgbt[msg.sender] = 0;
        rewardDebt[msg.sender] = Math.mulDiv(sponsorShares[msg.sender], accTgbtPerShare, REWARD_PRECISION);
        IERC20(address(tgbtToken)).safeTransfer(msg.sender, amount);

        emit TgbtClaimed(msg.sender, amount);
    }

    /**
     * @notice Post a threshold-signed reimbursement root.
     * @dev Root leaves are expected to be:
     *      keccak256(abi.encode(epochId, miner, target, selector, txHash, refundWei, tgbtFee, leafDeadline))
     */
    function postReimbursementEpoch(
        uint256 epochId,
        bytes32 merkleRoot,
        uint256 budgetWei,
        uint64 claimDeadline,
        bytes[] calldata signatures
    ) external nonReentrant whenSystemActive {
        if (epochs[epochId].merkleRoot != bytes32(0)) revert EpochAlreadyExists(epochId);
        if (merkleRoot == bytes32(0)) revert InvalidBudget();
        if (budgetWei == 0 || budgetWei > _freeLiquidity()) revert InvalidBudget();
        if (claimDeadline <= block.timestamp + minClaimWindow) revert InvalidClaimWindow();

        bytes32 digest = _hashTypedDataV4(
            keccak256(abi.encode(EPOCH_TYPEHASH, epochId, merkleRoot, budgetWei, claimDeadline))
        );
        _checkThresholdSignatures(digest, signatures);

        epochs[epochId] = ReimbursementEpoch({
            merkleRoot: merkleRoot,
            budgetWei: budgetWei,
            claimedWei: 0,
            claimDeadline: claimDeadline,
            postedAt: uint64(block.timestamp),
            released: false
        });
        reservedEth += budgetWei;

        emit ReimbursementEpochPosted(epochId, merkleRoot, budgetWei, claimDeadline, msg.sender);
    }

    function claimRefund(
        uint256 epochId,
        address target,
        bytes4 selector,
        bytes32 txHash,
        uint256 refundWei,
        uint256 tgbtFee,
        uint64 leafDeadline,
        bytes32[] calldata proof
    ) external nonReentrant whenSystemActive {
        if (target == address(0)) revert InvalidTarget();
        if (!approvedTarget[target] || !approvedSelector[target][selector]) {
            revert UnsupportedAction(target, selector);
        }
        if (refundWei == 0 || refundWei > maxRefundPerClaim) revert RefundTooLarge();
        if (tgbtFee > maxTgbtFeePerClaim) revert TgbtFeeTooLarge();
        if (claimedTx[txHash]) revert ClaimAlreadyUsed();

        ReimbursementEpoch storage epoch = epochs[epochId];
        if (epoch.merkleRoot == bytes32(0)) revert EpochNotFound(epochId);
        if (epoch.released || block.timestamp > epoch.claimDeadline || block.timestamp > leafDeadline) {
            revert EpochExpired(epochId);
        }

        bytes32 leaf = keccak256(
            abi.encode(epochId, msg.sender, target, selector, txHash, refundWei, tgbtFee, leafDeadline)
        );
        if (claimedLeaf[leaf]) revert ClaimAlreadyUsed();
        if (!MerkleProof.verifyCalldata(proof, epoch.merkleRoot, leaf)) revert InvalidSignature();
        if (epoch.claimedWei + refundWei > epoch.budgetWei) revert InvalidBudget();

        claimedLeaf[leaf] = true;
        claimedTx[txHash] = true;
        epoch.claimedWei += refundWei;
        reservedEth -= refundWei;
        totalEthRefunded += refundWei;
        totalSponsoredClaims += 1;

        if (tgbtFee > 0) {
            IERC20(address(tgbtToken)).safeTransferFrom(msg.sender, address(this), tgbtFee);
            _distributeTgbtFees(tgbtFee);
        }

        (bool sent, ) = payable(msg.sender).call{value: refundWei}("");
        if (!sent) revert TransferFailed();

        emit ReimbursementClaimed(epochId, msg.sender, target, selector, txHash, refundWei, tgbtFee);
    }

    function releaseExpiredBudget(uint256 epochId) external nonReentrant {
        ReimbursementEpoch storage epoch = epochs[epochId];
        if (epoch.merkleRoot == bytes32(0)) revert EpochNotFound(epochId);
        if (epoch.released) revert EpochExpired(epochId);
        if (block.timestamp <= epoch.claimDeadline) revert EpochNotExpired(epochId);

        uint256 unreleased = epoch.budgetWei - epoch.claimedWei;
        epoch.released = true;
        if (unreleased > 0) {
            reservedEth -= unreleased;
        }

        emit ReimbursementBudgetReleased(epochId, unreleased);
    }

    function setGasPolicy(
        uint256 newMaxRefundPerClaim,
        uint256 newMaxTgbtFeePerClaim,
        uint64 newMinClaimWindow
    ) external onlyGovernance {
        if (newMaxRefundPerClaim == 0) revert InvalidBudget();
        if (newMinClaimWindow == 0) revert InvalidClaimWindow();

        maxRefundPerClaim = newMaxRefundPerClaim;
        maxTgbtFeePerClaim = newMaxTgbtFeePerClaim;
        minClaimWindow = newMinClaimWindow;

        emit PolicyUpdated(newMaxRefundPerClaim, newMaxTgbtFeePerClaim, newMinClaimWindow);
    }

    function setAttestor(address attestor, bool allowed) external onlyGovernance {
        if (attestorsLocked) revert AttestorsAreLocked();
        if (attestor == address(0)) revert ZeroAddress();

        bool exists = isAttestor[attestor];
        if (exists == allowed) {
            emit AttestorUpdated(attestor, allowed);
            return;
        }

        isAttestor[attestor] = allowed;
        if (allowed) {
            _attestors.push(attestor);
        }

        emit AttestorUpdated(attestor, allowed);
    }

    function setAttestorThreshold(uint256 newThreshold) external onlyGovernance {
        if (attestorsLocked) revert AttestorsAreLocked();
        uint256 activeCount = _activeAttestorCount();
        if (newThreshold == 0 || newThreshold > activeCount) revert InvalidThreshold();

        emit AttestorThresholdUpdated(attestorThreshold, newThreshold);
        attestorThreshold = newThreshold;
    }

    function lockAttestors() external onlyGovernance {
        if (_activeAttestorCount() == 0 || attestorThreshold == 0) revert InvalidThreshold();
        attestorsLocked = true;
        emit AttestorSetLocked(_activeAttestorCount(), attestorThreshold);
    }

    function setTargetApproval(address target, bool approved) external onlyGovernance {
        if (targetsLocked) revert TargetsAreLocked();
        if (target == address(0)) revert ZeroAddress();
        approvedTarget[target] = approved;
        emit TargetApprovalSet(target, approved);
    }

    function setSelectorApproval(address target, bytes4 selector, bool approved) external onlyGovernance {
        if (targetsLocked) revert TargetsAreLocked();
        if (target == address(0)) revert ZeroAddress();
        approvedSelector[target][selector] = approved;
        emit SelectorApprovalSet(target, selector, approved);
    }

    function syncDefaultTargets() external onlyGovernance {
        if (targetsLocked) revert TargetsAreLocked();

        address miningModule = _module(MODULE_MINING);
        address batchMiningModule = _module(MODULE_BATCH_MINING);
        address staleBlockModule = _module(MODULE_STALE_BLOCK);

        if (miningModule != address(0)) {
            approvedTarget[miningModule] = true;
            emit TargetApprovalSet(miningModule, true);
        }
        if (batchMiningModule != address(0)) {
            approvedTarget[batchMiningModule] = true;
            emit TargetApprovalSet(batchMiningModule, true);
        }
        if (staleBlockModule != address(0)) {
            approvedTarget[staleBlockModule] = true;
            emit TargetApprovalSet(staleBlockModule, true);
        }
    }

    function enableDefaultSelectors() external onlyGovernance {
        if (targetsLocked) revert TargetsAreLocked();

        address miningModule = _module(MODULE_MINING);
        address batchMiningModule = _module(MODULE_BATCH_MINING);
        address staleBlockModule = _module(MODULE_STALE_BLOCK);

        if (miningModule != address(0)) {
            _setSelector(miningModule, bytes4(keccak256("submitMiningCommitment(bytes32,uint8,uint256,uint256,bytes)")), true);
            _setSelector(miningModule, bytes4(keccak256("revealMiningCommitment(bytes32,bytes,uint64,bytes,bytes32,uint8)")), true);
        }
        if (batchMiningModule != address(0)) {
            _setSelector(batchMiningModule, bytes4(keccak256("commitEpochRoot(uint256,bytes32,uint32,uint8,uint256,bytes)")), true);
            _setSelector(batchMiningModule, bytes4(keccak256("finalizeEpoch(uint256)")), true);
            _setSelector(batchMiningModule, bytes4(keccak256("recordStorageAttestation(uint256,bytes32)")), true);
        }
        if (staleBlockModule != address(0)) {
            _setSelector(staleBlockModule, bytes4(keccak256("submitStaleBlock(bytes,uint64,bytes32,uint32)")), true);
            _setSelector(staleBlockModule, bytes4(keccak256("claimReward(bytes32)")), true);
        }
    }

    function lockTargets() external onlyGovernance {
        targetsLocked = true;
        emit TargetsLocked();
    }

    function previewWithdraw(address, uint256 shareAmount) public view returns (uint256 ethAmount) {
        if (shareAmount == 0 || totalShares == 0) return 0;
        ethAmount = Math.mulDiv(_freeLiquidity(), shareAmount, totalShares);
    }

    function pendingTgbt(address sponsor) public view returns (uint256 amount) {
        uint256 accumulated = Math.mulDiv(sponsorShares[sponsor], accTgbtPerShare, REWARD_PRECISION);
        if (accumulated < rewardDebt[sponsor]) {
            return unclaimedTgbt[sponsor];
        }
        amount = unclaimedTgbt[sponsor] + (accumulated - rewardDebt[sponsor]);
    }

    function getAttestors() external view returns (address[] memory activeAttestors) {
        uint256 activeCount = _activeAttestorCount();
        activeAttestors = new address[](activeCount);
        uint256 cursor;
        for (uint256 i = 0; i < _attestors.length; i++) {
            address attestor = _attestors[i];
            if (isAttestor[attestor]) {
                activeAttestors[cursor++] = attestor;
            }
        }
    }

    function getPoolStats()
        external
        view
        returns (
            uint256 ethBalance,
            uint256 freeBalance,
            uint256 reservedBalance,
            uint256 shares,
            uint256 deposited,
            uint256 refunded,
            uint256 claimsSponsored,
            uint256 tgbtFees
        )
    {
        return (
            address(this).balance,
            _freeLiquidity(),
            reservedEth,
            totalShares,
            totalEthDeposited,
            totalEthRefunded,
            totalSponsoredClaims,
            totalTgbtFeesCollected
        );
    }

    function isSupportedAction(address target, bytes4 selector) external view returns (bool) {
        return approvedTarget[target] && approvedSelector[target][selector];
    }

    function _setAttestors(address[] calldata attestors, uint256 threshold) internal {
        uint256 len = attestors.length;
        if (len == 0 || threshold == 0 || threshold > len) revert InvalidThreshold();

        for (uint256 i = 0; i < len; i++) {
            address attestor = attestors[i];
            if (attestor == address(0)) revert ZeroAddress();
            if (isAttestor[attestor]) revert DuplicateSigner();
            isAttestor[attestor] = true;
            _attestors.push(attestor);
            emit AttestorUpdated(attestor, true);
        }

        attestorThreshold = threshold;
        emit AttestorThresholdUpdated(0, threshold);
    }

    function _checkThresholdSignatures(bytes32 digest, bytes[] calldata signatures) internal view {
        if (signatures.length < attestorThreshold) revert InvalidThreshold();

        address[] memory seen = new address[](signatures.length);
        uint256 validCount;

        for (uint256 i = 0; i < signatures.length; i++) {
            address signer = ECDSA.recover(digest, signatures[i]);
            if (!isAttestor[signer]) revert InvalidSignature();

            for (uint256 j = 0; j < validCount; j++) {
                if (seen[j] == signer) revert DuplicateSigner();
            }

            seen[validCount] = signer;
            validCount++;
        }

        if (validCount < attestorThreshold) revert InvalidThreshold();
    }

    function _setSelector(address target, bytes4 selector, bool approved) internal {
        approvedSelector[target][selector] = approved;
        emit SelectorApprovalSet(target, selector, approved);
    }

    function _activeAttestorCount() internal view returns (uint256 count) {
        for (uint256 i = 0; i < _attestors.length; i++) {
            if (isAttestor[_attestors[i]]) count++;
        }
    }

    function _freeLiquidity() internal view returns (uint256) {
        return address(this).balance > reservedEth ? address(this).balance - reservedEth : 0;
    }

    function _harvest(address sponsor) internal {
        uint256 shares = sponsorShares[sponsor];
        if (shares == 0) {
            rewardDebt[sponsor] = 0;
            return;
        }

        uint256 accumulated = Math.mulDiv(shares, accTgbtPerShare, REWARD_PRECISION);
        if (accumulated > rewardDebt[sponsor]) {
            unclaimedTgbt[sponsor] += accumulated - rewardDebt[sponsor];
        }
        rewardDebt[sponsor] = accumulated;
    }

    function _distributeTgbtFees(uint256 amount) internal {
        totalTgbtFeesCollected += amount;
        if (totalShares == 0) return;

        accTgbtPerShare += Math.mulDiv(amount, REWARD_PRECISION, totalShares);
        emit TgbtFeesDistributed(amount);
    }

    receive() external payable {
        emit DonationReceived(msg.sender, msg.value);
    }
}
