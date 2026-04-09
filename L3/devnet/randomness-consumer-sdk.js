class TemporalGradientL3Client {
  constructor(options = {}) {
    this.baseUrl = (options.baseUrl || 'http://127.0.0.1:4385').replace(/\/$/, '');
    this.fetchImpl = options.fetchImpl || globalThis.fetch;

    if (!this.fetchImpl) {
      throw new Error('A fetch implementation is required');
    }
  }

  async getHealth() {
    return this._get('/api/health');
  }

  async getLatestRandomness() {
    return this._get('/api/randomness/latest');
  }

  async getEpochs(limit = 20) {
    return this._get(`/api/epochs?limit=${encodeURIComponent(limit)}`);
  }

  async getEpoch(epochId) {
    return this._get(`/api/epochs/${encodeURIComponent(epochId)}`);
  }

  async getProofReceipts(limit = 20) {
    return this._get(`/api/proof-receipts?limit=${encodeURIComponent(limit)}`);
  }

  async getCertificates(limit = 20) {
    return this._get(`/api/certificates?limit=${encodeURIComponent(limit)}`);
  }

  async getRandomnessBundle() {
    const latest = await this.getLatestRandomness();
    let epoch = null;

    if (latest?.epochId !== undefined && latest?.epochId !== null) {
      epoch = await this.getEpoch(latest.epochId);
    }

    return {
      latest,
      epoch,
      trustSummary: this.summarizeTrust(latest, epoch),
    };
  }

  summarizeTrust(latest, epoch) {
    if (!latest) {
      return {
        level: 'unavailable',
        message: 'No randomness is currently available.',
      };
    }

    const label = latest.trustLabel || epoch?.trustLabel || 'experimental';
    const map = {
      'epoch-settled': 'Randomness is settled at the epoch layer.',
      'proof-verifiable': 'Randomness has proof-verifiable delivery context.',
      'externally-anchored': 'Randomness includes stronger external provenance context.',
      experimental: 'Randomness is available but should be treated as experimental.',
    };

    return {
      level: label,
      message: map[label] || map.experimental,
    };
  }

  async requireTrustLevel(requiredLevel) {
    const bundle = await this.getRandomnessBundle();
    const actual = bundle.trustSummary.level;
    const ranking = ['unavailable', 'experimental', 'epoch-settled', 'proof-verifiable', 'externally-anchored'];

    if (ranking.indexOf(actual) < ranking.indexOf(requiredLevel)) {
      throw new Error(`Required trust level '${requiredLevel}' not satisfied. Actual level: '${actual}'.`);
    }

    return bundle;
  }

  async _get(route) {
    const response = await this.fetchImpl(`${this.baseUrl}${route}`);
    const payload = await response.json().catch(() => ({}));

    if (!response.ok) {
      throw new Error(payload.error || `Request failed with status ${response.status}`);
    }

    return payload;
  }
}

if (typeof module !== 'undefined') {
  module.exports = { TemporalGradientL3Client };
}

if (typeof window !== 'undefined') {
  window.TemporalGradientL3Client = TemporalGradientL3Client;
}