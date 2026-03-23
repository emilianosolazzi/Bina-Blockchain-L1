#!/usr/bin/env node
/**
 * diagnose-miner.js — On-chain diagnostic for the TGBT miner.
 *
 * Queries the MiningModule and RateLimitModule contracts to show
 * exactly why the reveal phase failed.
 *
 * Usage: node scripts/diagnose-miner.js
 */

const ethers = require('ethers');

// ─── Config ──────────────────────────────────────────────────
const RPC_URL         = 'https://arb1.arbitrum.io/rpc';
const MINING_MODULE   = '0x56C458a06FB104cb31820856fCe42E1f6926CBDD';
const CORE_ADDRESS    = '0xF6556DDC7CdD3635A05428BD85BCf33A09F752e6';
const WALLET_ADDRESS  = '0x5cB4D906f0464b34c44d6555A770BF6aF4A2cEfe';
const POOL_ID         = 3;
const ZERO_HASH       = '0x0000000000000000000000000000000000000000000000000000000000000000';

// ─── ABIs ────────────────────────────────────────────────────
const MINING_ABI = [
	'function minerCommitments(address) view returns (bytes32 commitHash, uint64 timestamp, bool revealed, bool validated, bool revoked, bool emergency, bytes32 revealedValue, uint8 poolId)',
	'function minCommitmentAge() view returns (uint256)',
	'function maxCommitmentAge() view returns (uint256)',
	'function minBlockInterval() view returns (uint8)',
	'function lastMinerBlock(address) view returns (uint64)',
	'function nonces(address) view returns (uint256)',
	'function getPoolInfo(uint8 poolId) view returns (uint256 difficulty, uint256 emission, uint256 mined, bool active)',
	'function getMiningChallenge(uint8 poolId) view returns (bytes32[] outputs, uint256 difficulty)',
	'function usedOutputs(bytes32) view returns (uint256)',
];

const CORE_ABI = [
	'function moduleAddress(bytes32 moduleId) view returns (address)',
	'function getOutputHistory() view returns (bytes32[32])',
	'function systemActive() view returns (bool)',
];

const RATE_ABI = [
	'function getUserCapacity(address) view returns (uint256 currentTokens, uint256 capacity)',
	'function getRateStatistics() view returns (uint256 currentRate, uint256 averageRate, uint256 peakRate, uint16 rateBps, bool isWarning, bool isCritical)',
];

