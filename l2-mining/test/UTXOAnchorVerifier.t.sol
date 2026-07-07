// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Test } from "forge-std/Test.sol";
import { TemporalGradientCore } from "../contracts/TemporalGradientCore.sol";
import { UTXOAnchorVerifier } from "../contracts/UTXOAnchorVerifier.sol";

contract UTXOAnchorVerifierTest is Test {
    TemporalGradientCore internal core;
    UTXOAnchorVerifier internal verifier;

    address internal admin = address(0xA11CE);
    address internal attestor = address(0xA77057);

    string internal constant UTXO_ID = "d687778bc83a2bea4963eda038edf9dad7a97a3d8997e72a67697ff8b9f32fe0:1";
    string internal constant DATA_HASH = "7d94eacc5e4278b88b327321adafed10ee0d02db7c9f2b63224a5e5eefc33bbd";
    string internal constant MERKLE_ROOT = "7d94eacc5e4278b88b327321adafed10ee0d02db7c9f2b63224a5e5eefc33bbd";
    bytes32 internal constant EXPECTED_ANCHOR_ID = 0x4d7962f72f02e739cd185b4ec0d7b9017c97905269aa2490313c362c5e0b6116;
    bytes32 internal constant METADATA_DIGEST = 0x7a8f8fc2a92f6b3fd53a8b9baba026dc47175a5c408779752ada868343ddf6e6;
    uint64 internal constant CREATED_AT = 1782438645;

    function setUp() public {
        vm.startPrank(admin);
        core = new TemporalGradientCore(admin, bytes32(0));
        verifier = new UTXOAnchorVerifier();
        verifier.initialize(address(core), attestor);
        vm.stopPrank();
    }

    function testComputesScannerCompatibleAnchorId() public view {
        bytes32 anchorId = verifier.computeAnchorId(
            UTXO_ID,
            DATA_HASH,
            MERKLE_ROOT,
            "",
            CREATED_AT
        );

        assertEq(anchorId, EXPECTED_ANCHOR_ID);
    }

    function testComputesSameAnchorIdWithPrefixedHex() public view {
        bytes32 anchorId = verifier.computeAnchorId(
            UTXO_ID,
            string.concat("0x", DATA_HASH),
            string.concat("0x", MERKLE_ROOT),
            "",
            CREATED_AT
        );

        assertEq(anchorId, EXPECTED_ANCHOR_ID);
    }

    function testRegisterAndVerifyAnchor() public {
        vm.prank(attestor);
        bytes32 anchorId = verifier.registerAnchor(
            UTXO_ID,
            DATA_HASH,
            MERKLE_ROOT,
            "",
            METADATA_DIGEST,
            CREATED_AT,
            attestor
        );

        assertEq(anchorId, EXPECTED_ANCHOR_ID);

        (bool valid, bytes32 computedAnchorId) = verifier.verifyAnchor(
            UTXO_ID,
            DATA_HASH,
            MERKLE_ROOT,
            "",
            METADATA_DIGEST,
            CREATED_AT,
            attestor
        );

        assertTrue(valid);
        assertEq(computedAnchorId, EXPECTED_ANCHOR_ID);
    }
}