const { RandomnessService } = require('../../services/RandomnessService');
const { ethers } = require('ethers');
const { testProvider, getSigners, deployContracts } = require('../helpers/testSetup');

// Mark as integration tests (can be skipped in CI if needed)
jest.setTimeout(60000); // Longer timeout for blockchain interaction

describe('Randomness End-to-End Flow', () => {
  let service;
  let contracts;
  let user;
  let contributor1;
  let contributor2;
  let contributor3;

  beforeAll(async () => {
    // Deploy contracts and get signers
    [user, contributor1, contributor2, contributor3] = await getSigners();
    contracts = await deployContracts();

    // Initialize service with actual contract connection
    service = new RandomnessService({
      provider: testProvider,
      contractAddress: contracts.beaconAddress,
      signer: contributor1,
      kycValidator: {
        checkKYCStatus: async (user) => {
          // Mock KYC validator that approves test users
          return user.startsWith('test-');
        }
      },
      shardManager: {
        getCurrentShard: jest.fn().mockReturnValue('shard-1'),
        getShardLoad: jest.fn().mockReturnValue(0.5),
        reportShardStatus: jest.fn()
      }
    });
  });

  afterAll(async () => {
    await service.shutdown();
  });

  it('should complete full randomness request-contribute-fulfill flow', async () => {
    // Step 1: Request randomness
    const userSeed = ethers.utils.randomBytes(32);
    const requestResult = await service.requestRandomness(
      userSeed, 
      { requesterId: 'test-user-1', kycVerified: true }
    );
    expect(requestResult.requestId).toBeDefined();
    
    // Capture request ID
    const requestId = requestResult.requestId;
    
    // Step 2: Multiple contributors add entropy
    const contribution1 = await service.contributeEntropy(
      requestId, 
      ethers.utils.hexlify(ethers.utils.randomBytes(32))
    );
    expect(contribution1.success).toBe(true);
    
    // Switch to second contributor
    service.updateSigner(contributor2);
    const contribution2 = await service.contributeEntropy(
      requestId, 
      ethers.utils.hexlify(ethers.utils.randomBytes(32))
    );
    expect(contribution2.success).toBe(true);
    
    // Switch to third contributor
    service.updateSigner(contributor3);
    const contribution3 = await service.contributeEntropy(
      requestId, 
      ethers.utils.hexlify(ethers.utils.randomBytes(32))
    );
    expect(contribution3.success).toBe(true);
    
    // Step 3: Request should be automatically fulfilled after enough contributions
    
    // Small delay to allow for blockchain processing
    await new Promise(resolve => setTimeout(resolve, 5000));
    
    // Step 4: Verify that randomness is available and valid
    const randomResult = await service.getRandomResult(requestId);
    expect(randomResult).toMatch(/^0x[0-9a-f]{64}$/i);
    
    // Step 5: Verify result can be used for derived values
    const derivedValues = await service.deriveRandomValues(requestId, 5);
    expect(derivedValues.length).toBe(5);
    derivedValues.forEach(value => {
      expect(value).toMatch(/^0x[0-9a-f]{64}$/i);
    });
  });

  it('should fail request without KYC verification', async () => {
    const userSeed = ethers.utils.randomBytes(32);
    
    await expect(
      service.requestRandomness(userSeed, { requesterId: 'unverified-user', kycVerified: false })
    ).rejects.toThrow('KYC verification required');
  });
});