// ─── Main ────────────────────────────────────────────────────
async function main() {
	const provider = new ethers.providers.JsonRpcProvider(RPC_URL);
	const mining   = new ethers.Contract(MINING_MODULE, MINING_ABI, provider);
	const core     = new ethers.Contract(CORE_ADDRESS, CORE_ABI,   provider);

	console.log('=== TGBT Miner Diagnostic ===\n');
	console.log(`Wallet:   ${WALLET_ADDRESS}`);
	console.log(`Mining:   ${MINING_MODULE}`);
	console.log(`Pool:     ${POOL_ID}`);
	console.log('');

	// 1. System active?
	let systemActive = true;
	try {
		systemActive = await core.systemActive();
		console.log(`System Active: ${systemActive}`);
	} catch (e) {
		console.log(`System Active: UNKNOWN (${e.message.slice(0, 80)})`);
	}

	// 2. Current block numbers
	const l2Block = await provider.getBlockNumber();
	const latestBlock = await provider.getBlock('latest');
	let l1Block = l2Block;
	try {
		const raw = await provider.send('eth_getBlockByNumber', ['latest', false]);
		if (raw.l1BlockNumber) {
			l1Block = parseInt(raw.l1BlockNumber, 16);
		}
	} catch (e) {
		console.log(`  (l1BlockNumber unavailable: ${e.message.slice(0, 60)})`);
	}
	console.log(`\nBlock Numbers:`);
	console.log(`  L2 sequence:       ${l2Block}`);
	console.log(`  L1 (contract ref): ${l1Block}`);
	console.log(`  Timestamp:         ${latestBlock.timestamp} (${new Date(latestBlock.timestamp * 1000).toISOString()})`);

	// 3. On-chain commitment
	console.log(`\n─── Miner Commitment ───`);
	const c = await mining.minerCommitments(WALLET_ADDRESS);
	const commitHash   = c.commitHash;
	const commitBlock  = Number(c.timestamp);
	const revealed     = c.revealed;
	const revealedVal  = c.revealedValue;
	const onchainPool  = Number(c.poolId);

	const isZero = commitHash === ZERO_HASH;
	console.log(`  commitHash:    ${commitHash}${isZero ? '  (EMPTY — no active commitment)' : ''}`);
	console.log(`  commitBlock:   ${commitBlock}`);
	console.log(`  revealed:      ${revealed}`);
	console.log(`  revealedValue: ${revealedVal}`);
	console.log(`  poolId:        ${onchainPool}`);

	// 4. Commitment timing
	const minAge = Number(await mining.minCommitmentAge());
	const maxAge = Number(await mining.maxCommitmentAge());
	console.log(`\n─── Commitment Window ───`);
	console.log(`  minCommitmentAge: ${minAge} blocks`);
	console.log(`  maxCommitmentAge: ${maxAge} blocks`);

	if (!isZero && commitBlock > 0) {
		const revealOpensAt = commitBlock + minAge;
		const expiresAt     = commitBlock + maxAge;
		const blocksUntilOpen    = revealOpensAt > l1Block ? revealOpensAt - l1Block : 0;
		const blocksUntilExpires = expiresAt > l1Block ? expiresAt - l1Block : 0;
		const isInWindow   = l1Block >= revealOpensAt && l1Block <= expiresAt;
		const isExpired    = l1Block > expiresAt;
		const isTooEarly   = l1Block < revealOpensAt;

		console.log(`  Reveal window:    L1 block ${revealOpensAt} → ${expiresAt}`);
		console.log(`  Current L1:       ${l1Block}`);
		console.log(`  Status:           ${isExpired ? '❌ EXPIRED' : isTooEarly ? '⏳ TOO EARLY ('+blocksUntilOpen+' blocks to go)' : '✅ IN WINDOW ('+blocksUntilExpires+' blocks remaining)'}`);

		if (revealed) {
			console.log(`  ✅ Already revealed — this commitment is done`);
		}
	}

	// 5. Mining cooldown
	console.log(`\n─── Mining Cooldown ───`);
	const minInterval = Number(await mining.minBlockInterval());
	const lastBlock   = Number(await mining.lastMinerBlock(WALLET_ADDRESS));
	const nextAllowed = lastBlock + minInterval;
	const nonce       = Number(await mining.nonces(WALLET_ADDRESS));
	console.log(`  minBlockInterval:  ${minInterval}`);
	console.log(`  lastMinerBlock:    ${lastBlock}`);
	console.log(`  nextAllowed:       ${nextAllowed}`);
	console.log(`  current L1:        ${l1Block}`);
	console.log(`  cooldown clear:    ${l1Block >= nextAllowed ? '✅ YES' : '❌ NO — ' + (nextAllowed - l1Block) + ' blocks to go'}`);
	console.log(`  contractNonce:     ${nonce}`);

	// 6. Pool info
	console.log(`\n─── Pool ${POOL_ID} ───`);
	const pool = await mining.getPoolInfo(POOL_ID);
	const remaining = parseFloat(ethers.utils.formatUnits(pool.emission, 18));
	const mined     = parseFloat(ethers.utils.formatUnits(pool.mined, 18));
	console.log(`  difficulty:   ${pool.difficulty}`);
	console.log(`  emission:     ${remaining.toFixed(2)} TGBT remaining`);
	console.log(`  totalMined:   ${mined.toFixed(2)} TGBT`);
	console.log(`  active:       ${pool.active}`);

	// 7. Output history check
	console.log(`\n─── Output History ───`);
	try {
		const history = await core.getOutputHistory();
		const nonZero = history.filter(h => h !== ZERO_HASH);
		console.log(`  History entries:   ${nonZero.length}/32`);
		if (nonZero.length > 0) {
			console.log(`  Latest outputs:`);
			for (let i = 0; i < Math.min(5, nonZero.length); i++) {
				console.log(`    [${i}] ${nonZero[i]}`);
			}
		}
	} catch (e) {
		console.log(`  ⚠ Could not read output history: ${e.message.slice(0, 80)}`);
	}

	// 8. Rate limiter
	console.log(`\n─── Rate Limiter ───`);
	try {
		const rateLimitAddr = await core.moduleAddress(ethers.utils.id('RATE_LIMIT_MODULE'));
		console.log(`  Module address: ${rateLimitAddr}`);
		if (rateLimitAddr !== ethers.constants.AddressZero) {
			const rateLimit = new ethers.Contract(rateLimitAddr, RATE_ABI, provider);
			const [tokens, capacity] = await rateLimit.getUserCapacity(WALLET_ADDRESS);
			const stats = await rateLimit.getRateStatistics();
			console.log(`  User tokens:   ${tokens} / ${capacity}`);
			console.log(`  Global rate:   ${stats.currentRate} ops (avg ${stats.averageRate}, peak ${stats.peakRate})`);
			console.log(`  rateBps:       ${stats.rateBps}`);
			console.log(`  warning:       ${stats.isWarning}`);
			console.log(`  critical:      ${stats.isCritical}`);
			console.log(`  Rate limit ok: ${Number(tokens) >= 2 ? '✅ YES (tokens >= reveal cost 2)' : '❌ NO — only ' + tokens + ' tokens (need 2)'}`);
		} else {
			console.log(`  ⚠ No rate limit module registered`);
		}
	} catch (e) {
		console.log(`  ⚠ Could not read rate limiter: ${e.message.slice(0, 120)}`);
	}

	// 9. Simulate what the Rust miner sees via get_onchain_commitment (raw calldata)
	console.log(`\n─── Raw ABI Decode (minerCommitments) ───`);
	try {
		const selector = ethers.utils.id('minerCommitments(address)').slice(0, 10);
		const encoded  = ethers.utils.defaultAbiCoder.encode(['address'], [WALLET_ADDRESS]);
		const calldata = selector + encoded.slice(2);
		const result   = await provider.call({ to: MINING_MODULE, data: calldata });
		console.log(`  Raw return (${result.length} hex chars = ${(result.length - 2) / 2} bytes):`);
		// Decode 8 words of 32 bytes each
		for (let i = 0; i < 8; i++) {
			const word = '0x' + result.slice(2 + i * 64, 2 + (i + 1) * 64);
			const label = ['commitHash', 'timestamp ', 'revealed  ', 'validated ', 'revoked   ', 'emergency ', 'revealVal ', 'poolId    '][i];
			const num = BigInt(word);
			console.log(`    word[${i}] ${label}: ${word}  (${num})`);
		}
	} catch (e) {
		console.log(`  Error: ${e.message.slice(0, 100)}`);
	}

	// 10. Summary / Diagnosis
	console.log('\n════════════════════════════════');
	console.log('        DIAGNOSIS SUMMARY');
	console.log('════════════════════════════════');

	const problems = [];

	if (!systemActive) problems.push('🔴 System is paused (whenSystemActive would revert)');

	if (isZero) {
		problems.push('🟡 No active commitment on-chain — commitment may have been cleared/expired');
	} else if (revealed) {
		problems.push('🟢 Commitment was already revealed successfully');
	} else {
		const expiresAt = commitBlock + maxAge;
		if (l1Block > expiresAt) {
			problems.push(`🟡 Commitment EXPIRED at L1 block ${expiresAt} (current: ${l1Block}) — must wait for clearance then commit again`);
		} else if (l1Block < commitBlock + minAge) {
			problems.push(`⏳ Commitment is still too recent — reveal opens at L1 block ${commitBlock + minAge}`);
		} else {
			problems.push(`🟢 Commitment is in the reveal window — miner should be revealing now`);
		}
	}

	if (onchainPool !== POOL_ID && !isZero) {
		problems.push(`🔴 POOL MISMATCH: on-chain poolId=${onchainPool} but miner configured for pool ${POOL_ID}`);
	}

	if (!pool.active) problems.push('🔴 Pool is not active');
	if (remaining <= 0) problems.push('🔴 Pool has zero remaining emission');

	if (l1Block < nextAllowed) {
		problems.push(`⏳ Mining cooldown active — ${nextAllowed - l1Block} blocks remaining`);
	}

	if (problems.length === 0) {
		console.log('  ✅ No obvious issues detected');
	} else {
		for (const p of problems) {
			console.log(`  ${p}`);
		}
	}

	console.log('');
}

main().catch(err => {
	console.error('Diagnostic failed:', err.message);
	process.exit(1);
});
