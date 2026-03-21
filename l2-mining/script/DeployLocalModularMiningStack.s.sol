// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Script, console2 } from "forge-std/Script.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { MiningModule } from "../contracts/modules/MiningModule.sol";
import { RandomnessModule } from "../contracts/modules/RandomnessModule.sol";
import { TokenomicsModule } from "../contracts/modules/TokenomicsModule.sol";
import { RateLimitModule } from "../contracts/modules/RateLimitModule.sol";
import { MockProtocolToken } from "../test/mocks/MockProtocolToken.sol";

contract DeployLocalModularMiningStackScript is Script {
    function run()
        external
        returns (
            address coreAddress,
            address miningModuleAddress,
            address randomnessModuleAddress,
            address tokenomicsModuleAddress,
            address rateLimitModuleAddress,
            address rewardTokenAddress,
            address holdTokenAddress
        )
    {
        uint256 deployerPrivateKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        address admin = vm.envAddress("ADMIN_ADDRESS");
        address minerAddress = vm.envAddress("MINER_ADDRESS");
        uint256 initialReward = vm.envOr("INITIAL_REWARD", uint256(10 ether));
        uint256 initialDifficulty = vm.envOr("INITIAL_DIFFICULTY", uint256(1000));
        uint256 blocksPerEpoch = vm.envOr("BLOCKS_PER_EPOCH", uint256(100));
        uint256 halvingInterval = vm.envOr("HALVING_INTERVAL", uint256(1000));
        uint256 initialEmission = vm.envOr("INITIAL_EMISSION", uint256(700_000_000 ether));

        vm.startBroadcast(deployerPrivateKey);

        MockProtocolToken rewardToken = new MockProtocolToken("Reward Token", "RWD");
    MockProtocolToken holdToken = new MockProtocolToken("TGBT Token", "TGBT");

        TemporalGradientCore core = new TemporalGradientCore(admin, bytes32(0));

        MiningModule miningModule = new MiningModule();
        miningModule.initialize(address(core), address(holdToken), initialDifficulty, initialEmission);

        RandomnessModule randomnessModule = new RandomnessModule();
        randomnessModule.initialize(address(core), address(rewardToken));

        TokenomicsModule tokenomicsModule = new TokenomicsModule();
        tokenomicsModule.initialize(
            address(core),
            address(rewardToken),
            initialReward,
            blocksPerEpoch,
            halvingInterval,
            0,
            0
        );

        RateLimitModule rateLimitModule = new RateLimitModule();
        rateLimitModule.initialize(address(core));

        core.setModule(keccak256("MINING_MODULE"), address(miningModule));
        core.setModule(keccak256("RANDOMNESS_MODULE"), address(randomnessModule));
        core.setModule(keccak256("TOKENOMICS_MODULE"), address(tokenomicsModule));
        core.setModule(keccak256("RATE_LIMIT_MODULE"), address(rateLimitModule));

        holdToken.mint(minerAddress, miningModule.REQUIRED_TGBT_HOLD_AMOUNT());

        vm.stopBroadcast();

        console2.log("Local modular mining stack ready");
        console2.log("Core:", address(core));
        console2.log("Mining module:", address(miningModule));
        console2.log("Randomness module:", address(randomnessModule));
        console2.log("Tokenomics module:", address(tokenomicsModule));
        console2.log("Rate limit module:", address(rateLimitModule));
        console2.log("Reward token:", address(rewardToken));
        console2.log("Anti-sybil hold token:", address(holdToken));
        console2.log("Miner address funded with TGBT hold balance:", minerAddress);

        return (
            address(core),
            address(miningModule),
            address(randomnessModule),
            address(tokenomicsModule),
            address(rateLimitModule),
            address(rewardToken),
            address(holdToken)
        );
    }
}