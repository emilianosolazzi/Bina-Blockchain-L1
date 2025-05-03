const { ethers } = require('ethers');
const path = require('path');
const fs = require('fs').promises;
const { SecureBuffer } = require('../memory');
const { ShardManager } = require('../sharding/ShardManager');
const { KYCValidator } = require('../security/KYCValidator');
const { PrometheusMetrics } = require('../metrics/PrometheusMetrics');
const { Logger } = require('../utils/Logger');
const { ContractEventValidator } = require('../security/ContractEventValidator');
const { ABI_CRC_SIGNATURES } = require('../constants/abiSignatures');

/**
 * Service for interacting with the Temporal Gradient Beacon for randomness
 */
class RandomnessService {
  /**
   * Creates a new RandomnessService instance
   * @param {Object} options Configuration options
   * @param {String} options.rpcUrl RPC URL to connect to the blockchain
   * @param {String} options.contractAddress The beacon contract address
   * @param {String} options.privateKeyFile Path to private key file (or can use signer directly)
   * @param {ethers.Signer} options.signer Ethers signer (alternative to privateKeyFile)
   * @param {Object} options.kycValidator KYC validator instance or config
   * @param {Object} options.shardManager ShardManager instance or config
   */
  constructor(options = {}) {
    this.options = options;
    this.contractAbi = require('../abis/TemporalGradientBeacon.json');
    
    // Initialize blockchain provider and contract
    this.provider = options.provider || new ethers.providers.JsonRpcProvider(options.rpcUrl);
    
    // Initialize signer
    this.initializeSigner(options);
    
    // Initialize contract instance
    this.contract = new ethers.Contract(
      options.contractAddress,
      this.contractAbi,
      this.signer || this.provider
    );
    
    // Initialize KYC validator
    this.kycValidator = options.kycValidator instanceof KYCValidator ?
      options.kycValidator :
      new KYCValidator(options.kycValidator || {});
      
    // Initialize shard manager for load balancing
    this.shardManager = options.shardManager instanceof ShardManager ?
      options.shardManager :
      new ShardManager(options.shardManager || {});
      
    // Initialize event validator
    this.eventValidator = new ContractEventValidator(ABI_CRC_SIGNATURES);
    
    // Initialize metrics
    this.metrics = new PrometheusMetrics();
    this.initMetrics();
    
    // Wait for initialization to complete
    this.initialized = Promise.resolve().then(() => {
      Logger.info(`RandomnessService initialized with contract: ${options.contractAddress}`);
      this.startBackgroundTasks();
      
      return true;
    });
  }
  
  /**
   * Initializes ethers signer
   * @private
   */
  async initializeSigner(options) {
    // If signer provided directly, use it
    if (options.signer instanceof ethers.Signer) {
      this.signer = options.signer;
      return;
    }
    
    // Otherwise try to load private key from file
    if (options.privateKeyFile) {
      try {
        // Use SecureBuffer for key storage
        this.keyBuffer = new SecureBuffer(32);
        
        // Read key file
        const keyData = await fs.readFile(options.privateKeyFile, 'utf-8');
        const privateKey = keyData.trim();
        
        // Validate key format
        if (!privateKey.match(/^(0x)?[0-9a-fA-F]{64}$/)) {
          throw new Error('Invalid private key format');
        }
        
        // Store key in secure buffer
        const keyBytes = Buffer.from(privateKey.replace(/^0x/, ''), 'hex');
        for (let i = 0; i < keyBytes.length; i++) {
          this.keyBuffer.as_mut_slice()[i] = keyBytes[i];
        }
        
        // Create a wallet without storing the key in a JavaScript string
        const wallet = new ethers.Wallet(keyBytes, this.provider);
        this.signer = wallet;
        
        // Clear the raw bytes
        keyBytes.fill(0);
        
      } catch (error) {
        Logger.error('Failed to load private key:', error);
        throw new Error('Failed to initialize signer: ' + error.message);
      }
    }
  }
  
  /**
   * Updates the current signer
   * @param {ethers.Signer} newSigner The new signer to use
   */
  updateSigner(newSigner) {
    this.signer = newSigner;
    this.contract = this.contract.connect(newSigner);
  }
  
