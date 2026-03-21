// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { ModuleBase } from "./ModuleBase.sol";
import { RandomnessLib } from "../RandomnessLib.sol";

contract RandomnessModule is ModuleBase {
    bytes32 public constant EMERGENCY_ROLE = keccak256("EMERGENCY_ROLE");

    RandomnessLib.State internal randomnessState;

    event RandomnessRequested(uint256 indexed requestId, address indexed requester, bytes32 userSeed);
    event RandomnessContributionAdded(
        uint256 indexed requestId,
        address indexed contributor,
        bytes32 entropyContribution,
        uint256 contributionCount,
        uint256 minContributions
    );
    event RandomnessFulfilled(uint256 indexed requestId, bytes32 result);
    event EmergencyFeeParametersChanged(uint256 baseFee, uint256 feePerContributor);
    event RandomnessContributionParamsChanged(uint256 minContributions, uint256 maxContributions, uint256 expiryBlocks);
    event RandomnessTokenUpdated(address indexed token);

    error OnlyEmergency();
    error MinContributionsTooLow();
    error MaxLessThanMin();
    error MaxContributionsTooHigh();
    error ZeroAddress();

    function initialize(address coreAddress, address tgbtTokenAddress) external {
        __ModuleBase_init(coreAddress);
        if (tgbtTokenAddress == address(0)) revert ZeroAddress();

        randomnessState.tgbtTokenAddress = tgbtTokenAddress;
        randomnessState.baseEmergencyFee = 100 ether;
        randomnessState.feePerContributor = 10 ether;
        randomnessState.expiryBlocks = 50000;
        randomnessState.minContributions = 3;
        randomnessState.maxContributions = 10;
        randomnessState.maxBatchSize = 20;
    }

    function requestRandomness(bytes32 userSeed) external whenSystemActive returns (uint256 requestId) {
        requestId = RandomnessLib.createRequest(randomnessState, msg.sender, userSeed);
        emit RandomnessRequested(requestId, msg.sender, userSeed);
    }

    function contributeEntropy(uint256 requestId, bytes32 entropyContribution) external whenSystemActive {
        bool shouldFulfill = RandomnessLib.addContribution(randomnessState, requestId, msg.sender, entropyContribution);
        (, , , uint256 contributionCount) = RandomnessLib.getRequestState(randomnessState, requestId);

        emit RandomnessContributionAdded(
            requestId,
            msg.sender,
            entropyContribution,
            contributionCount,
            randomnessState.minContributions
        );

        if (shouldFulfill) {
            bytes32 result = RandomnessLib.fulfillRequest(
                randomnessState,
                requestId,
                _historicalHash(),
                bytes32(0)
            );
            emit RandomnessFulfilled(requestId, result);
        }
    }

    function getRandomResult(uint256 requestId) external view returns (bytes32) {
        return RandomnessLib.getRandomness(randomnessState, requestId);
    }

    function emergencyRandomnessFulfill(uint256 requestId, bytes32 entropyMerkleRoot) external whenSystemActive {
        if (!core.hasRole(EMERGENCY_ROLE, msg.sender)) revert OnlyEmergency();

        bytes32 result = RandomnessLib.emergencyFulfill(
            randomnessState,
            requestId,
            _historicalHash(),
            bytes32(0),
            entropyMerkleRoot,
            address(this),
            IERC20(randomnessState.tgbtTokenAddress),
            msg.sender
        );
        emit RandomnessFulfilled(requestId, result);
    }

    function getRandomRequestState(uint256 requestId)
        external
        view
        returns (address requester, uint256 timestamp, bool fulfilled, uint256 contributionsCount)
    {
        return RandomnessLib.getRequestState(randomnessState, requestId);
    }

    function getRandomnessConfig()
        external
        view
        returns (
            uint256 minContributions,
            uint256 maxContributions,
            uint256 expiryBlocks,
            uint256 baseEmergencyFee,
            uint256 feePerContributor,
            uint256 maxBatchSize
        )
    {
        return (
            randomnessState.minContributions,
            randomnessState.maxContributions,
            randomnessState.expiryBlocks,
            randomnessState.baseEmergencyFee,
            randomnessState.feePerContributor,
            randomnessState.maxBatchSize
        );
    }

    function getRandomnessReceipt(uint256 requestId)
        external
        view
        returns (
            address requester,
            uint256 requestedAt,
            bool fulfilled,
            bytes32 userSeed,
            bytes32 result,
            uint256 contributionsCount,
            uint256 minContributions,
            uint256 contributionsRemaining,
            uint256 maxContributions,
            uint256 emergencyFeeQuote
        )
    {
        // Read storage directly to avoid stack-too-deep from 9-value tuple destructuring
        RandomnessLib.RandomnessRequest storage req = randomnessState.requests[requestId];
        requester = req.requester;
        requestedAt = req.timestamp;
        fulfilled = req.fulfilled;
        userSeed = req.userSeed;
        result = req.result;
        contributionsCount = randomnessState.contributions[requestId].contributors.length;
        minContributions = randomnessState.minContributions;
        maxContributions = randomnessState.maxContributions;
        emergencyFeeQuote = randomnessState.baseEmergencyFee +
            (randomnessState.feePerContributor * contributionsCount);
        contributionsRemaining = contributionsCount >= minContributions
            ? 0
            : minContributions - contributionsCount;
    }

    function getRandomnessContributionDetails(uint256 requestId)
        external
        view
        returns (address[] memory contributors, bytes32[] memory contributions)
    {
        return RandomnessLib.getContributionDetails(randomnessState, requestId);
    }

    // Governance tuning functions removed — all params are set once at initialize(), immutable thereafter.
    // Token address, fee params, and contribution params are permanently locked at deployment.

    function _historicalHash() internal view returns (bytes32) {
        bytes32[32] memory history = _outputHistory();
        return keccak256(abi.encodePacked(history));
    }
}
