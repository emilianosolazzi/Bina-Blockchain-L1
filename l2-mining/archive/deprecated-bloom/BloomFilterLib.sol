// Archived on 2026-03-21 from l2-mining/contracts/BloomFilterLib.sol
// Reason: removed from the active L2 mining system to eliminate bloom-filter false positives
// and reduce on-chain complexity and gas overhead.

// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title BloomFilterLib
 * @notice A gas-efficient bloom filter library optimized for Arbitrum L2 with 10M+ user scalability
 * @dev Implements a bloom filter with configurable hash functions, salt, and bit manipulation for storage efficiency.
 *      Optimized for the EnhancedTemporalGradientBeacon to track used outputs at massive scale.
 *      False-positive rate is approximately (1 - e^(-k*n/m))^k, where:
 *      - k: number of hash functions (numHashes)
 *      - n: number of inserted elements
 *      - m: number of bits (size * 256, as each bucket is a bytes32 with 256 bits)
 *      For 10M+ users, recommended parameters:
 *      - size: 65536 (maximum) for ~16.7M bits capacity
 *      - numHashes: 4-5 (optimal for expected load)
 *      - Dynamic scaling for different network conditions
 * 
 *      With size = 65536, numHashes = 4, and 1M outputs:
 *      - FPR ≈ (1-e^(-4*1000000/16777216))^4 ≈ 0.2%
 *      - This is better than the target 0.5% FPR
 */
