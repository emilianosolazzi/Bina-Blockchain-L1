#!/usr/bin/env node
// scripts/generate-jwt-secret.js
// Generates a cryptographically secure JWT secret and prints it.

import crypto from 'crypto';

const secret = crypto.randomBytes(64).toString('hex');
console.log('Generated JWT secret (add to .env as JWT_SECRET):');
console.log(secret);
