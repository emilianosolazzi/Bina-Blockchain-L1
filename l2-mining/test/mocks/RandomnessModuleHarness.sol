// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { RandomnessLib } from "../../contracts/RandomnessLib.sol";
import { RandomnessModule } from "../../contracts/modules/RandomnessModule.sol";

contract RandomnessModuleHarness is RandomnessModule {
    function contributeEntropyNoAutoFulfill(uint256 requestId, bytes32 entropyContribution) external whenSystemActive {
        bool shouldFulfill = RandomnessLib.addContribution(randomnessState, requestId, msg.sender, entropyContribution);
        shouldFulfill;

        (, , , uint256 contributionCount) = RandomnessLib.getRequestState(randomnessState, requestId);

        emit RandomnessContributionAdded(
            requestId,
            msg.sender,
            entropyContribution,
            contributionCount,
            randomnessState.minContributions
        );
    }

    function previewEmergencyFulfillResult(uint256 requestId) external view returns (bytes32) {
        (
            address requester,
            uint256 requestedAt,
            bool fulfilled,
            bytes32 userSeed,
            ,
            uint256 contributionsCount,
            ,
            ,
            
        ) = RandomnessLib.getRequestReceipt(randomnessState, requestId);

        require(requester != address(0), "missing request");
        require(!fulfilled, "already fulfilled");

        (, bytes32[] memory contributions) = RandomnessLib.getContributionDetails(randomnessState, requestId);

        bytes memory packed = abi.encodePacked(
            userSeed,
            _historicalHash(),
            bytes32(0),
            bytes32(block.prevrandao),
            block.number,
            block.timestamp,
            contributionsCount
        );

        for (uint256 i = 0; i < contributions.length; i++) {
            packed = abi.encodePacked(packed, contributions[i]);
        }

        requestedAt;
        return keccak256(packed);
    }
}