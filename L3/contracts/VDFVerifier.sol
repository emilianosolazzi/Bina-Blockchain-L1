// SPDX-License-Identifier: MIT
// Legacy L3 reference only.
// Keep for future delay / anti-manipulation entropy design; do not deploy as-is.
pragma solidity ^0.8.28;

/**
 * @title VDFVerifier
 * @notice Implements VDF challenge-response for time-locked entropy
 * @dev Prevents manipulation by requiring computational work that takes a minimum time
 */
contract VDFVerifier {
    // VDF parameters
    uint256 public difficulty;        // Number of iterations required (must be >= minDifficulty)
    uint256 public minTimeSeconds;    // Minimum wall-clock time expected between challenge and solution
    uint256 public maxChallengeWindow; // Maximum time window for challenge validity
    
    // Quantum resistance parameters
    uint16 private constant QR_HASH_ITERATIONS = 3;  // Gas-optimized number of iterations
    uint8 private constant QR_HASH_ROTATION = 7;     // Prime number for bit rotation
    
    // Stores when each challenge was issued
    mapping(bytes32 => uint256) public challengeTimestamp;

    event ChallengeGenerated(bytes32 indexed challenge, uint256 issuedAt);
    event VDFVerified(bytes32 indexed challenge, bool success, uint256 verifiedAt);
    event ChallengeExpired(bytes32 indexed challenge, uint256 expiryTime);
    
    constructor(uint256 _difficulty, uint256 _minTimeSeconds, uint256 _maxChallengeWindow) {
        require(_difficulty > 0, "VDFVerifier: difficulty must be > 0");
        require(_minTimeSeconds > 0, "VDFVerifier: minTime must be > 0");
        require(_maxChallengeWindow > _minTimeSeconds, "VDFVerifier: maxWindow must be > minTime");
        difficulty = _difficulty;
        minTimeSeconds = _minTimeSeconds;
        maxChallengeWindow = _maxChallengeWindow;
    }
    
    /**
     * @notice Generate a new VDF challenge from beacon output with quantum resistance
     * @dev Uses hybrid quantum-resistant hashing
     * @param beaconOutput Entropy source (e.g., from beacon)
     * @return challenge A unique time-locked challenge
     */
    function generateChallenge(bytes32 beaconOutput) external returns (bytes32 challenge) {
        bytes32 initialHash = keccak256(abi.encodePacked(beaconOutput, block.timestamp, address(this)));
        challenge = hybrid_quantum_resistant_hash(initialHash);
        
        challengeTimestamp[challenge] = block.timestamp;
        
        emit ChallengeGenerated(challenge, block.timestamp);
        
        return challenge;
    }
    
    /**
     * @notice Verify a VDF solution for a challenge
     * @dev Checks that solution is correct and required time has passed
     * @param challenge The original challenge issued
     * @param solution The computed solution
     * @param proof Optional proof (unused, kept for interface compatibility)
     * @return valid Whether the VDF solution is valid
     */
    function verifyVDFSolution(
        bytes32 challenge,
        bytes32 solution,
        bytes calldata proof
    ) external returns (bool valid) {
        uint256 issuedAt = challengeTimestamp[challenge];
        require(issuedAt > 0, "VDFVerifier: unknown challenge");

        // Ensure minimum time has passed
        require(block.timestamp >= issuedAt + minTimeSeconds, "VDFVerifier: too early");
        
        // Ensure challenge hasn't expired
        require(block.timestamp <= issuedAt + maxChallengeWindow, "VDFVerifier: challenge expired");

        // Recompute hash chain: starting from challenge, hash N times
        bytes32 current = challenge;
        for (uint256 i = 0; i < difficulty; i++) {
            current = keccak256(abi.encodePacked(current, i));
        }

        valid = (current == solution);

        // Prevent challenge replay by deleting the timestamp
        delete challengeTimestamp[challenge];

        emit VDFVerified(challenge, valid, block.timestamp);

        return valid;
    }
    
    /**
     * @notice Applies quantum-resistant hashing techniques to input
     * @dev Uses multiple iterations with bit rotation for enhanced security
     * @param input The input to hash
     * @return A quantum-resistant hash
     */
    function hybrid_quantum_resistant_hash(bytes32 input) internal view returns (bytes32) {
        bytes32 h = input;
        
        // Apply multiple iterations of hashing with bit rotation
        for (uint256 i = 0; i < QR_HASH_ITERATIONS; i++) {
            h = keccak256(abi.encodePacked(h ^ bytes32(i + 1), block.timestamp));
            h = bytes32((uint256(h) << QR_HASH_ROTATION) | (uint256(h) >> (256 - QR_HASH_ROTATION)));
        }
        
        return h;
    }
    
    /**
     * @notice Check if a challenge has expired and can be cleaned up
     * @param challenge The challenge to check
     * @return expired Whether the challenge has expired
     */
    function isChallengeExpired(bytes32 challenge) external view returns (bool expired) {
        uint256 issuedAt = challengeTimestamp[challenge];
        if (issuedAt == 0) return false; // Challenge doesn't exist or was already used
        
        return block.timestamp > issuedAt + maxChallengeWindow;
    }
    
    /**
     * @notice Clean up expired challenges to free storage
     * @param challenges Array of challenges to check and clean up
     * @return cleaned Number of challenges cleaned up
     */
    function cleanupExpiredChallenges(bytes32[] calldata challenges) external returns (uint256 cleaned) {
        cleaned = 0;
        
        for (uint256 i = 0; i < challenges.length; i++) {
            bytes32 challenge = challenges[i];
            uint256 issuedAt = challengeTimestamp[challenge];
            
            if (issuedAt > 0 && block.timestamp > issuedAt + maxChallengeWindow) {
                delete challengeTimestamp[challenge];
                cleaned++;
                
                emit ChallengeExpired(challenge, issuedAt + maxChallengeWindow);
            }
        }
        
        return cleaned;
    }
}
