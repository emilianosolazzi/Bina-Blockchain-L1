// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/**
 * @title HardwareSourceVerifier
 * @notice Verifies and scores entropy from hardware sources
 */
contract HardwareSourceVerifier {
    // Source types supported
    enum HardwareSource { RDRAND, TPM, HSM, SGX_ENCLAVE, QUANTUM }
    
    // Verification levels
    uint8 public constant VERIFICATION_NONE = 0;
    uint8 public constant VERIFICATION_BASIC = 1;
    uint8 public constant VERIFICATION_FULL = 2;
    
    // PQC algorithm identifiers
    uint8 public constant PQC_ALGORITHM_KYBER = 1;
    uint8 public constant PQC_ALGORITHM_DILITHIUM = 2;
    uint8 public constant PQC_ALGORITHM_HYBRID = 3; // Combined Kyber+Dilithium
    
    // SGX attestation parameters
    struct SGXAttestation {
        bytes mrEnclave;      // Measurement of the enclave code
        bytes mrSigner;       // Measurement of the signer of the enclave
        uint64 timestamp;     // Attestation timestamp
        bytes quoteBody;      // SGX quote body
        bytes signature;      // Signature over the quote
    }
    
    // PQC operation information
    struct PQCOperation {
        uint8 algorithm;      // Algorithm used (Kyber, Dilithium, or Hybrid)
        bytes publicKey;      // Public key used for verification
        bytes32 operationHash; // Hash of the operation performed
        uint64 executionTimeUs; // Execution time in microseconds (for scoring)
    }
    
    // Mapping to track registered hardware sources per miner
    mapping(address => mapping(HardwareSource => bool)) private registeredSources;
    // Mapping to track verified SGX enclaves
    mapping(bytes => bool) private verifiedEnclaves;
    // Mapping to track performance metrics for scoring
    mapping(address => mapping(uint8 => uint64)) private minerPerformanceMetrics;
    
    event HardwareSourceRegistered(address indexed miner, HardwareSource sourceType);
    event EntropySourceScored(bytes32 indexed entropyHash, uint256 qualityScore);
    event SGXEnclaveVerified(address indexed miner, bytes mrEnclave);
    event PQCOperationRecorded(address indexed miner, uint8 algorithm, uint64 executionTimeUs);
    
    /**
     * @notice Verifies a hardware source attestation
     * @dev For SGX, verifies the attestation against Intel's remote attestation service
     * @param miner Address of the miner registering the hardware source
     * @param sourceType Type of hardware source being registered
     * @param attestation Attestation data specific to the hardware source
     * @param signature Signature over the attestation data
     * @return valid Whether the attestation is valid
     * @return verificationLevel Level of verification achieved
     */
    function verifyHardwareSource(
        address miner,
        HardwareSource sourceType,
        bytes calldata attestation,
        bytes calldata signature
    ) external view returns (bool valid, uint8 verificationLevel) {
        if (sourceType == HardwareSource.SGX_ENCLAVE) {
            // Parse SGX attestation from the attestation data
            SGXAttestation memory sgxData = abi.decode(attestation, (SGXAttestation));
            
            // Verify SGX attestation (in production would call out to IAS or DCAP)
            bool validAttestation = verifySGXAttestation(sgxData, signature);
            if (!validAttestation) {
                return (false, VERIFICATION_NONE);
            }
            
            // Check if this is a trusted enclave
            if (verifiedEnclaves[sgxData.mrEnclave]) {
                return (true, VERIFICATION_FULL);
            } else {
                // First time seeing this enclave - basic verification
                return (true, VERIFICATION_BASIC);
            }
        } else if (sourceType == HardwareSource.QUANTUM) {
            // Quantum source verification would go here
            // Would involve checking signatures/attestations from quantum hardware
            return (true, VERIFICATION_BASIC);
        } else {
            // Handle other hardware sources
            return (true, VERIFICATION_BASIC);
        }
    }
    
    /**
     * @notice Records and verifies a post-quantum crypto operation performed in an SGX enclave
     * @dev Uses SGX to accelerate Kyber and/or Dilithium operations to ~0.5ms
     * @param minerAddress Address of the miner performing the operation
     * @param pqcData Data regarding the PQC operation performed
     * @param attestation SGX attestation for the enclave that performed the operation
     * @return valid Whether the operation is valid
     * @return qualityScore Quality score based on execution time and security level
     */
    function verifyAcceleratedPQCOperation(
        address minerAddress,
        PQCOperation calldata pqcData,
        bytes calldata attestation
    ) external returns (bool valid, uint256 qualityScore) {
        // Decode SGX attestation
        SGXAttestation memory sgxData = abi.decode(attestation, (SGXAttestation));
        
        // Verify if SGX enclave is trusted
        bool isTrustedEnclave = verifiedEnclaves[sgxData.mrEnclave];
        if (!isTrustedEnclave) {
            return (false, 0);
        }
        
        // Check that the miner is registered with this source type
        if (!registeredSources[minerAddress][HardwareSource.SGX_ENCLAVE]) {
            return (false, 0);
        }
        
        // Verify the SGX enclave signed the operation (would call SGX verification)
        bool validOperation = verifyEnclaveSignedOperation(sgxData, pqcData);
        if (!validOperation) {
            return (false, 0);
        }
        
        // Store performance metrics for scoring
        minerPerformanceMetrics[minerAddress][pqcData.algorithm] = pqcData.executionTimeUs;
        
        // Calculate quality score based on execution time
        // Optimal execution time from SGX acceleration should be ~500 microseconds (0.5ms)
        uint256 speedMultiplier;
        if (pqcData.executionTimeUs <= 500) {
            // Below 0.5ms is ideal performance
            speedMultiplier = 100;
        } else if (pqcData.executionTimeUs <= 1000) {
            // Between 0.5ms and 1ms still good
            speedMultiplier = 90 - ((pqcData.executionTimeUs - 500) / 50);
        } else if (pqcData.executionTimeUs <= 5000) {
            // Between 1ms and 5ms acceptable
            speedMultiplier = 80 - ((pqcData.executionTimeUs - 1000) / 500);
        } else {
            // Above 5ms suggests no hardware acceleration
            speedMultiplier = 60;
        }
        
        // Algorithm type affects security score
        uint256 algorithmScore;
        if (pqcData.algorithm == PQC_ALGORITHM_HYBRID) {
            // Highest score for hybrid approach (Kyber + Dilithium)
            algorithmScore = 100;
        } else if (pqcData.algorithm == PQC_ALGORITHM_DILITHIUM) {
            // Dilithium (signatures)
            algorithmScore = 90;
        } else if (pqcData.algorithm == PQC_ALGORITHM_KYBER) {
            // Kyber (encryption)
            algorithmScore = 85;
        } else {
            // Unknown algorithm
            algorithmScore = 50;
        }
        
        // Final quality score combines speed and algorithm security
        qualityScore = (speedMultiplier + algorithmScore) / 2;
        
        emit PQCOperationRecorded(minerAddress, pqcData.algorithm, pqcData.executionTimeUs);
        emit EntropySourceScored(pqcData.operationHash, qualityScore);
        
        return (true, qualityScore);
    }
    
    /**
     * @notice Registers a new hardware source for a miner
     * @param sourceType Type of hardware source to register
     * @param attestation Attestation data for the hardware source
     */
    function registerHardwareSource(
        HardwareSource sourceType,
        bytes calldata attestation,
        bytes calldata signature
    ) external {
        (bool valid, uint8 level) = verifyHardwareSource(msg.sender, sourceType, attestation, signature);
        require(valid, "Invalid attestation");
        
        // Register the hardware source for the miner
        registeredSources[msg.sender][sourceType] = true;
        
        // For SGX enclaves, verify and track the enclave measurement
        if (sourceType == HardwareSource.SGX_ENCLAVE && level >= VERIFICATION_BASIC) {
            SGXAttestation memory sgxData = abi.decode(attestation, (SGXAttestation));
            verifiedEnclaves[sgxData.mrEnclave] = true;
            emit SGXEnclaveVerified(msg.sender, sgxData.mrEnclave);
        }
        
        emit HardwareSourceRegistered(msg.sender, sourceType);
    }
    
    /**
     * @notice Verifies SGX attestation against Intel's remote attestation service
     * @dev In production, this would call out to IAS or DCAP for verification
     * @param attestation SGX attestation data
     * @param signature Signature over the attestation
     * @return valid Whether the attestation is valid
     */
    function verifySGXAttestation(
        SGXAttestation memory attestation,
        bytes calldata signature
    ) internal pure returns (bool valid) {
        // Simplified verification for demo purposes
        // In production, this would verify against Intel Attestation Service
        
        // Check that enclave measurements are non-zero
        if (attestation.mrEnclave.length == 0 || attestation.mrSigner.length == 0) {
            return false;
        }
        
        // Check quote signature (simplified)
        if (attestation.signature.length == 0) {
            return false;
        }
        
        // In a real implementation, would verify the SGX quote signature
        // using Intel's public key and check enclave attributes
        
        return true;
    }
    
    /**
     * @notice Verifies that an operation was signed by a verified SGX enclave
     * @param attestation SGX attestation data for the enclave
     * @param operation PQC operation data
     * @return valid Whether the operation signature is valid
     */
    function verifyEnclaveSignedOperation(
        SGXAttestation memory attestation,
        PQCOperation calldata operation
    ) internal pure returns (bool) {
        // In production would verify the operation was performed inside the attested enclave
        // This would involve checking signatures against the enclave's public key
        
        // Basic check: operation time makes sense for SGX acceleration (~0.5ms for Kyber/Dilithium)
        if (operation.executionTimeUs < 100 || operation.executionTimeUs > 10000) {
            return false; // Unrealistic execution time
        }
        
        // For simplicity in this sample implementation, assume valid if properly formatted
        return (operation.algorithm > 0 && 
                operation.operationHash != bytes32(0) && 
                operation.publicKey.length > 0);
    }
    
    /**
     * @notice Gets performance metrics for a miner's PQC operations
     * @param miner Address of the miner
     * @param algorithm PQC algorithm identifier
     * @return executionTimeUs The last recorded execution time in microseconds
     */
    function getMinerPerformanceMetrics(
        address miner, 
        uint8 algorithm
    ) external view returns (uint64 executionTimeUs) {
        return minerPerformanceMetrics[miner][algorithm];
    }
}
