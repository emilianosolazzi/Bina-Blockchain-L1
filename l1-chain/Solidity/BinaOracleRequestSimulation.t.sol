// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

import "./BinaOracle.sol";

contract DemoRandomnessConsumer {
    BinaOracle public immutable oracle;
    bytes32 public lastWord;

    constructor(BinaOracle oracle_) {
        oracle = oracle_;
    }

    function request(bytes32 purpose, bytes32 salt, uint64 minHeight) external returns (uint256) {
        return oracle.requestUtility(purpose, salt, minHeight);
    }

    function fulfill(uint256 requestId) external returns (bytes32) {
        lastWord = oracle.fulfillUtility(requestId);
        return lastWord;
    }

    function randomUint(bytes32 purpose, bytes32 salt, uint256 upperBound) external view returns (uint256) {
        return oracle.randomUint(purpose, salt, upperBound);
    }
}

contract BinaOracleRequestSimulation {
    event SimulatedRandomnessRequest(
        address indexed consumer,
        uint256 indexed requestId,
        bytes32 purpose,
        bytes32 salt,
        bytes32 utilityWord,
        uint256 boundedRandom
    );

    function testConsumerRequestsAndFulfillsRandomness() public {
        BinaOracle oracle = new BinaOracle(address(this));
        DemoRandomnessConsumer consumer = new DemoRandomnessConsumer(oracle);

        bytes32 purpose = keccak256("BINA_GAMING");
        bytes32 salt = keccak256("demo-consumer-round-1");
        bytes32 seed = bytes32(uint256(0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcd));
        bytes32 blockHash = bytes32(uint256(0xabc001));
        uint64 minHeight = 123;

        uint256 requestId = consumer.request(purpose, salt, minHeight);
        require(requestId == 1, "first request id");

        try consumer.fulfill(requestId) returns (bytes32) {
            revert("request should not be ready before relay");
        } catch {}

        BinaOracle.BinaOutput memory output = BinaOracle.BinaOutput({
            height: minHeight,
            blockHash: blockHash,
            randomnessOutput: seed,
            nullifier: bytes32(uint256(0xfeed01)),
            binaMiner: bytes20(hex"3054ac8bc5c9b358e270e17183851201d0bc6b69"),
            btcHeight: 957090,
            btcSeed: bytes32(uint256(0xbeef01)),
            minedTimestamp: uint64(block.timestamp),
            workBits: 25,
            claimDigest: bytes32(uint256(0xc1a1)),
            electionScore: bytes32(uint256(0xe1ec)),
            falconVerified: true
        });

        oracle.submitOutput(output, purpose, bytes("mock finalized BINA proof bundle"));

        bytes32 utilityWord = consumer.fulfill(requestId);
        bytes32 expected = keccak256(
            abi.encodePacked(
                "BINA_EVM_UTILITY_V1",
                block.chainid,
                address(oracle),
                seed,
                purpose,
                salt,
                address(consumer),
                requestId
            )
        );
        require(utilityWord == expected, "derived word mismatch");

        uint256 boundedRandom = consumer.randomUint(purpose, salt, 1000);
        require(boundedRandom < 1000, "bounded random out of range");
        uint256 explicitConsumerRandom = oracle.randomUintFor(purpose, salt, address(consumer), 1000);
        require(explicitConsumerRandom == boundedRandom, "explicit consumer random mismatch");

        emit SimulatedRandomnessRequest(
            address(consumer),
            requestId,
            purpose,
            salt,
            utilityWord,
            boundedRandom
        );
    }
}