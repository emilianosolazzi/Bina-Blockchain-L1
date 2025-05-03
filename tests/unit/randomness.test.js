const { RandomnessService } = require('../../services/RandomnessService');
const { SecureBuffer } = require('../../memory');
const { ethers } = require('ethers');
const MockProvider = require('../mocks/MockProvider');
const MockContract = require('../mocks/MockContract');

jest.mock('../../memory');
jest.mock('ethers');

describe('RandomnessService', () => {
  let service;
  let mockProvider;
  let mockContract;

  beforeEach(() => {
    mockProvider = new MockProvider();
    mockContract = new MockContract();
    
    // Mock provider and contract setup
    ethers.providers.JsonRpcProvider.mockImplementation(() => mockProvider);
    ethers.Contract.mockImplementation(() => mockContract);
    
    // Setup secure buffer mocks
    SecureBuffer.mockImplementation(function(size) {
      this.size = size;
      this.buffer = Buffer.alloc(size);
      this.clean = jest.fn();
      this.as_slice = jest.fn(() => this.buffer);
      this.as_mut_slice = jest.fn(() => this.buffer);
      return this;
    });

    // Initialize service
    service = new RandomnessService({
      rpcUrl: 'http://localhost:8545',
      contractAddress: '0x1234567890123456789012345678901234567890',
      privateKeyFile: './test.key',
    });
  });

  describe('requestRandomness', () => {
    it('should successfully request randomness with valid parameters', async () => {
      // Arrange
      const userSeed = ethers.utils.randomBytes(32);
      const requestId = 123;
      mockContract.requestRandomness.mockResolvedValue({ 
        wait: jest.fn().mockResolvedValue({ events: [{ args: { requestId } }] }) 
      });
      
      // Act
      const result = await service.requestRandomness(userSeed, { requesterId: 'user123', kycVerified: true });
      
      // Assert
      expect(result.requestId).toBe(requestId);
      expect(mockContract.requestRandomness).toHaveBeenCalledWith(
        expect.anything(),
        expect.objectContaining({ 
          gasLimit: expect.any(Number) 
        })
      );
    });

    it('should require KYC verification', async () => {
      // Arrange
      const userSeed = ethers.utils.randomBytes(32);
      
      // Act & Assert
      await expect(
        service.requestRandomness(userSeed, { requesterId: 'user123', kycVerified: false })
      ).rejects.toThrow('KYC verification required');
    });
  });

  describe('contributeEntropy', () => {
    it('should successfully contribute entropy to a request', async () => {
      // Arrange
      const requestId = 123;
      const entropy = ethers.utils.randomBytes(32);
      
      mockContract.contributeEntropy.mockResolvedValue({ 
        wait: jest.fn().mockResolvedValue({ status: 1 }) 
      });
      
      // Act
      const result = await service.contributeEntropy(requestId, entropy);
      
      // Assert
      expect(result.success).toBe(true);
      expect(mockContract.contributeEntropy).toHaveBeenCalledWith(
        requestId,
        expect.anything(),
        expect.objectContaining({ 
          gasLimit: expect.any(Number) 
        })
      );
    });
  });

  describe('getRandomResult', () => {
    it('should retrieve fulfilled randomness result', async () => {
      // Arrange
      const requestId = 123;
      const mockResult = '0x' + '1'.repeat(64);
      mockContract.getRandomResult.mockResolvedValue(mockResult);
      
      // Act
      const result = await service.getRandomResult(requestId);
      
      // Assert
      expect(result).toBe(mockResult);
      expect(mockContract.getRandomResult).toHaveBeenCalledWith(requestId);
    });

    it('should throw error for unfulfilled request', async () => {
      // Arrange
      const requestId = 456;
      mockContract.getRandomResult.mockRejectedValue(new Error('request not fulfilled'));
      
      // Act & Assert
      await expect(service.getRandomResult(requestId)).rejects.toThrow('request not fulfilled');
    });
  });

  describe('shutdown', () => {
    it('should clean up resources', async () => {
      // Act
      await service.shutdown();
      
      // Assert
      expect(service.keyBuffer.clean).toHaveBeenCalled();
    });
  });
});