  /**
   * Initialize Prometheus metrics
   * @private
   */
  initMetrics() {
    this.metrics.registerCounter('randomness_requests_total', 'Total randomness requests');
    this.metrics.registerCounter('randomness_contributions_total', 'Total entropy contributions');
    this.metrics.registerCounter('randomness_fulfilled_total', 'Total fulfilled randomness requests');
    this.metrics.registerCounter('randomness_failures_total', 'Failed randomness operations', ['operation']);
    this.metrics.registerHistogram('randomness_request_duration', 'Randomness request duration in ms');
    this.metrics.registerGauge('kyc_verifications_active', 'Number of active KYC verifications');
  }
  
  /**
   * Start background tasks
   * @private
   */
  startBackgroundTasks() {
    // Listen for shard events
    this.shardManager.on('shard:failover', this.handleShardFailover.bind(this));
    this.shardManager.on('shard:recovered', this.handleShardRecovery.bind(this));
    
    // Set up event validation
    this.setupEventValidation();
  }
  
  /**
   * Sets up event validation for contract events
   * @private
   */
  setupEventValidation() {
    // Override provider listeners to add validation
    const originalOn = this.contract.on.bind(this.contract);
    
    this.contract.on = (event, listener) => {
      const wrappedListener = (...args) => {
        try {
          // Validate event structure before passing to listener
          if (this.eventValidator.validateEvent(event, args)) {
            listener(...args);
          } else {
            Logger.warn(`Invalid event received for ${event} - CRC check failed`);
          }
        } catch (error) {
          Logger.error(`Error in event validation for ${event}:`, error);
        }
      };
      
      return originalOn(event, wrappedListener);
    };
  }
  
  /**
   * Handle shard failover event
   * @private
   */
  handleShardFailover(data) {
    Logger.info(`Handling shard failover from ${data.shardId}`);
    // Implement failover logic here, such as:
    // - Redirect pending requests to new shards
    // - Update internal state for sharding awareness
  }
  
  /**
   * Handle shard recovery event
   * @private
   */
  handleShardRecovery(data) {
    Logger.info(`Shard ${data.shardId} has recovered`);
    // Implement recovery logic here
  }

  /**
   * Request randomness from the beacon
   * @param {Bytes|String} userSeed User-provided seed for randomness
   * @param {Object} options Additional options
   * @param {String} options.requesterId ID of the requester for tracking
   * @param {Boolean} options.kycVerified Whether KYC check has already been performed
   * @param {Boolean} options.highPriority Whether this is a high priority request
   * @returns {Promise<Object>} Request result
   */
  async requestRandomness(userSeed, options = {}) {
    await this.initialized;
    const startTime = Date.now();
    
    try {
      // Wait for a good shard to handle this request
      const currentShard = this.shardManager.getShardForRequest(
        { type: 'requestRandomness', seed: userSeed },
        { highPriority: options.highPriority }
      );
      
      // Always verify KYC status if not explicitly verified
      let kycVerified = !!options.kycVerified;
      if (!kycVerified) {
        if (!options.requesterId) {
          throw new Error('requesterId is required for KYC verification');
        }
        
        this.metrics.incrementGauge('kyc_verifications_active');
        kycVerified = await this.kycValidator.checkKYCStatus(options.requesterId);
        this.metrics.decrementGauge('kyc_verifications_active');
        
        if (!kycVerified) {
          Logger.warn(`KYC verification failed for ${options.requesterId}`);
          throw new Error('KYC verification required');
        }
      }
      
      // Normalize user seed to bytes32
      const seedBytes = ethers.utils.isHexString(userSeed) ?
        ethers.utils.arrayify(userSeed) :
        ethers.utils.toUtf8Bytes(userSeed);
      
      // Hash seed to ensure it's a valid bytes32
      const normalizedSeed = ethers.utils.keccak256(seedBytes);
      
      // Make the contract call
      const tx = await this.contract.requestRandomness(normalizedSeed, {
        gasLimit: 500000,
      });
      
      // Wait for transaction to be mined
      const receipt = await tx.wait();
      
      // Find the RequestRandomness event to get the request ID
      let requestId = null;
      for (const event of receipt.events || []) {
        if (event.event === 'RandomnessRequested') {
          requestId = event.args.requestId.toNumber();
          break;
        }
      }
      
      if (!requestId) {
        throw new Error('Could not find request ID in transaction events');
      }
      
      // Report metrics
      this.metrics.incrementCounter('randomness_requests_total');
      this.metrics.observeHistogram('randomness_request_duration', Date.now() - startTime);
      
      // Update shard status for load balancing
      this.reportShardStatus(currentShard);
      
      return {
        requestId,
        transactionHash: receipt.transactionHash,
        blockNumber: receipt.blockNumber,
        gasUsed: receipt.gasUsed.toString(),
      };
    } catch (error) {
      Logger.error('Error requesting randomness:', error);
      this.metrics.incrementCounter('randomness_failures_total', { operation: 'request' });
      throw error;
    }
  }
  
