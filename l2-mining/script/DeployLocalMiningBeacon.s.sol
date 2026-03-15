// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Script, console2 } from "forge-std/Script.sol";
import { ERC1967Proxy } from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import { TemporalGradientL2Beacon } from "../contracts/TemporalGradientL2Beacon.sol";
import { MockProtocolToken } from "../test/mocks/MockProtocolToken.sol";

contract DeployLocalMiningBeaconScript is Script {
    function run() external returns (address beaconAddress, address rewardTokenAddress, address stakeTokenAddress) {
        uint256 deployerPrivateKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        address minerAddress = vm.envAddress("MINER_ADDRESS");
        uint256 initialReward = vm.envOr("INITIAL_REWARD", uint256(10 ether));
        uint256 initialDifficulty = vm.envOr("INITIAL_DIFFICULTY", uint256(1000));
        uint256 blocksPerEpoch = vm.envOr("BLOCKS_PER_EPOCH", uint256(100));
        uint256 halvingInterval = vm.envOr("HALVING_INTERVAL", uint256(1000));

        vm.startBroadcast(deployerPrivateKey);

        MockProtocolToken rewardToken = new MockProtocolToken("Reward Token", "RWD");
        MockProtocolToken stakeToken = new MockProtocolToken("Stake Token", "STK");
        TemporalGradientL2Beacon implementation = new TemporalGradientL2Beacon();

        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(
                TemporalGradientL2Beacon.initialize,
                (address(rewardToken), address(stakeToken), initialReward, initialDifficulty, blocksPerEpoch, halvingInterval)
            )
        );

        TemporalGradientL2Beacon beacon = TemporalGradientL2Beacon(address(proxy));
        uint256 requiredStake = beacon.REQUIRED_TSTAKE_AMOUNT();
        stakeToken.mint(minerAddress, requiredStake);

        vm.stopBroadcast();

        console2.log("Local mining beacon ready");
        console2.log("Beacon proxy:", address(beacon));
        console2.log("Reward token:", address(rewardToken));
        console2.log("Stake token:", address(stakeToken));
        console2.log("Miner address funded with stake:", minerAddress);
        console2.log("Required stake:", requiredStake);
        console2.log("Pool 0 difficulty:", initialDifficulty);

        return (address(beacon), address(rewardToken), address(stakeToken));
    }
}