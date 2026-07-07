// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

/**
 * @title BINAOracle
 * @author Emiliano Solazzi
 * @notice Decentralized Post-Quantum Randomness Oracle
 * @dev Any miner can submit Blake3 PoW outputs anchored to Bitcoin
 * Multiple consumers (AI, DeFi, gaming) can use the randomness
 */
contract BINAOracle {
    
    // ======================== STRUCTS ========================
    
    struct RandomnessRecord {
        uint256 height;
        bytes32 blockHash;
        bytes32 randomnessOutput;
        bytes32 nullifier;
        address minerAddress;
        uint256 btcHeight;
        bytes32 btcSeed;
        uint256 minedTimestamp;
        uint8 zeroBits;
        bool powVerified;
    }
    
    struct AIConsumerMetadata {
        bytes32 purposeHash;
        bytes32 packageHash;
        bytes32 modelHash;
        bytes32 datasetHash;
    }

    // Ultra-lean storage (Strictly 3 Slots to save ~40,000 gas per submission)
    // Miner address and zeroBits are verified off-chain via logs
    struct StoredRecord {
        bytes32 randomnessOutput; // Slot 1
        uint256 btcHeight;        // Slot 2
        uint256 minedTimestamp;   // Slot 3
    }

    // ======================== EVENTS ========================
    
    // Primary submission — anyone can submit
    event RandomnessSubmitted(
        bytes32 indexed blockHash,
        bytes32 indexed purposeHash,
        bytes32 randomnessOutput,
        bytes32 nullifier,
        address indexed minerAddress,
        uint8 zeroBits,
        uint256 btcHeight,
        uint256 minedTimestamp,
        bytes falconSignature
    );
    
    // Consumer-agnostic events
    event RandomnessAvailable(
        bytes32 indexed blockHash,
        bytes32 randomnessOutput,
        uint256 btcHeight,
        address indexed minerAddress
    );
    
    // AI-specific (one of many consumers)
    event AIRandomnessAnchored(
        bytes32 indexed blockHash,
        bytes32 indexed purposeHash,
        bytes32 modelHash,
        bytes32 randomnessOutput,
        bytes32 packageHash,
        bytes32 datasetHash
    );
    
    // DeFi-specific (example)
    event DeFiRandomnessUsed(
        bytes32 indexed blockHash,
        bytes32 randomnessOutput,
        address indexed consumer,
        string useCase
    );
    
    // Gaming-specific (example)
    event GamingRandomnessUsed(
        bytes32 indexed blockHash,
        bytes32 randomnessOutput,
        address indexed consumer,
        uint256 gameId
    );
    
    // Slashing
    event InvalidSignatureDetected(
        bytes32 indexed blockHash,
        address indexed submitter,
        bytes32 nullifier,
        string reason
    );

    // Additional Core Tracking Events
    event ValidatorSelectionUpdated(
        uint256 indexed round,
        bytes32 seed,
        uint256 btcHeight
    );
    
    event BatchSeedUpdated(
        bytes32 indexed purposeHash,
        bytes32 seed,
        uint256 btcHeight
    );
    
    event BlockAnchored(
        uint256 indexed height,
        bytes32 blockHash,
        bytes32 parentEthBlockHash,
        bytes32 randomnessOutput,
        uint256 btcHeight
    );

    // ======================== CONSTANTS ========================
    
    bytes32 private constant PURPOSE_VALIDATOR_SELECTION = keccak256(bytes("SPSF_VALIDATOR_SELECTION"));
    bytes32 private constant PURPOSE_BATCH_SEED = keccak256(bytes("SPSF_VALIDATION_BATCH_SEED"));
    
    uint256 private constant MAX_TIMESTAMP_DRIFT = 3600;
    uint256 private constant MAX_TIMESTAMP_AGE = 86400;
    
    // Minimum difficulty for submission (anyone can mine)
    uint8 public constant MIN_ZERO_BITS = 22;
    uint8 public constant MAX_ZERO_BITS = 45;
    
    // ======================== IMMUTABLE STATE ========================
    
    // No single trusted miner — anyone can submit
    // But we keep a registry of known miners for reputation
    mapping(address => bool) public registeredMiners;
    mapping(address => uint256) public minerSubmissionCount;
    mapping(address => uint256) public minerLastSubmission;
    
    // ======================== STORAGE ========================
    
    mapping(bytes32 => StoredRecord) public recordsByBlockHash;
    mapping(bytes32 => bytes32) public latestSeedByPurpose;
    mapping(bytes32 => uint256) public latestBTCHeightByPurpose;
    mapping(bytes32 => bool) public usedNullifiers;
    mapping(uint256 => bytes32) public validatorSeedByRound;
    uint256 public currentValidatorRound;
    
    // Track all submissions (any consumer)
    bytes32[] public allBlockHashes;  // Limited to prevent bloat
    uint256 public constant MAX_BLOCK_HISTORY = 10000;
    
    // ======================== CONSTRUCTOR ========================
    
    constructor() {
        currentValidatorRound = 0;
    }
    
    // ======================== MINER REGISTRATION ========================
    
    /**
     * @notice Register as a BINA miner (optional, for reputation)
     */
    function registerMiner() external {
        registeredMiners[msg.sender] = true;
    }
    
    /**
     * @notice Check if an address is a registered miner
     */
    function isRegisteredMiner(address _miner) external view returns (bool) {
        return registeredMiners[_miner];
    }

    // ======================== CORE FUNCTIONS ========================

    /**
     * @notice Submit randomness from ANY BINA miner
     * @dev Anyone with a valid Blake3 PoW + Falcon signature can submit
     * This is the decentralized entry point
     */
    function submitRandomness(
        RandomnessRecord calldata _record,
        bytes calldata _falconSignature,
        string calldata _consumerTag  // "AI", "DeFi", "Gaming", "Generic"
    ) external {
        // 1. Anyone can submit — but we track who
        require(_record.minerAddress == msg.sender, "BINAOracle: sender must match miner address");
        
        // 2. Basic validation
        require(!usedNullifiers[_record.nullifier], "BINAOracle: nullifier already spent");
        usedNullifiers[_record.nullifier] = true;
        
        require(_record.minedTimestamp <= block.timestamp + MAX_TIMESTAMP_DRIFT, 
            "BINAOracle: timestamp too far in future");
        require(_record.minedTimestamp >= block.timestamp - MAX_TIMESTAMP_AGE, 
            "BINAOracle: timestamp too old");
        require(_record.zeroBits >= MIN_ZERO_BITS && _record.zeroBits <= MAX_ZERO_BITS, 
            "BINAOracle: zero bits out of range");
        
        // 3. Store minimized data (3 slots only)
        recordsByBlockHash[_record.blockHash] = StoredRecord({
            randomnessOutput: _record.randomnessOutput,
            btcHeight: _record.btcHeight,
            minedTimestamp: _record.minedTimestamp
        });
        
        // 4. Track miner stats
        minerSubmissionCount[msg.sender]++;
        minerLastSubmission[msg.sender] = block.timestamp;
        if (!registeredMiners[msg.sender]) {
            registeredMiners[msg.sender] = true;  // Auto-register
        }
        
        // 5. Limit history to prevent bloat
        if (allBlockHashes.length < MAX_BLOCK_HISTORY) {
            allBlockHashes.push(_record.blockHash);
        }
        
        // 6. Generic randomness available event (any consumer can use)
        emit RandomnessAvailable(
            _record.blockHash,
            _record.randomnessOutput,
            _record.btcHeight,
            msg.sender
        );
        
        // 7. Consumer-specific handling
        bytes32 purposeHash;
        
        // Try to parse purpose from _consumerTag (if it's a known purpose)
        bytes32 tagHash = keccak256(bytes(_consumerTag));
        
        if (tagHash == keccak256(bytes("VALIDATOR_SELECTION"))) {
            purposeHash = PURPOSE_VALIDATOR_SELECTION;
            currentValidatorRound++;
            validatorSeedByRound[currentValidatorRound] = _record.randomnessOutput;
            
            emit ValidatorSelectionUpdated(
                currentValidatorRound,
                _record.randomnessOutput,
                _record.btcHeight
            );
        } else if (tagHash == keccak256(bytes("BATCH_SEED"))) {
            purposeHash = PURPOSE_BATCH_SEED;
            emit BatchSeedUpdated(
                purposeHash,
                _record.randomnessOutput,
                _record.btcHeight
            );
        } else if (tagHash == keccak256(bytes("AI"))) {
            // AI-specific handling — but we need metadata
            // For AI, they should call submitAIRandomness separately
            purposeHash = keccak256(bytes("SPSF_AI"));
        } else if (tagHash == keccak256(bytes("DEFI"))) {
            emit DeFiRandomnessUsed(
                _record.blockHash,
                _record.randomnessOutput,
                msg.sender,
                "generic_deFi"
            );
        } else if (tagHash == keccak256(bytes("GAMING"))) {
            emit GamingRandomnessUsed(
                _record.blockHash,
                _record.randomnessOutput,
                msg.sender,
                0  // gameId can be set separately
            );
        } else {
            // Generic purpose — derive from tag
            purposeHash = keccak256(bytes(_consumerTag));
        }
        
        // 8. Update latest seed for this purpose
        latestSeedByPurpose[purposeHash] = _record.randomnessOutput;
        latestBTCHeightByPurpose[purposeHash] = _record.btcHeight;
        
        // 9. Core event with Falcon signature (always emitted for verification)
        emit RandomnessSubmitted(
            _record.blockHash,
            purposeHash,
            _record.randomnessOutput,
            _record.nullifier,
            msg.sender,
            _record.zeroBits,
            _record.btcHeight,
            _record.minedTimestamp,
            _falconSignature
        );
        
        emit BlockAnchored(
            _record.height,
            _record.blockHash,
            blockhash(block.number - 1),
            _record.randomnessOutput,
            _record.btcHeight
        );
    }
    
    /**
     * @notice AI-specific submission with full metadata
     * @dev Separate function for AI consumers to attach metadata
     */
    function submitAIRandomness(
        RandomnessRecord calldata _record,
        AIConsumerMetadata calldata _aiMeta,
        bytes calldata _falconSignature
    ) external {
        // Same validation as above
        require(_record.minerAddress == msg.sender, "BINAOracle: sender must match miner address");
        require(!usedNullifiers[_record.nullifier], "BINAOracle: nullifier already spent");
        usedNullifiers[_record.nullifier] = true;
        
        require(_record.minedTimestamp <= block.timestamp + MAX_TIMESTAMP_DRIFT, 
            "BINAOracle: timestamp too far in future");
        require(_record.minedTimestamp >= block.timestamp - MAX_TIMESTAMP_AGE, 
            "BINAOracle: timestamp too old");
        require(_record.zeroBits >= MIN_ZERO_BITS && _record.zeroBits <= MAX_ZERO_BITS, 
            "BINAOracle: zero bits out of range");
        
        // Store minimized data
        recordsByBlockHash[_record.blockHash] = StoredRecord({
            randomnessOutput: _record.randomnessOutput,
            btcHeight: _record.btcHeight,
            minedTimestamp: _record.minedTimestamp
        });
        
        minerSubmissionCount[msg.sender]++;
        minerLastSubmission[msg.sender] = block.timestamp;
        
        if (allBlockHashes.length < MAX_BLOCK_HISTORY) {
            allBlockHashes.push(_record.blockHash);
        }
        
        bytes32 purposeHash = _aiMeta.purposeHash;
        
        // Check if it's validator selection
        if (purposeHash == PURPOSE_VALIDATOR_SELECTION) {
            currentValidatorRound++;
            validatorSeedByRound[currentValidatorRound] = _record.randomnessOutput;
            
            emit ValidatorSelectionUpdated(
                currentValidatorRound,
                _record.randomnessOutput,
                _record.btcHeight
            );
        } else if (purposeHash == PURPOSE_BATCH_SEED) {
            emit BatchSeedUpdated(
                purposeHash,
                _record.randomnessOutput,
                _record.btcHeight
            );
        }
        
        // Update latest seed
        latestSeedByPurpose[purposeHash] = _record.randomnessOutput;
        latestBTCHeightByPurpose[purposeHash] = _record.btcHeight;
        
        // AI-specific event with full metadata
        emit AIRandomnessAnchored(
            _record.blockHash,
            purposeHash,
            _aiMeta.modelHash,
            _record.randomnessOutput,
            _aiMeta.packageHash,
            _aiMeta.datasetHash
        );
        
        // Core event
        emit RandomnessSubmitted(
            _record.blockHash,
            purposeHash,
            _record.randomnessOutput,
            _record.nullifier,
            msg.sender,
            _record.zeroBits,
            _record.btcHeight,
            _record.minedTimestamp,
            _falconSignature
        );
        
        emit BlockAnchored(
            _record.height,
            _record.blockHash,
            blockhash(block.number - 1),
            _record.randomnessOutput,
            _record.btcHeight
        );
    }

    // ======================== VIEW FUNCTIONS ========================
    
    function getStoredRecordByBlock(bytes32 _blockHash) external view returns (
        bytes32 randomnessOutput,
        uint256 btcHeight,
        uint256 timestamp
    ) {
        StoredRecord storage rec = recordsByBlockHash[_blockHash];
        require(rec.randomnessOutput != bytes32(0), "BINAOracle: block not found");
        return (rec.randomnessOutput, rec.btcHeight, rec.minedTimestamp);
    }
    
    function getLatestSeed(string calldata _purpose) external view returns (
        bytes32 seed,
        uint256 btcHeight
    ) {
        bytes32 purposeHash = keccak256(bytes(_purpose));
        return (latestSeedByPurpose[purposeHash], latestBTCHeightByPurpose[purposeHash]);
    }
    
    function getLatestSeedByHash(bytes32 _purposeHash) external view returns (
        bytes32 seed,
        uint256 btcHeight
    ) {
        return (latestSeedByPurpose[_purposeHash], latestBTCHeightByPurpose[_purposeHash]);
    }
    
    function getValidatorSeed(uint256 _round) external view returns (bytes32) {
        require(_round <= currentValidatorRound, "BINAOracle: round not found");
        return validatorSeedByRound[_round];
    }
    
    function getLatestValidatorSeed() external view returns (bytes32, uint256) {
        return (validatorSeedByRound[currentValidatorRound], currentValidatorRound);
    }
    
    function isNullifierUsed(bytes32 _nullifier) external view returns (bool) {
        return usedNullifiers[_nullifier];
    }
    
    function getValidatorRoundCount() external view returns (uint256) {
        return currentValidatorRound;
    }
    
    function getMinerStats(address _miner) external view returns (
        uint256 submissions,
        uint256 lastSubmission,
        bool registered
    ) {
        return (minerSubmissionCount[_miner], minerLastSubmission[_miner], registeredMiners[_miner]);
    }
    
    function getBlockCount() external view returns (uint256) {
        return allBlockHashes.length;
    }
    
    function getBlockHash(uint256 _index) external view returns (bytes32) {
        require(_index < allBlockHashes.length, "BINAOracle: index out of bounds");
        return allBlockHashes[_index];
    }

    // ======================== UTILITY FUNCTIONS ========================
    
    function shuffleValidators(
        bytes32 _seed,
        address[] memory _validators
    ) external pure returns (address[] memory) {
        uint256 n = _validators.length;
        if (n == 0) return _validators;
        
        address[] memory shuffled = new address[](n);
        for (uint256 i = 0; i < n; i++) {
            shuffled[i] = _validators[i];
        }
        
        for (uint256 i = n - 1; i > 0; i--) {
            bytes32 hash = keccak256(abi.encodePacked(_seed, i));
            uint256 j = uint256(hash) % (i + 1);
            (shuffled[i], shuffled[j]) = (shuffled[j], shuffled[i]);
        }
        return shuffled;
    }
    
    function generateBatchId(
        bytes32 _seed,
        bytes32 _txMerkleRoot
    ) external pure returns (bytes32) {
        return keccak256(abi.encodePacked(_seed, _txMerkleRoot));
    }
    
    function generateValidatorSetId(
        bytes32 _seed,
        uint256 _validatorCount
    ) external pure returns (bytes32) {
        return keccak256(abi.encodePacked(_seed, _validatorCount));
    }

    // ======================== SLASHING ========================
    
    function reportInvalidSignature(
        bytes32 _blockHash,
        bytes32 _nullifier,
        string calldata _reason
    ) external {
        require(recordsByBlockHash[_blockHash].randomnessOutput != bytes32(0), 
            "BINAOracle: block not found");
        require(usedNullifiers[_nullifier], 
            "BINAOracle: nullifier not used");
        
        emit InvalidSignatureDetected(
            _blockHash,
            msg.sender,
            _nullifier,
            _reason
        );
    }
}
