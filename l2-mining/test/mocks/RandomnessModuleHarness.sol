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
}