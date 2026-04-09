const { EventEmitter } = require('events');

class MaterializedRandomnessStore extends EventEmitter {
  constructor(options = {}) {
    super();
    this.maxEpochs = options.maxEpochs || 10_000;
    this.maxReceipts = options.maxReceipts || 10_000;
    this.maxCertificates = options.maxCertificates || 10_000;

    this.epochs = new Map();
    this.proofReceipts = new Map();
    this.certificates = new Map();

    this.latestRandomness = null;
    this.lastSyncAt = 0;
  }

  upsertEpoch(epoch) {
    const normalized = this.normalizeEpoch(epoch);
    this.epochs.set(normalized.epochId, normalized);
    this.trimMap(this.epochs, this.maxEpochs);

    if (!this.latestRandomness || normalized.epochId >= this.latestRandomness.epochId) {
      this.latestRandomness = {
        epochId: normalized.epochId,
        outputHash: normalized.merkleRoot,
        finalized: normalized.finalized,
        trustLabel: normalized.trustLabel,
        dataUri: normalized.dataUri,
        sourceChainId: normalized.sourceChainId,
      };
    }

    this.lastSyncAt = Date.now();
    this.emit('epoch:upsert', normalized);
    return normalized;
  }

  upsertProofReceipt(receipt) {
    const normalized = { ...receipt, receiptId: Number(receipt.receiptId) };
    this.proofReceipts.set(normalized.receiptId, normalized);
    this.trimMap(this.proofReceipts, this.maxReceipts);
    this.lastSyncAt = Date.now();
    this.emit('proofReceipt:upsert', normalized);
    return normalized;
  }

  upsertCertificate(certificate) {
    const normalized = { ...certificate, certificateId: Number(certificate.certificateId) };
    this.certificates.set(normalized.certificateId, normalized);
    this.trimMap(this.certificates, this.maxCertificates);
    this.lastSyncAt = Date.now();
    this.emit('certificate:upsert', normalized);
    return normalized;
  }

  getLatestRandomness() {
    return this.latestRandomness;
  }

  getEpoch(epochId) {
    return this.epochs.get(Number(epochId)) || null;
  }

  listEpochs(limit = 20) {
    return Array.from(this.epochs.values())
      .sort((a, b) => b.epochId - a.epochId)
      .slice(0, limit);
  }

  listProofReceipts(limit = 20) {
    return Array.from(this.proofReceipts.values())
      .sort((a, b) => b.receiptId - a.receiptId)
      .slice(0, limit);
  }

  listCertificates(limit = 20) {
    return Array.from(this.certificates.values())
      .sort((a, b) => b.certificateId - a.certificateId)
      .slice(0, limit);
  }

  getSyncStatus() {
    return {
      lastSyncAt: this.lastSyncAt,
      epochCount: this.epochs.size,
      proofReceiptCount: this.proofReceipts.size,
      certificateCount: this.certificates.size,
      latestEpochId: this.latestRandomness ? this.latestRandomness.epochId : null,
    };
  }

  normalizeEpoch(epoch) {
    return {
      ...epoch,
      epochId: Number(epoch.epochId),
      leafCount: Number(epoch.leafCount || 0),
      sourceChainId: Number(epoch.sourceChainId || 0),
      committedAt: Number(epoch.committedAt || 0),
      finalizedAt: Number(epoch.finalizedAt || 0),
      finalized: Boolean(epoch.finalized),
      trustLabel: epoch.trustLabel || (epoch.finalized ? 'epoch-settled' : 'experimental'),
    };
  }

  trimMap(map, maxEntries) {
    while (map.size > maxEntries) {
      const oldestKey = map.keys().next().value;
      map.delete(oldestKey);
    }
  }
}

module.exports = { MaterializedRandomnessStore };