  /**
   * Contribute entropy to a randomness request
   * @param {Number} requestId The request ID to contribute to
   * @param {Bytes|String} entropy Entropy to contribute
   * @returns {Promise<Object>} Contribution result
   */
  async contributeEntropy(requestId, entropy) {
    await this.initialized;
    const startTime = Date.now();
    
    try {
      // Get shard
      const currentShard = this.shardManager.getShardForRequest(
        { type: 'contributeEntropy', requestId },
        {}
      );
      
      // Normalize entropy
      let entropyBytes;
      if (ethers.utils.isHexString(entropy)) {
        entropyBytes = entropy;
      } else {
        // Hash any non-hex input to get bytes32
        entropyBytes = ethers.utils.keccak256(
          typeof entropy === 'string' ? ethers.utils.toUtf8Bytes(entropy) : entropy
        );
      }
      
      // Make the contract call
      const tx = await this.contract.contributeEntropy(requestId, entropyBytes, {
        gasLimit: 300000,
      });
      
      // Wait for transaction
      const receipt = await tx.wait();
      
      // Report metrics
      this.metrics.incrementCounter('randomness_contributions_total');
      
      // Update shard status
      this.reportShardStatus(currentShard);
      
      return {
        success: receipt.status === 1,
        transactionHash: receipt.transactionHash,
        blockNumber: receipt.blockNumber,
      };
    } catch (error) {
      Logger.error('Error contributing entropy:', error);
      this.metrics.incrementCounter('randomness_failures_total', { operation: 'contribute' });
      throw error;
    }
  }
  
  /**
   * Get random result for a fulfilled request
   * @param {Number} requestId The request ID
   * @returns {Promise<String>} The random result as bytes32 hex string
   */
  async getRandomResult(requestId) {
    await this.initialized;
    
    try {
      const result = await this.contract.getRandomResult(requestId);
      return result;
    } catch (error) {
      Logger.error('Error getting random result:', error);
      this.metrics.incrementCounter('randomness_failures_total', { operation: 'getResult' });
      throw error;
    }
  }
  
  /**
   * Derive multiple random values from a single fulfilled request
   * @param {Number} requestId The fulfilled request ID
   * @param {Number} count Number of values to derive
   * @returns {Promise<Array>} Array of derived random values
   */
  async deriveRandomValues(requestId, count) {
    const baseRandom = await this.getRandomResult(requestId);
    const results = [];
    
    for (let i = 0; i < count; i++) {
      const derived = ethers.utils.keccak256(
        ethers.utils.solidityPack(['bytes32', 'uint256'], [baseRandom, i])
      );
      results.push(derived);
    }
    
    return results;
  }
  
  /**
   * Report current shard status for load balancing
   * @private
   * @param {String} shardId The current shard ID
   */
  reportShardStatus(shardId) {
    // In a real implementation, calculate these values based on actual load
    const pendingRequests = Math.floor(Math.random() * 100); // Placeholder
    const requestsPerSecond = Math.floor(Math.random() * 50); // Placeholder
    const load = pendingRequests / 200; // Simple load calculation
    
    this.shardManager.reportShardStatus(shardId, {
      load,
      pendingRequests,
      requestsPerSecond,
      healthy: true
    });
  }
  
  /**
   * Clean up resources
   */
  async shutdown() {
    // Clean up sensitive key material
    if (this.keyBuffer) {
      this.keyBuffer.clean();
    }
    
    // Clean up shard manager
    if (this.shardManager) {
      this.shardManager.shutdown();
    }
    
    Logger.info('RandomnessService shut down');
  }
}

module.exports = { RandomnessService };
