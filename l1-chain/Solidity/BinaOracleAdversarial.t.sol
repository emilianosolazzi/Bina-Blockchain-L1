// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

import "./BinaOracle.sol";

/// @dev Minimal cheatcode interface (no forge-std dependency) — only used by
///      the single test that needs to move past a future-commitment deadline.
interface IHEVM {
    function warp(uint256 newTimestamp) external;
}

/// @dev Stand-in for a distinct publisher identity: routes calls through a
///      separate contract address so tests can exercise multi-publisher
///      quorum and bonding without needing `vm.prank`.
contract PublisherAgent {
    receive() external payable {}

    function submitOutput(
        BinaOracle oracle,
        BinaOracle.BinaOutput calldata output,
        bytes32 purpose,
        bytes calldata proofBundle,
        string calldata proofURI
    ) external {
        oracle.submitOutput(output, purpose, proofBundle, proofURI);
    }

    function submitOutputForPurposes(
        BinaOracle oracle,
        BinaOracle.BinaOutput calldata output,
        bytes32[] calldata purposes,
        bytes calldata proofBundle,
        string calldata proofURI
    ) external {
        oracle.submitOutputForPurposes(output, purposes, proofBundle, proofURI);
    }

    function depositBond(BinaOracle oracle) external payable {
        oracle.depositBond{value: msg.value}();
    }

    function withdrawBond(BinaOracle oracle, uint256 amount) external {
        oracle.withdrawBond(amount);
    }

    function resolveFutureCommitment(BinaOracle oracle, uint256 commitmentId) external {
        oracle.resolveFutureCommitment(commitmentId);
    }
}

