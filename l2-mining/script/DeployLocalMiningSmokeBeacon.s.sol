// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Script, console2 } from "forge-std/Script.sol";
import { LocalMiningSmokeBeacon } from "../contracts/mocks/LocalMiningSmokeBeacon.sol";

contract DeployLocalMiningSmokeBeaconScript is Script {
    function run() external returns (address beaconAddress) {
        uint256 deployerPrivateKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        uint256 difficulty = vm.envOr("INITIAL_DIFFICULTY", uint256(1000));
        uint256 rewardAmount = vm.envOr("INITIAL_REWARD", uint256(10 ether));
        uint256 commitmentAge = vm.envOr("MIN_COMMITMENT_AGE", uint256(2));

        vm.startBroadcast(deployerPrivateKey);
        LocalMiningSmokeBeacon beacon = new LocalMiningSmokeBeacon(difficulty, rewardAmount, commitmentAge);
        vm.stopBroadcast();

        console2.log("Local mining smoke beacon ready");
        console2.log("Beacon:", address(beacon));
        console2.log("Difficulty:", difficulty);
        console2.log("Reward:", rewardAmount);
        console2.log("Min commitment age:", commitmentAge);

        return address(beacon);
    }
}