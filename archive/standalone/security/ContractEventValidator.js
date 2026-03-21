const { ethers } = require('ethers');
const { Logger } = require('../utils/Logger');

/**
 * Validates smart contract events against known signatures using CRC checks
 * @class ContractEventValidator
 */
class ContractEventValidator {
  /**
   * Creates a new ContractEventValidator instance
   * @param {Object} signatures Event signature CRCs, mapping from event name to CRC value
   */
  constructor(signatures = {}) {
    this.signatures = signatures;
    this.validationCache = new Map(); // Cache validation results
    
    Logger.info('ContractEventValidator initialized');
  }
  
  /**
   * Validates an event against its known ABI signature
   * @param {String} eventName Name of the event 
   * @param {Array} eventArgs Event arguments received from the contract
   * @returns {Boolean} Whether the event matches its expected signature
   */
  validateEvent(eventName, eventArgs) {
    try {
      // If we don't have a signature for this event, consider it valid
      if (!this.signatures[eventName]) {
        return true;
      }
      
      // Check if we have a cached result for this event instance
      const cacheKey = this.getCacheKey(eventName, eventArgs);
      if (this.validationCache.has(cacheKey)) {
        return this.validationCache.get(cacheKey);
      }
      
      // Extract the event object (usually first argument)
      const event = eventArgs[0];
      if (!event) {
        Logger.warn(`No event object found for ${eventName}`);
        return false;
      }
      
      // Calculate CRC of event signature components
      const calculatedCrc = this.calculateEventCrc(event);
      const expectedCrc = this.signatures[eventName];
      
      // Compare CRC values
      const isValid = calculatedCrc === expectedCrc;
      
      if (!isValid) {
        Logger.warn(`Event validation failed for ${eventName}: CRC mismatch ` +
          `(calculated: ${calculatedCrc}, expected: ${expectedCrc})`);
      }
      
      // Cache the result
      this.validationCache.set(cacheKey, isValid);
      
      return isValid;
      
    } catch (error) {
      Logger.error(`Error validating event ${eventName}:`, error);
      return false;
    }
  }
  
  /**
   * Creates a cache key for an event instance
   * @private
   * @param {String} eventName Event name
   * @param {Array} eventArgs Event arguments
   * @returns {String} Cache key
   */
  getCacheKey(eventName, eventArgs) {
    try {
      // Extract transaction hash if available
      let txHash = '';
      if (eventArgs[0] && eventArgs[0].transactionHash) {
        txHash = eventArgs[0].transactionHash;
      }
      
      return `${eventName}-${txHash}`;
    } catch (error) {
      // If we can't create a proper key, use a randomized one
      return `${eventName}-${Date.now()}-${Math.random()}`;
    }
  }
  
  /**
   * Calculate CRC value for an event signature
   * @private
   * @param {Object} event The event object 
   * @returns {String} CRC value
   */
  calculateEventCrc(event) {
    try {
      // Extract signature components
      const components = [
        event.event || '',
        event.signature || '',
        (event.args && Object.keys(event.args).length) || 0,
        event.address || '0x0'
      ];
      
      // Convert components to a string representation
      const signature = components.join('|');
      
      // Calculate CRC32 from signature
      return this.calculateCrc32(signature);
      
    } catch (error) {
      Logger.error('Error calculating event CRC:', error);
      return '0';
    }
  }
  
  /**
   * Calculate CRC32 value of a string
   * @private
   * @param {String} str Input string
   * @returns {String} CRC32 as hex string
   */
  calculateCrc32(str) {
    let crc = -1;
    for (let i = 0; i < str.length; i++) {
      const byte = str.charCodeAt(i);
      crc = (crc >>> 8) ^ this.crcTable[(crc ^ byte) & 0xFF];
    }
    // Finalize CRC
    crc = (crc ^ (-1)) >>> 0;
    return '0x' + crc.toString(16).padStart(8, '0');
  }
  
  // CRC32 lookup table
  get crcTable() {
    if (!this._crcTable) {
      this._crcTable = new Uint32Array(256);
      
      for (let i = 0; i < 256; i++) {
        let c = i;
        for (let j = 0; j < 8; j++) {
          c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1);
        }
        this._crcTable[i] = c;
      }
    }
    
    return this._crcTable;
  }
  
  /**
   * Register a new event signature
   * @param {String} eventName Event name
   * @param {String} crcValue CRC value for this event
   */
  registerSignature(eventName, crcValue) {
    this.signatures[eventName] = crcValue;
    
    // Clear any cached validations for this event
    for (const key of this.validationCache.keys()) {
      if (key.startsWith(eventName + '-')) {
        this.validationCache.delete(key);
      }
    }
  }
  
  /**
   * Clear the validation cache
   */
  clearCache() {
    this.validationCache.clear();
  }
}

module.exports = { ContractEventValidator };