/// @dev Adversarial / failure-path coverage for BinaOracle.sol, mirroring
///      the adversarial test suite already in place on the Rust L1 node
///      (unauthorized actors, stale/conflicting state, double-spend-style
///      reuse) plus the new quorum and bonding/slashing mechanics.
contract BinaOracleAdversarialTests {
    receive() external payable {}

    bytes32 constant PURPOSE = keccak256("BINA_TEST_PURPOSE");

    function _sampleOutput(
        uint64 height,
        bytes32 blockHash,
        bytes32 randomness,
        bytes32 nullifier
    ) internal view returns (BinaOracle.BinaOutput memory) {
        return BinaOracle.BinaOutput({
            height: height,
            blockHash: blockHash,
            randomnessOutput: randomness,
            nullifier: nullifier,
            binaMiner: bytes20(hex"3054ac8bc5c9b358e270e17183851201d0bc6b69"),
            btcHeight: 957090,
            btcSeed: bytes32(uint256(0xbeef01)),
            minedTimestamp: uint64(block.timestamp),
            workBits: 25,
            claimDigest: bytes32(uint256(0xc1a1)),
            electionScore: bytes32(uint256(0xe1ec)),
            falconVerified: true
        });
    }

    // ---- baseline ingestion guards ----

    function testUnauthorizedPublisherCannotSubmit() public {
        BinaOracle oracle = new BinaOracle(address(0));
        PublisherAgent stranger = new PublisherAgent();
        BinaOracle.BinaOutput memory output =
            _sampleOutput(10, bytes32(uint256(1)), bytes32(uint256(2)), bytes32(uint256(3)));
        try stranger.submitOutput(oracle, output, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: unauthorized publisher");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.NotPublisher.selector, "wrong revert reason");
        }
    }

    function testDuplicateSubmissionRejected() public {
        BinaOracle oracle = new BinaOracle(address(0));
        bytes32 blockHash = bytes32(uint256(100));
        BinaOracle.BinaOutput memory output = _sampleOutput(10, blockHash, bytes32(uint256(2)), bytes32(uint256(3)));
        oracle.submitOutput(output, PURPOSE, bytes("proof"), "");
        try oracle.submitOutput(output, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: duplicate submission");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.AlreadySubmitted.selector, "wrong revert reason");
        }
    }

    function testNullifierReuseRejected() public {
        BinaOracle oracle = new BinaOracle(address(0));
        bytes32 nullifier = bytes32(uint256(999));
        BinaOracle.BinaOutput memory outputA =
            _sampleOutput(10, bytes32(uint256(101)), bytes32(uint256(2)), nullifier);
        oracle.submitOutput(outputA, PURPOSE, bytes("proof"), "");

        BinaOracle.BinaOutput memory outputB =
            _sampleOutput(11, bytes32(uint256(102)), bytes32(uint256(3)), nullifier);
        try oracle.submitOutput(outputB, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: nullifier reuse");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.NullifierAlreadyUsed.selector, "wrong revert reason");
        }
    }

    function testFalconRequiredButMissingRejected() public {
        BinaOracle oracle = new BinaOracle(address(0));
        oracle.setRequireFalcon(true);
        BinaOracle.BinaOutput memory output =
            _sampleOutput(10, bytes32(uint256(103)), bytes32(uint256(2)), bytes32(uint256(3)));
        output.falconVerified = false;
        try oracle.submitOutput(output, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: falcon not verified");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.FalconNotVerified.selector, "wrong revert reason");
        }
    }

    function testTimestampDriftRejected() public {
        BinaOracle oracle = new BinaOracle(address(0));
        BinaOracle.BinaOutput memory output =
            _sampleOutput(10, bytes32(uint256(104)), bytes32(uint256(2)), bytes32(uint256(3)));
        output.minedTimestamp = uint64(block.timestamp + 2 hours);
        try oracle.submitOutput(output, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: timestamp drift");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.InvalidTimestamp.selector, "wrong revert reason");
        }
    }

    function testTimestampTooOldRejected() public {
        // Forge's genesis timestamp is ~1, so warp forward first to make an
        // "8 days ago" mined timestamp actually older than MAX_TIMESTAMP_AGE.
        IHEVM(address(uint160(uint256(keccak256("hevm cheat code"))))).warp(30 days);
        BinaOracle oracle = new BinaOracle(address(0));
        BinaOracle.BinaOutput memory output =
            _sampleOutput(10, bytes32(uint256(105)), bytes32(uint256(2)), bytes32(uint256(3)));
        output.minedTimestamp = uint64(block.timestamp - 8 days);
        try oracle.submitOutput(output, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: timestamp too old");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.InvalidTimestamp.selector, "wrong revert reason");
        }
    }

    // ---- #4: monotonic per-purpose height ----

    function testStaleHeightRejected() public {
        BinaOracle oracle = new BinaOracle(address(0));
        BinaOracle.BinaOutput memory outputHigh =
            _sampleOutput(100, bytes32(uint256(106)), bytes32(uint256(2)), bytes32(uint256(3)));
        oracle.submitOutput(outputHigh, PURPOSE, bytes("proof"), "");

        BinaOracle.BinaOutput memory outputLow =
            _sampleOutput(50, bytes32(uint256(107)), bytes32(uint256(4)), bytes32(uint256(5)));
        try oracle.submitOutput(outputLow, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: stale height");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.StaleHeight.selector, "wrong revert reason");
        }
    }

    // ---- #2: multi-publisher quorum ----

    function testQuorumRequiresMatchingFingerprint() public {
        BinaOracle oracle = new BinaOracle(address(0));
        oracle.setQuorumThreshold(2);
        PublisherAgent second = new PublisherAgent();
        oracle.setPublisher(address(second), true);

        bytes32 blockHash = bytes32(uint256(108));
        BinaOracle.BinaOutput memory outputA = _sampleOutput(10, blockHash, bytes32(uint256(2)), bytes32(uint256(3)));
        oracle.submitOutput(outputA, PURPOSE, bytes("proof"), "");

        BinaOracle.BinaOutput memory outputB = outputA;
        outputB.randomnessOutput = bytes32(uint256(999));
        try second.submitOutput(oracle, outputB, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: attestation mismatch");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.AttestationMismatch.selector, "wrong revert reason");
        }
    }

    function testQuorumFinalizesOnceThresholdReached() public {
        BinaOracle oracle = new BinaOracle(address(0));
        oracle.setQuorumThreshold(2);
        PublisherAgent second = new PublisherAgent();
        oracle.setPublisher(address(second), true);

        bytes32 blockHash = bytes32(uint256(109));
        BinaOracle.BinaOutput memory output = _sampleOutput(10, blockHash, bytes32(uint256(2)), bytes32(uint256(3)));

        oracle.submitOutput(output, PURPOSE, bytes("proof"), "");
        try oracle.getLatestSeed(PURPOSE) returns (bytes32, uint64, uint64, bytes32) {
            revert("should not be ready after only 1 of 2 attestations");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.PurposeNotReady.selector, "wrong revert reason");
        }

        second.submitOutput(oracle, output, PURPOSE, bytes("proof"), "");
        (bytes32 seed,,,) = oracle.getLatestSeed(PURPOSE);
        require(seed == output.randomnessOutput, "seed should finalize once quorum is reached");
    }

    function testAlreadyAttestedRejected() public {
        BinaOracle oracle = new BinaOracle(address(0));
        oracle.setQuorumThreshold(2);
        bytes32 blockHash = bytes32(uint256(110));
        BinaOracle.BinaOutput memory output = _sampleOutput(10, blockHash, bytes32(uint256(2)), bytes32(uint256(3)));

        oracle.submitOutput(output, PURPOSE, bytes("proof"), "");
        try oracle.submitOutput(output, PURPOSE, bytes("proof"), "") {
            revert("should have reverted: already attested by same publisher");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.AlreadyAttested.selector, "wrong revert reason");
        }
    }

    // ---- #1 / #3: publisher bonding and slashing ----

    function testBondingSlashesAssignedPublisherAfterMissedDeadline() public {
        IHEVM vm = IHEVM(address(uint160(uint256(keccak256("hevm cheat code")))));
        BinaOracle oracle = new BinaOracle(address(0));
        PublisherAgent agent = new PublisherAgent();
        oracle.setPublisher(address(agent), true);
        agent.depositBond{value: 1 ether}(oracle);
        require(oracle.activePublisherCount() == 1, "agent should be active after bonding");

        uint64 targetHeight = 500;
        uint64 deadline = uint64(block.timestamp + 100);
        uint256 commitmentId = oracle.requestFuturePublication(PURPOSE, targetHeight, deadline);

        vm.warp(block.timestamp + 200);

        uint256 balanceBefore = address(this).balance;
        oracle.resolveFutureCommitment(commitmentId);
        uint256 balanceAfter = address(this).balance;

        require(balanceAfter - balanceBefore == 0.5 ether, "resolver should receive the slashed bond");
        (, , , , bool resolved) = oracle.futureCommitments(commitmentId);
        require(resolved, "commitment should be marked resolved");
        require(oracle.publisherBonds(address(agent)) == 0.5 ether, "assigned publisher's bond should be reduced");
    }

    function testFutureCommitmentResolvesWithoutSlashIfPublished() public {
        BinaOracle oracle = new BinaOracle(address(0));
        PublisherAgent agent = new PublisherAgent();
        oracle.setPublisher(address(agent), true);
        agent.depositBond{value: 1 ether}(oracle);

        uint64 targetHeight = 42;
        uint64 deadline = uint64(block.timestamp + 1000);
        uint256 commitmentId = oracle.requestFuturePublication(PURPOSE, targetHeight, deadline);

        BinaOracle.BinaOutput memory output =
            _sampleOutput(targetHeight, bytes32(uint256(111)), bytes32(uint256(2)), bytes32(uint256(3)));
        oracle.submitOutput(output, PURPOSE, bytes("proof"), "");

        uint256 balanceBefore = address(this).balance;
        oracle.resolveFutureCommitment(commitmentId);
        uint256 balanceAfter = address(this).balance;
        require(balanceAfter == balanceBefore, "no slash payout expected when published on time");
        require(oracle.publisherBonds(address(agent)) == 1 ether, "bond should be untouched, only unlocked");

        (, , , , bool resolved) = oracle.futureCommitments(commitmentId);
        require(resolved, "commitment should resolve");
    }

    function testInsufficientFreeBondRejectsNewCommitment() public {
        BinaOracle oracle = new BinaOracle(address(0));
        PublisherAgent agent = new PublisherAgent();
        oracle.setPublisher(address(agent), true);
        agent.depositBond{value: 1 ether}(oracle);

        oracle.requestFuturePublication(PURPOSE, 1000, uint64(block.timestamp + 100));
        oracle.requestFuturePublication(PURPOSE, 1001, uint64(block.timestamp + 100));

        try oracle.requestFuturePublication(PURPOSE, 1002, uint64(block.timestamp + 100)) {
            revert("should have reverted: insufficient free bond");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.InsufficientFreeBond.selector, "wrong revert reason");
        }
    }

    function testWithdrawBondRespectsLockedAmount() public {
        BinaOracle oracle = new BinaOracle(address(0));
        PublisherAgent agent = new PublisherAgent();
        oracle.setPublisher(address(agent), true);
        agent.depositBond{value: 1 ether}(oracle);
        oracle.requestFuturePublication(PURPOSE, 2000, uint64(block.timestamp + 100));

        try agent.withdrawBond(oracle, 0.6 ether) {
            revert("should have reverted: withdrawing more than free bond");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.InsufficientBond.selector, "wrong revert reason");
        }

        agent.withdrawBond(oracle, 0.5 ether);
    }

    function testNoActivePublishersBlocksFutureRequest() public {
        BinaOracle oracle = new BinaOracle(address(0));
        try oracle.requestFuturePublication(PURPOSE, 10, uint64(block.timestamp + 100)) {
            revert("should have reverted: no bonded publishers");
        } catch (bytes memory reason) {
            require(bytes4(reason) == BinaOracle.NoActivePublishers.selector, "wrong revert reason");
        }
    }
}