library BloomFilterLib {
    error InvalidFilterSize();
    error ZeroSize();
    error InvalidNumHashes();
    error IndexOutOfBounds();
    error NotPowerOfTwo();
    error ExceedsMaxSize();
    error InvalidAppeal();
    error AlreadyAppealed();
    error NotConsortiumMember();
    error InsufficientVotes();
    error AppealAlreadyResolved();
    error InvalidConsortiumAction();
    error AppealDoesNotExist();
    error InvalidMemberAddress();

    uint256 private constant HASH_SEED_1 = 0x734f6e50;
    uint256 private constant HASH_SEED_2 = 0x46724062;
    uint256 private constant HASH_SEED_3 = 0x34a2e4d9;
    uint256 private constant HASH_SEED_4 = 0xb76c9e13;
    uint256 private constant HASH_SEED_5 = 0x5a4e718c;

    uint256 private constant MAX_HASHES = 8;
    uint256 private constant MIN_SIZE = 128;
    uint256 private constant MAX_SIZE = 65536;

    struct Appeal {
        bytes32 output;
        address reporter;
        uint256 timestamp;
        bool resolved;
        bool accepted;
        uint256 voteCount;
        mapping(address => bool) voters;
    }

    struct Filter {
        bytes32[] buckets;
        uint256 size;
        uint256 numHashes;
        uint256 salt;
        uint256 insertCount;
        mapping(bytes32 => bool) appealedOutputs;
        mapping(bytes32 => Appeal) appeals;
        mapping(address => bool) consortiumMembers;
        uint256 minVotesRequired;
        uint256 totalConsortiumMembers;
    }

    event FilterUpdated(bytes32 indexed entry, uint256 indexed bucketIndex);
    event FilterCleared(uint256 size, uint256 numHashes);
    event FilterReset(uint256 newSize, uint256 newNumHashes, uint256 newSalt);
    event FilterScaled(uint256 oldSize, uint256 newSize, uint256 itemCount);
    event FalsePositiveDetected(bytes32 indexed output, address indexed reporter, bytes32 indexed appealId, bytes proof, uint256 timestamp);
    event AppealResolved(bytes32 indexed appealId, bool resolved, bool accepted);
    event ConsortiumMembershipChanged(address indexed member, bool isAdded, uint256 newTotalMembers);
    event VoteThresholdChanged(uint256 oldThreshold, uint256 newThreshold);
    event AppealVoteCast(bytes32 indexed appealId, address indexed voter, bool voteInFavor, uint256 newVoteCount);

    function createFilter(Filter storage filter, uint256 size, uint256 numHashes, uint256 salt) internal {
        resetFilter(filter, size, numHashes, salt);
        filter.insertCount = 0;
    }

    function updateFilter(Filter storage filter, bytes32 entry) internal {
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;
        uint256 salt = filter.salt;

        uint256[MAX_HASHES] memory hashSeeds;
        unchecked {
            for (uint256 i = 0; i < numHashes && i < MAX_HASHES; i++) {
                if (i == 0) hashSeeds[i] = HASH_SEED_1;
                else if (i == 1) hashSeeds[i] = HASH_SEED_2;
                else if (i == 2) hashSeeds[i] = HASH_SEED_3;
                else if (i == 3) hashSeeds[i] = HASH_SEED_4;
                else if (i == 4) hashSeeds[i] = HASH_SEED_5;
                else hashSeeds[i] = uint256(keccak256(abi.encodePacked(i, salt)));
            }
        }

        unchecked {
            for (uint256 i = 0; i < numHashes; i++) {
                uint256 hash = uint256(keccak256(abi.encodePacked(hashSeeds[i], entry, salt))) % (size * 256);
                uint256 bucketIndex = hash / 256;
                uint256 bitIndex = hash % 256;
                if (bucketIndex >= size) revert IndexOutOfBounds();
                bytes32 bucket = filter.buckets[bucketIndex];
                bucket = bucket | bytes32(1 << bitIndex);
                filter.buckets[bucketIndex] = bucket;
                emit FilterUpdated(entry, bucketIndex);
            }
        }

        filter.insertCount++;
    }

    function mightContain(Filter storage filter, bytes32 entry) internal view returns (bool exists) {
        uint256 size = filter.size;
        uint256 numHashes = filter.numHashes;
        uint256 salt = filter.salt;

        uint256[MAX_HASHES] memory hashSeeds;
        unchecked {
            for (uint256 i = 0; i < numHashes && i < MAX_HASHES; i++) {
                if (i == 0) hashSeeds[i] = HASH_SEED_1;
                else if (i == 1) hashSeeds[i] = HASH_SEED_2;
                else if (i == 2) hashSeeds[i] = HASH_SEED_3;
                else if (i == 3) hashSeeds[i] = HASH_SEED_4;
                else if (i == 4) hashSeeds[i] = HASH_SEED_5;
                else hashSeeds[i] = uint256(keccak256(abi.encodePacked(i, salt)));
            }
        }

        unchecked {
            for (uint256 i = 0; i < numHashes; i++) {
                uint256 hash = uint256(keccak256(abi.encodePacked(hashSeeds[i], entry, salt))) % (size * 256);
                uint256 bucketIndex = hash / 256;
                uint256 bitIndex = hash % 256;
                if (bucketIndex >= size) return false;
                bytes32 bucket = filter.buckets[bucketIndex];
                if ((bucket & bytes32(1 << bitIndex)) == 0) {
                    return false;
                }
            }
        }

        return true;
    }

    function clearFilter(Filter storage filter) internal {
        uint256 size = filter.size;
        assembly {
            mstore(0x0, 0)
            let bucketsSlot := keccak256(0x0, 0x20)
            for { let i := 0 } lt(i, size) { i := add(i, 1) } {
                sstore(add(bucketsSlot, i), 0)
            }
        }
        filter.insertCount = 0;
        emit FilterCleared(size, filter.numHashes);
    }

    function resetFilter(Filter storage filter, uint256 newSize, uint256 newNumHashes, uint256 newSalt) internal {
        validateFilterParams(newSize, newNumHashes);
        assembly {
            mstore(0x0, 0)
            let bucketsSlot := keccak256(0x0, 0x20)
            let length := sload(bucketsSlot)
            sstore(bucketsSlot, 0)
            for { let i := 0 } lt(i, length) { i := add(i, 1) } {
                sstore(add(bucketsSlot, add(i, 1)), 0)
            }
        }

        bytes32[] storage buckets = filter.buckets;
        assembly {
            mstore(0x0, 0)
            let bucketsSlot := keccak256(0x0, 0x20)
            sstore(bucketsSlot, newSize)
        }

        unchecked {
            for (uint256 i = 0; i < newSize; i++) {
                buckets.push(bytes32(0));
            }
        }

        filter.size = newSize;
        filter.numHashes = newNumHashes;
        filter.salt = newSalt;
        filter.insertCount = 0;

        emit FilterReset(newSize, newNumHashes, newSalt);
    }

    function scaleFilter(Filter storage filter, uint256 newSize, uint256 expectedItems, uint256 targetFPR) internal {
        validateFilterParams(newSize, 0);
        uint256 oldSize = filter.size;
        uint256 m = newSize * 256;
        uint256 n = expectedItems;
        uint256 optimalHashes = calculateOptimalHashCount(m, n, targetFPR);
        optimalHashes = optimalHashes > MAX_HASHES ? MAX_HASHES : optimalHashes;
        optimalHashes = optimalHashes == 0 ? 1 : optimalHashes;
        uint256 newSalt = uint256(keccak256(abi.encodePacked(block.timestamp, block.prevrandao, filter.salt)));
        resetFilter(filter, newSize, optimalHashes, newSalt);
        emit FilterScaled(oldSize, newSize, expectedItems);
    }

    function countSetBits(Filter storage filter) internal view returns (uint256 count) {
        uint256 size = filter.size;
        count = 0;
        unchecked {
            for (uint256 i = 0; i < size; i++) {
                bytes32 bucket = filter.buckets[i];
                uint256 value = uint256(bucket);
                while (value > 0) {
                    value &= value - 1;
                    count++;
                }
            }
        }
    }

    function estimateCurrentFPR(Filter storage filter) internal view returns (uint256 rateBps) {
        uint256 m = filter.size * 256;
        uint256 k = filter.numHashes;
        uint256 n = filter.insertCount;
        if (m == 0) return 10000;
        if (n == 0) return 0;
        uint256 scale = 10000;
        uint256 kn = k * n;
        uint256 fractionScaled;
        if (kn > type(uint256).max / scale) fractionScaled = (kn / m) * scale;
        else fractionScaled = (kn * scale) / m;
        if (fractionScaled > scale) return scale;
        uint256 complement = scale - fractionScaled;
        uint256 result = scale;
        unchecked {
            for (uint256 i = 0; i < k; i++) {
                if (result > type(uint256).max / complement) result = (result / scale) * complement;
                else result = (result * complement) / scale;
            }
        }
        return scale - result;
    }

    function estimateFPR(uint256 size, uint256 numHashes, uint256 itemCount) internal pure returns (uint256 rateBps) {
        uint256 m = size * 256;
        uint256 k = numHashes;
        uint256 n = itemCount;
        if (m == 0) return 10000;
        if (n == 0) return 0;
        uint256 scale = 10000;
        uint256 kn = k * n;
        uint256 fractionScaled;
        if (kn > type(uint256).max / scale) fractionScaled = (kn / m) * scale;
        else fractionScaled = (kn * scale) / m;
        if (fractionScaled > scale) return scale;
        uint256 complement = scale - fractionScaled;
        uint256 result = scale;
        unchecked {
            for (uint256 i = 0; i < k; i++) {
                if (result > type(uint256).max / complement) result = (result / scale) * complement;
                else result = (result * complement) / scale;
            }
        }
        return scale - result;
    }

    function calculateOptimalHashCount(uint256 m, uint256 n, uint256 targetFPRbps) internal pure returns (uint256) {
        if (m == 0 || n == 0) return 1;
        uint256 scale = 1000;
        uint256 lnInvP;
        if (targetFPRbps <= 10) lnInvP = 9210;
        else {
            if (targetFPRbps < 100) lnInvP = 6908;
            else if (targetFPRbps < 1000) lnInvP = 4605;
            else lnInvP = 2303;
        }
        uint256 mOverN;
        if (m > type(uint256).max / scale) mOverN = (m / n) * scale;
        else mOverN = (m * scale) / n;
        uint256 k = (lnInvP * mOverN) / (693 * scale);
        return k == 0 ? 1 : (k > MAX_HASHES ? MAX_HASHES : k);
    }

    function getFilterMetrics(Filter storage filter) internal view returns (
        uint256 size,
        uint256 bitCount,
        uint256 insertCount,
        uint256 fillRatioBps,
        uint256 estimatedFPRbps
    ) {
        size = filter.size;
        bitCount = size * 256;
        insertCount = filter.insertCount;
        uint256 setbits = countSetBits(filter);
        fillRatioBps = bitCount > 0 ? (setbits * 10000) / bitCount : 0;
        estimatedFPRbps = estimateCurrentFPR(filter);
        return (size, bitCount, insertCount, fillRatioBps, estimatedFPRbps);
    }

    function validateFilterParams(uint256 size, uint256 numHashes) private pure {
        if (size == 0) revert ZeroSize();
        if (size < MIN_SIZE) revert InvalidFilterSize();
        if (size > MAX_SIZE) revert ExceedsMaxSize();
        if ((size & (size - 1)) != 0) revert NotPowerOfTwo();
        if (numHashes > 0 && (numHashes == 0 || numHashes > MAX_HASHES)) revert InvalidNumHashes();
    }

    function validateFPR(Filter storage filter, uint256 expectedItems, uint256 targetFPRbps) internal view returns (bool valid, uint256 actualFPRbps) {
        actualFPRbps = estimateFPR(filter.size, filter.numHashes, expectedItems);
        return (actualFPRbps <= targetFPRbps, actualFPRbps);
    }

    function reportFalsePositive(Filter storage filter, bytes32 output, bytes memory proof) internal returns (bytes32 appealId) {
        require(mightContain(filter, output), "Not a false positive");
        if (filter.appealedOutputs[output]) revert AlreadyAppealed();
        appealId = keccak256(abi.encodePacked(output, msg.sender, block.number, block.timestamp));
        filter.appealedOutputs[output] = true;
        Appeal storage newAppeal = filter.appeals[appealId];
        newAppeal.output = output;
        newAppeal.reporter = msg.sender;
        newAppeal.timestamp = block.timestamp;
        newAppeal.resolved = false;
        newAppeal.accepted = false;
        newAppeal.voteCount = 0;
        emit FalsePositiveDetected(output, msg.sender, appealId, proof, block.timestamp);
        return appealId;
    }

    function voteOnAppeal(Filter storage filter, bytes32 appealId, bool acceptAppeal) internal returns (bool resolved, bool finalAccepted) {
        if (!filter.consortiumMembers[msg.sender]) revert NotConsortiumMember();
        Appeal storage appeal = filter.appeals[appealId];
        if (appeal.timestamp == 0) revert AppealDoesNotExist();
        if (appeal.resolved) revert AppealAlreadyResolved();
        if (appeal.voters[msg.sender]) revert InvalidConsortiumAction();
        appeal.voters[msg.sender] = true;
        appeal.voteCount++;
        emit AppealVoteCast(appealId, msg.sender, acceptAppeal, appeal.voteCount);
        if (appeal.voteCount >= filter.minVotesRequired) {
            appeal.resolved = true;
            appeal.accepted = acceptAppeal;
            emit AppealResolved(appealId, true, acceptAppeal);
            return (true, acceptAppeal);
        }
        return (false, false);
    }

    function hasAppeal(Filter storage filter, bytes32 output) internal view returns (bool) {
        return filter.appealedOutputs[output];
    }

    function getAppealStatus(Filter storage filter, bytes32 appealId) internal view returns (bool exists, bool resolved, bool accepted, uint256 voteCount) {
        Appeal storage appeal = filter.appeals[appealId];
        exists = appeal.timestamp > 0;
        if (!exists) return (false, false, false, 0);
        return (true, appeal.resolved, appeal.accepted, appeal.voteCount);
    }

    function initConsortium(Filter storage filter, address[] memory initialMembers, uint256 minVotes) internal {
        if (initialMembers.length == 0) revert InvalidConsortiumAction();
        if (minVotes == 0) revert InsufficientVotes();
        filter.minVotesRequired = minVotes;
        filter.totalConsortiumMembers = 0;
        for (uint256 i = 0; i < initialMembers.length; i++) {
            if (!filter.consortiumMembers[initialMembers[i]] && initialMembers[i] != address(0)) {
                filter.consortiumMembers[initialMembers[i]] = true;
                filter.totalConsortiumMembers++;
                emit ConsortiumMembershipChanged(initialMembers[i], true, filter.totalConsortiumMembers);
            }
        }
        if (minVotes > filter.totalConsortiumMembers) {
            filter.minVotesRequired = filter.totalConsortiumMembers;
            emit VoteThresholdChanged(minVotes, filter.minVotesRequired);
        }
    }

    function updateConsortiumMembership(Filter storage filter, address member, bool isAdding) internal {
        if (member == address(0)) revert InvalidMemberAddress();
        if (isAdding && !filter.consortiumMembers[member]) {
            filter.consortiumMembers[member] = true;
            filter.totalConsortiumMembers++;
            emit ConsortiumMembershipChanged(member, true, filter.totalConsortiumMembers);
        } else if (!isAdding && filter.consortiumMembers[member]) {
            filter.consortiumMembers[member] = false;
            filter.totalConsortiumMembers--;
            emit ConsortiumMembershipChanged(member, false, filter.totalConsortiumMembers);
            if (filter.minVotesRequired > filter.totalConsortiumMembers && filter.totalConsortiumMembers > 0) {
                uint256 oldThreshold = filter.minVotesRequired;
                filter.minVotesRequired = filter.totalConsortiumMembers;
                emit VoteThresholdChanged(oldThreshold, filter.minVotesRequired);
            }
        }
    }

    function generatePruningReport(Filter storage filter, uint256 targetUsage) internal view returns (uint256 pruneSizeBps, uint256 estimatedGasSavings, uint256 recommendedNewSize, uint256 recommendedNewHashes) {
        (uint256 size, , uint256 insertCount, uint256 fillRatioBps, uint256 fprBps) = getFilterMetrics(filter);
        if (fillRatioBps > targetUsage) {
            pruneSizeBps = fillRatioBps - targetUsage;
        }
        uint256 slotsToBeCleared = (size * pruneSizeBps) / 10000;
        estimatedGasSavings = slotsToBeCleared * 15000;
        recommendedNewSize = size;
        while (recommendedNewSize > MIN_SIZE && fillRatioBps < targetUsage / 2) {
            recommendedNewSize = recommendedNewSize / 2;
            fillRatioBps = fillRatioBps * 2;
        }
        uint256 remaining_items = insertCount * (10000 - pruneSizeBps) / 10000;
        recommendedNewHashes = calculateOptimalHashCount(recommendedNewSize * 256, remaining_items, fprBps);
        return (pruneSizeBps, estimatedGasSavings, recommendedNewSize, recommendedNewHashes);
    }
}
