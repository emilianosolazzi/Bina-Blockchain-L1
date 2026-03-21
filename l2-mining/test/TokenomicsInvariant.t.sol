// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { StdInvariant } from "forge-std/StdInvariant.sol";
import { ERC1967Proxy } from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { TokenomicsModule } from "../contracts/modules/TokenomicsModule.sol";
import { TGBT } from "../contracts/TGBT_Token.sol";
import { TokenomicsHandler } from "./invariant/TokenomicsHandler.sol";

contract TokenomicsInvariantTest is StdInvariant, Test {
    bytes32 internal constant MINING_MODULE = keccak256("MINING_MODULE");
    bytes32 internal constant STALE_BLOCK_MODULE = keccak256("STALE_BLOCK_MODULE");

    TemporalGradientCore internal core;
    TokenomicsModule internal tokenomics;
    TGBT internal token;
    TokenomicsHandler internal handler;

    function setUp() public {
        core = new TemporalGradientCore(address(this), bytes32(uint256(1)));
        token = new TGBT(address(this));

        TokenomicsModule implementation = new TokenomicsModule();
        ERC1967Proxy proxy = new ERC1967Proxy(
            address(implementation),
            abi.encodeCall(TokenomicsModule.initialize, (address(core), address(token), 10 ether, 1_000, 10_000, 2, 125))
        );
        tokenomics = TokenomicsModule(address(proxy));
        handler = new TokenomicsHandler(tokenomics);

        core.setModule(MINING_MODULE, address(handler));
        core.setModule(STALE_BLOCK_MODULE, address(handler));
        token.grantAuthorization(address(tokenomics));

        targetContract(address(handler));
    }

    function invariant_supplyNeverExceedsCap() public view {
        assertLe(token.totalSupply(), token.MAX_SUPPLY());
    }

    function invariant_miningNeverExceedsAllocation() public view {
        assertLe(tokenomics.totalMined(), tokenomics.MINING_ALLOCATION());
    }

    function invariant_staleNeverExceedsAllocation() public view {
        assertLe(tokenomics.totalStaleRewards(), tokenomics.STALE_BLOCK_ALLOCATION());
    }

    function invariant_tokenSupplyMatchesTrackedIssuance() public view {
        assertEq(token.totalSupply(), tokenomics.totalMined() + tokenomics.totalStaleRewards());
    }
}