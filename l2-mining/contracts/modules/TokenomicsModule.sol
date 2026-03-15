// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { ModuleBase } from "./ModuleBase.sol";
import { ITGBT } from "../interfaces/ITGBT.sol";
import { ITokenomicsModule } from "../interfaces/ITokenomicsModule.sol";
import { TokenomicsLib } from "../TokenomicsLib.sol";
import { Math } from "@openzeppelin/contracts/utils/math/Math.sol";

contract TokenomicsModule is ModuleBase, ITokenomicsModule {
    using TokenomicsLib for TokenomicsLib.EpochState;
    using Math for uint256;

    bytes32 public constant MODULE_MINING = keccak256("MINING_MODULE");
    bytes32 public constant MODULE_BATCH_MINING = keccak256("BATCH_MINING_MODULE");
    bytes32 public constant SLASHER_ROLE = keccak256("SLASHER_ROLE");
    bytes32 public constant BURNER_ROLE = keccak256("BURNER_ROLE");

    uint256 public constant TOTAL_SUPPLY_CAP = 2_000_000_000 ether;
    uint256 public constant MINING_ALLOCATION = 700_000_000 ether;
    uint256 public constant MAX_BONUS_MULTIPLIER = 500;
    uint256 public constant DEFAULT_BONUS_THRESHOLD = 2;
    uint16 public constant DEFAULT_BONUS_MULTIPLIER = 125;

    bytes32 public constant RULE_VIOLATION = keccak256("RULE_VIOLATION");
    bytes32 public constant MALICIOUS_BEHAVIOR = keccak256("MALICIOUS");
    bytes32 public constant INACTIVITY = keccak256("INACTIVITY");
    bytes32 public constant MISSED_ENTROPY = keccak256("MISSED_ENTROPY");

    uint8 public constant VIOLATION_TYPE_RULE = 1;
    uint8 public constant VIOLATION_TYPE_MALICIOUS = 2;
    uint8 public constant BURN_TYPE_INACTIVITY = 1;
    uint8 public constant BURN_TYPE_MISSED = 2;

    ITGBT public tgbtToken;
    TokenomicsLib.EpochState internal epochState;
    uint256 public totalMined;
    uint256 public bonusThreshold;
    uint16 public bonusMultiplier;

    mapping(address => uint256) public lastActivityBlock;
    mapping(address => uint256) public missedContributions;

    event TokenUpdated(address newToken);
    event AutoSlashed(address indexed account, uint8 violationType, uint8 severity, uint256 amount);
    event AutoBurned(address indexed account, uint8 burnType, uint256 parameter, uint256 amount);
    event ExceptionalSolution(address indexed miner, uint256 difficulty, uint256 threshold, uint256 multiplier);

    error OnlyMiningModule();
    error ZeroToken();
    error InvalidMultiplier();
    error InvalidThreshold();
    error InvalidSeverity();
    error InvalidViolationType();
    error UnauthorizedRole(bytes32 role, address caller);

    function initialize(
        address coreAddress,
        address tokenAddress,
        uint256 initialReward,
        uint256 blocksPerEpoch,
        uint256 halvingInterval,
        uint256 initialBonusThreshold,
        uint16 initialBonusMultiplier
    ) external {
        __ModuleBase_init(coreAddress);

        if (tokenAddress == address(0)) revert ZeroToken();

        tgbtToken = ITGBT(tokenAddress);
        epochState.currentEpoch = 0;
        epochState.blocksPerEpoch = blocksPerEpoch;
        epochState.epochStartBlock = block.number;
        epochState.lastHalvingBlock = block.number;
        epochState.halvingInterval = halvingInterval;
        epochState.rewardAmount = initialReward;
        totalMined = 0;
        bonusThreshold = initialBonusThreshold == 0 ? DEFAULT_BONUS_THRESHOLD : initialBonusThreshold;
        bonusMultiplier = initialBonusMultiplier == 0 ? DEFAULT_BONUS_MULTIPLIER : initialBonusMultiplier;
    }

    function onBlockMined(
        address miner,
        bytes32 output,
        uint8,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    ) external onlyAuthorizedMiningModule whenSystemActive returns (uint256 reward) {
        epochState.rewardAmount = TokenomicsLib.checkEpochTransition(epochState);
        reward = _calculateReward(output, epochState.rewardAmount, poolTargetDifficulty, poolTotalMined, poolEmissionBucket);

        if (reward > 0) {
            tgbtToken.mint(miner, reward);
            totalMined += reward;
            _updateActivity(miner);
        }
    }

    function onManualSlash(address account, uint256 amount, bytes32 reason) external onlyCoreOrModule whenSystemActive {
        if (amount == 0) {
            return;
        }
        tgbtToken.slash(account, amount, reason);
        _updateActivity(account);
    }

    function setTGBTToken(address newToken) external onlyGovernance {
        if (newToken == address(0)) revert ZeroToken();
        tgbtToken = ITGBT(newToken);
        emit TokenUpdated(newToken);
    }

    function setBonusParameters(uint16 multiplier, uint256 threshold) external onlyGovernance {
        if (multiplier == 0 || multiplier > MAX_BONUS_MULTIPLIER) revert InvalidMultiplier();
        if (threshold == 0) revert InvalidThreshold();

        bonusMultiplier = multiplier;
        bonusThreshold = threshold;
    }

    function setEpochBlocks(uint256 newBlocksPerEpoch) external onlyGovernance {
        TokenomicsLib.setEpochBlocks(epochState, newBlocksPerEpoch);
    }

    function setHalvingInterval(uint256 newHalvingInterval) external onlyGovernance {
        TokenomicsLib.setHalvingInterval(epochState, newHalvingInterval);
    }

    function autoSlash(address account, uint8 violationType, uint8 severity)
        external
        whenSystemActive
        returns (uint256)
    {
        _requireRole(SLASHER_ROLE);
        if (severity == 0 || severity > 100) revert InvalidSeverity();

        bytes32 reason;
        uint256 baseAmount;

        if (violationType == VIOLATION_TYPE_RULE) {
            baseAmount = 100 ether;
            reason = RULE_VIOLATION;
        } else if (violationType == VIOLATION_TYPE_MALICIOUS) {
            baseAmount = 1000 ether;
            reason = MALICIOUS_BEHAVIOR;
        } else {
            revert InvalidViolationType();
        }

        uint256 amountToSlash = (baseAmount * severity) / 100;
        uint256 balance = tgbtToken.balanceOf(account);
        uint256 actualAmount = amountToSlash > balance ? balance : amountToSlash;

        if (actualAmount > 0) {
            tgbtToken.slash(account, actualAmount, reason);
            emit AutoSlashed(account, violationType, severity, actualAmount);
            _updateActivity(account);
        }

        return actualAmount;
    }

    function checkInactivity(address account) external whenSystemActive {
        _requireRole(BURNER_ROLE);

        if (lastActivityBlock[account] == 0) {
            return;
        }

        uint256 inactiveBlocks = block.number - lastActivityBlock[account];
        uint256 inactiveDays = (inactiveBlocks * 15) / 86400;

        if (inactiveDays <= 30) return;

        uint256 burnPercent = ((inactiveDays - 30) / 30) + 1;
        if (burnPercent > 10) burnPercent = 10;

        uint256 balance = tgbtToken.balanceOf(account);
        uint256 burnAmount = (balance * burnPercent) / 100;

        if (burnAmount > 0) {
            tgbtToken.burnFromBeacon(account, burnAmount, INACTIVITY);
            emit AutoBurned(account, BURN_TYPE_INACTIVITY, inactiveDays, burnAmount);
        }

        _updateActivity(account);
    }

    function recordMissedContribution(address contributor) external whenSystemActive {
        _requireRole(BURNER_ROLE);

        missedContributions[contributor]++;

        if (missedContributions[contributor] >= 3) {
            uint256 missedCount = missedContributions[contributor];
            uint256 burnAmount = 5 ether * missedCount;
            uint256 balance = tgbtToken.balanceOf(contributor);
            uint256 actualBurn = burnAmount > balance ? balance : burnAmount;

            if (actualBurn > 0) {
                tgbtToken.burnFromBeacon(contributor, actualBurn, MISSED_ENTROPY);
                emit AutoBurned(contributor, BURN_TYPE_MISSED, missedCount, actualBurn);
                _updateActivity(contributor);
            }

            missedContributions[contributor] = 0;
        }
    }

    function resetMissedContributions(address account) external onlyGovernance {
        missedContributions[account] = 0;
    }

    function getMiningEconomics()
        external
        view
        returns (
            uint256 currentReward,
            uint256 currentEpoch,
            uint256 blocksPerEpoch,
            uint256 halvingInterval,
            uint256 nextHalvingBlock,
            uint256 currentBonusThreshold,
            uint256 currentBonusMultiplier,
            uint256 minedSoFar,
            uint256 remainingAllocation
        )
    {
        return (
            epochState.rewardAmount,
            epochState.currentEpoch,
            epochState.blocksPerEpoch,
            epochState.halvingInterval,
            epochState.lastHalvingBlock + epochState.halvingInterval,
            bonusThreshold,
            bonusMultiplier,
            totalMined,
            MINING_ALLOCATION > totalMined ? MINING_ALLOCATION - totalMined : 0
        );
    }

    function getTokenomicsInfo()
        external
        view
        returns (
            uint256 cap,
            uint256 miningAlloc,
            uint256 currentBlockReward,
            uint256 epoch,
            uint256 totalMinedToDate,
            uint256 remaining,
            uint256 nextHalvingBlock
        )
    {
        return TokenomicsLib.getTokenomicsInfo(epochState, TOTAL_SUPPLY_CAP, MINING_ALLOCATION, totalMined);
    }

    function getAccountPenaltyState(address account)
        external
        view
        returns (uint256 lastActivity, uint256 missedContributionCount)
    {
        return (lastActivityBlock[account], missedContributions[account]);
    }

    function onlyMiningModuleAddress() external view returns (address) {
        return _module(MODULE_MINING);
    }

    function onlyBatchMiningModuleAddress() external view returns (address) {
        return _module(MODULE_BATCH_MINING);
    }

    function _calculateReward(
        bytes32 output,
        uint256 baseReward,
        uint256 poolTargetDifficulty,
        uint256 poolTotalMined,
        uint256 poolEmissionBucket
    ) internal returns (uint256 reward) {
        uint256 difficulty = type(uint256).max - uint256(output);
        reward = baseReward;

        uint256 bonusTarget = Math.mulDiv(poolTargetDifficulty, bonusThreshold, 1);
        if (difficulty > bonusTarget) {
            reward = Math.mulDiv(baseReward, bonusMultiplier, 100);
            emit ExceptionalSolution(msg.sender, difficulty, bonusTarget, bonusMultiplier);
        }

        if (totalMined + reward > MINING_ALLOCATION) {
            reward = MINING_ALLOCATION > totalMined ? MINING_ALLOCATION - totalMined : 0;
        }

        if (poolTotalMined + reward > poolEmissionBucket) {
            reward = poolEmissionBucket > poolTotalMined ? poolEmissionBucket - poolTotalMined : 0;
        }
    }

    function _updateActivity(address account) internal {
        lastActivityBlock[account] = block.number;
    }

    function _requireRole(bytes32 role) internal view {
        if (!core.hasRole(role, msg.sender)) revert UnauthorizedRole(role, msg.sender);
    }

    modifier onlyAuthorizedMiningModule() {
        address miningModule = _module(MODULE_MINING);
        address batchMiningModule = _module(MODULE_BATCH_MINING);
        if (msg.sender != miningModule && msg.sender != batchMiningModule) revert OnlyMiningModule();
        _;
    }
}