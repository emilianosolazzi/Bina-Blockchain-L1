// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

interface IRateLimitModule {
    function consumeOrRevert(address user, uint256 cost, bytes32 operation) external;
    function getUserCapacity(address user) external view returns (uint256 currentTokens, uint256 capacity);
}
