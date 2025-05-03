import { ethers } from 'ethers';

/**
 * Fetch and verify entropy from multiple chains
 */
class CrossChainEntropyFetcher {
  private providers: Map<string, ethers.providers.Provider> = new Map();
  private lastEntropySources: Record<string, string> = {};
  
  constructor() {
    // Initialize connections to multiple chains
    this.providers.set('ethereum', new ethers.providers.JsonRpcProvider(process.env.ETH_RPC_URL));
    this.providers.set('arbitrum', new ethers.providers.JsonRpcProvider(process.env.ARB_RPC_URL));
    this.providers.set('optimism', new ethers.providers.JsonRpcProvider(process.env.OP_RPC_URL));
  }

  /**
   * Add a new chain to the entropy sources
   * @param name Chain identifier
   * @param rpcUrl JSON-RPC endpoint URL
   */
  addChain(name: string, rpcUrl: string): void {
    this.providers.set(name, new ethers.providers.JsonRpcProvider(rpcUrl));
  }

  /**
   * Fetch latest block randomness from multiple chains and combine them
   */
  async fetchMultiChainEntropy(): Promise<string> {
    const entropyInputs: string[] = [];
    this.lastEntropySources = {};
    
    // Gather entropy from each chain
    for (const [name, provider] of this.providers.entries()) {
      try {
        const blockData = await provider.getBlock('latest');
        
        // Check if block timestamp is fresh (within 2 minutes)
        const now = Math.floor(Date.now() / 1000);
        if (Math.abs(now - blockData.timestamp) > 120) {
          throw new Error(`${name} block is stale`);
        }
        
        entropyInputs.push(blockData.hash);
        
        // Store the entropy source for debugging/fallback
        this.lastEntropySources[name] = blockData.hash;
      } catch (error) {
        console.error(`Failed to fetch entropy from ${name}:`, error);
      }
    }
    
    // Combine entropy sources
    const combinedEntropy = ethers.utils.solidityKeccak256(
      ['bytes32[]'], 
      [entropyInputs]
    );
    
    return combinedEntropy;
  }
  
  /**
   * Get the most recently fetched entropy sources by chain
   * @returns Record of chain name to entropy hash
   */
  getLastEntropySources(): Record<string, string> {
    return {...this.lastEntropySources}; // Return a copy to prevent mutation
  }
}
