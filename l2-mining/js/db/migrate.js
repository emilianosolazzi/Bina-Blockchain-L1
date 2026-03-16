// db/migrate.js
// Run: node db/migrate.js
// Creates the required PostgreSQL tables for the beacon API.

import pg from 'pg';
const { Pool } = pg;

const DATABASE_URL = process.env.DATABASE_URL;
if (!DATABASE_URL) {
  console.error('[FATAL] DATABASE_URL not set');
  process.exit(1);
}

const pool = new Pool({ connectionString: DATABASE_URL });

const MIGRATIONS = [
  {
    name: '001_randomness_requests',
    sql: `
      CREATE TABLE IF NOT EXISTS randomness_requests (
        id                    SERIAL PRIMARY KEY,
        request_id            UUID UNIQUE NOT NULL,
        client_ip             TEXT,
        user_agent            TEXT,
        endpoint              TEXT NOT NULL,
        entropy_source        TEXT,
        num_words             INTEGER DEFAULT 0,
        seed                  TEXT,
        output                TEXT,
        randomness            JSONB,
        success               BOOLEAN NOT NULL DEFAULT TRUE,
        error_message         TEXT,
        timestamp             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        fulfilled_at          TIMESTAMPTZ,
        webhook_url           TEXT,
        webhook_status        TEXT,
        webhook_response_code INTEGER,
        webhook_response      TEXT
      );
      CREATE INDEX IF NOT EXISTS idx_rr_request_id ON randomness_requests(request_id);
      CREATE INDEX IF NOT EXISTS idx_rr_timestamp  ON randomness_requests(timestamp DESC);
      CREATE INDEX IF NOT EXISTS idx_rr_endpoint   ON randomness_requests(endpoint);
    `,
  },
  {
    name: '002_api_keys',
    sql: `
      CREATE TABLE IF NOT EXISTS api_keys (
        id          SERIAL PRIMARY KEY,
        key_hash    TEXT UNIQUE NOT NULL,
        label       TEXT NOT NULL,
        tier        TEXT NOT NULL DEFAULT 'free',
        rate_limit  INTEGER NOT NULL DEFAULT 100,
        active      BOOLEAN NOT NULL DEFAULT TRUE,
        created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        last_used   TIMESTAMPTZ
      );
      CREATE INDEX IF NOT EXISTS idx_ak_key_hash ON api_keys(key_hash);
    `,
  },
  {
    name: '003_migrations_tracker',
    sql: `
      CREATE TABLE IF NOT EXISTS _migrations (
        name       TEXT PRIMARY KEY,
        applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
      );
    `,
  },
];

async function migrate() {
  const client = await pool.connect();
  try {
    // Ensure migrations table exists first
    await client.query(`
      CREATE TABLE IF NOT EXISTS _migrations (
        name       TEXT PRIMARY KEY,
        applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
      );
    `);

    for (const m of MIGRATIONS) {
      const { rows } = await client.query(
        'SELECT 1 FROM _migrations WHERE name = $1',
        [m.name],
      );
      if (rows.length > 0) {
        console.log(`  ✓ ${m.name} (already applied)`);
        continue;
      }

      await client.query('BEGIN');
      try {
        await client.query(m.sql);
        await client.query(
          'INSERT INTO _migrations (name) VALUES ($1)',
          [m.name],
        );
        await client.query('COMMIT');
        console.log(`  ✓ ${m.name} (applied)`);
      } catch (err) {
        await client.query('ROLLBACK');
        console.error(`  ✗ ${m.name} FAILED:`, err.message);
        process.exit(1);
      }
    }

    console.log('\nAll migrations complete.');
  } finally {
    client.release();
    await pool.end();
  }
}

migrate().catch((err) => {
  console.error('Migration failed:', err.message);
  process.exit(1);
});
