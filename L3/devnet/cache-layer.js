const { EventEmitter } = require('events');

class CacheLayer extends EventEmitter {
  constructor(options = {}) {
    super();
    this.defaultTtlMs = options.defaultTtlMs || 5_000;
    this.maxEntries = options.maxEntries || 1_000;
    this.store = new Map();
    this.stats = {
      hits: 0,
      misses: 0,
      evictions: 0,
      sets: 0,
      deletes: 0,
      staleReads: 0,
    };
  }

  set(key, value, ttlMs = this.defaultTtlMs) {
    if (this.store.size >= this.maxEntries && !this.store.has(key)) {
      this.evictOldest();
    }

    this.store.set(key, {
      value,
      createdAt: Date.now(),
      expiresAt: Date.now() + Math.max(1, ttlMs),
    });
    this.stats.sets += 1;
    this.emit('cache:set', { key });
    return value;
  }

  get(key, options = {}) {
    const entry = this.store.get(key);
    if (!entry) {
      this.stats.misses += 1;
      return null;
    }

    const now = Date.now();
    if (entry.expiresAt <= now) {
      if (options.allowStale === true) {
        this.stats.hits += 1;
        this.stats.staleReads += 1;
        return { ...entry.value, _cache: { stale: true, ageMs: now - entry.createdAt } };
      }
      this.store.delete(key);
      this.stats.misses += 1;
      this.stats.deletes += 1;
      return null;
    }

    this.stats.hits += 1;
    return { ...entry.value, _cache: { stale: false, ageMs: now - entry.createdAt } };
  }

  getOrCompute(key, computeFn, ttlMs = this.defaultTtlMs) {
    const cached = this.get(key);
    if (cached) {
      return Promise.resolve(cached);
    }

    return Promise.resolve(computeFn()).then((value) => this.set(key, value, ttlMs));
  }

  delete(key) {
    const deleted = this.store.delete(key);
    if (deleted) {
      this.stats.deletes += 1;
      this.emit('cache:delete', { key });
    }
    return deleted;
  }

  clear() {
    this.store.clear();
    this.emit('cache:clear');
  }

  evictOldest() {
    const oldestKey = this.store.keys().next().value;
    if (oldestKey !== undefined) {
      this.store.delete(oldestKey);
      this.stats.evictions += 1;
      this.emit('cache:evict', { key: oldestKey });
    }
  }

  getStats() {
    return {
      ...this.stats,
      size: this.store.size,
      hitRate: (this.stats.hits + this.stats.misses) === 0
        ? 0
        : this.stats.hits / (this.stats.hits + this.stats.misses),
    };
  }
}

module.exports = { CacheLayer };