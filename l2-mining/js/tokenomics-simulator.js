#!/usr/bin/env node
/**
 * TGBT Tokenomics Simulator
 * 
 * Models emission schedules, miner rewards, multi-miner scaling,
 * and halving behaviour on Arbitrum L2 (0.25s blocks).
 *
 * Usage:  node js/tokenomics-simulator.js
 */

// ═══════════════════════════════════════════════════════════════════
//  CONTRACT CONSTANTS  (must match Solidity)
// ═══════════════════════════════════════════════════════════════════
const TOTAL_SUPPLY_CAP     = 2_000_000_000;   // 2 B TGBT
const MINING_ALLOCATION    = 700_000_000;      // 700 M TGBT
const STALE_BLOCK_ALLOC    = 25_000_000;       // 25 M  TGBT
const REWARD_PER_SOLUTION  = 1.375;            // BatchMiningModule
const INITIAL_REWARD       = 10;               // 10 TGBT base reward per reveal
const REDUCTION_NUMERATOR  = 65;               // each "halving" → reward × 65/100
const REDUCTION_DENOMINATOR = 100;
const MIN_REWARD           = 1e-12;            // 1e6 wei in TGBT  (effectively 0)
const BONUS_MULTIPLIER     = 1.25;             // 125% if difficulty > 2× target

// ═══════════════════════════════════════════════════════════════════
//  ARBITRUM L2
// ═══════════════════════════════════════════════════════════════════
const BLOCK_TIME_SECS      = 0.25;
const BLOCKS_PER_DAY       = 86_400 / BLOCK_TIME_SECS;          // 345,600
const BLOCKS_PER_YEAR      = BLOCKS_PER_DAY * 365.25;           // ~126,144,000

// ═══════════════════════════════════════════════════════════════════
//  MINING THROUGHPUT LIMITS
// ═══════════════════════════════════════════════════════════════════
const MIN_COMMITMENT_AGE   = 2;         // blocks before reveal
const SUBMIT_COST          = 1;         // rate-limit tokens
const REVEAL_COST          = 2;         // rate-limit tokens
const RATE_LIMIT_CAPACITY  = 60;        // tokens
// Token-bucket refill not modelled exactly → assume sustainable rate

// Minimum cycle: commit + wait 2 blocks + reveal = ~3 blocks per solution
const BLOCKS_PER_CYCLE     = 3;
const SECS_PER_CYCLE       = BLOCKS_PER_CYCLE * BLOCK_TIME_SECS;  // 0.75s
// Rate-limit sustainable cycles: 60 / 3 = 20 bursts, then refill
// Conservative estimate: 1 reveal per 1 second sustained (~4 blocks)
const SUSTAINABLE_CYCLES_PER_DAY_PER_MINER = 86_400 / 1; // 86,400 max

// ═══════════════════════════════════════════════════════════════════
//  HELPERS
// ═══════════════════════════════════════════════════════════════════
function applyReductions(reward, intervals) {
    let r = reward;
    for (let i = 0; i < Math.min(intervals, 100); i++) {
        r = (r * REDUCTION_NUMERATOR) / REDUCTION_DENOMINATOR;
        if (r < MIN_REWARD) return MIN_REWARD;
    }
    return r;
}

function fmt(n, dec = 2) {
    if (n >= 1e9) return (n / 1e9).toFixed(dec) + ' B';
    if (n >= 1e6) return (n / 1e6).toFixed(dec) + ' M';
    if (n >= 1e3) return (n / 1e3).toFixed(dec) + ' K';
    return n.toFixed(dec);
}

function banner(title) {
    console.log('\n' + '═'.repeat(70));
    console.log('  ' + title);
    console.log('═'.repeat(70));
}

// ═══════════════════════════════════════════════════════════════════
//  1.  HALVING SCHEDULE COMPARISON
// ═══════════════════════════════════════════════════════════════════
function simulateHalvingSchedule(halvingIntervalBlocks, label) {
    banner(`HALVING SCHEDULE — ${label}`);

    const halvingIntervalDays = (halvingIntervalBlocks * BLOCK_TIME_SECS) / 86_400;
    const halvingIntervalYears = halvingIntervalDays / 365.25;
    console.log(`  Halving interval: ${fmt(halvingIntervalBlocks, 0)} blocks = ${halvingIntervalDays.toFixed(1)} days = ${halvingIntervalYears.toFixed(2)} years`);
    console.log(`  Reduction per halving: ${100 - REDUCTION_NUMERATOR}% (reward × 0.${REDUCTION_NUMERATOR})`);
    console.log('');

    let reward = INITIAL_REWARD;
    let totalMined = 0;
    const years = 20;
    const totalBlocks = years * BLOCKS_PER_YEAR;

    console.log('  Halving  │  Year   │  Block Reward  │  Cumulative Mined    │  % of 700M Alloc');
    console.log('  ─────────┼─────────┼────────────────┼──────────────────────┼──────────────────');

    let halvingCount = 0;
    let currentBlock = 0;

    for (let h = 0; h <= 30; h++) {
        const nextHalvingBlock = (h + 1) * halvingIntervalBlocks;
        const blocksThisInterval = Math.min(halvingIntervalBlocks, totalBlocks - currentBlock);
        if (blocksThisInterval <= 0) break;

        // Assume 1 successful mine per block on average (theoretical max throughput)
        // In reality, only a fraction of blocks have a mining reveal
        totalMined += reward * blocksThisInterval;
        if (totalMined > MINING_ALLOCATION) totalMined = MINING_ALLOCATION;

        const yearSoFar = (nextHalvingBlock * BLOCK_TIME_SECS) / (365.25 * 86_400);
        const pct = (totalMined / MINING_ALLOCATION * 100);

        console.log(`  ${String(h).padStart(7)}  │  ${yearSoFar.toFixed(2).padStart(5)}  │  ${reward.toFixed(4).padStart(12)} TGBT  │  ${fmt(totalMined).padStart(18)}  │  ${pct.toFixed(2).padStart(6)}%`);

        currentBlock = nextHalvingBlock;
        if (currentBlock >= totalBlocks || totalMined >= MINING_ALLOCATION) break;

        reward = applyReductions(INITIAL_REWARD, h + 1);
        halvingCount = h + 1;
    }

    console.log(`\n  Total halvings in 20 years: ${halvingCount}`);
    console.log(`  Final block reward: ${reward.toFixed(6)} TGBT`);
    console.log(`  Total mined (capped at allocation): ${fmt(totalMined)} TGBT`);
}

// ═══════════════════════════════════════════════════════════════════
//  2.  MINER ECONOMICS
// ═══════════════════════════════════════════════════════════════════
function simulateMinerEconomics() {
    banner('MINER ECONOMICS — How much can a single miner earn?');

    // Conservative: 1 solution per second sustained (rate-limit + commit age)
    const solutionsPerDay = SUSTAINABLE_CYCLES_PER_DAY_PER_MINER;
    const basePerDay = solutionsPerDay * INITIAL_REWARD;
    const bonusPerDay = solutionsPerDay * INITIAL_REWARD * BONUS_MULTIPLIER;

    console.log(`  Mining throughput (sustained): ~${fmt(solutionsPerDay, 0)} solutions/day`);
    console.log(`  (limited by rate-limit token bucket + ${MIN_COMMITMENT_AGE}-block commit age)\n`);

    // But in reality, not every reveal wins at target difficulty
    // assume 100% success rate for theoretical max, then show scaled

    console.log('  ┌────────────────────────────────────────────────────────────────┐');
    console.log('  │  Scenario                    │  Daily TGBT   │  Monthly TGBT  │');
    console.log('  ├────────────────────────────────────────────────────────────────┤');

    const scenarios = [
        ['Theoretical max (1/sec)',          solutionsPerDay, 1.0],
        ['Rate-limited realistic (1/2 sec)', solutionsPerDay * 0.5, 1.0],
        ['After 1st halving (×0.65)',        solutionsPerDay * 0.5, 0.65],
        ['After 2nd halving (×0.4225)',      solutionsPerDay * 0.5, 0.4225],
        ['Bonus solutions only (1.25×)',     solutionsPerDay * 0.1, BONUS_MULTIPLIER],
    ];

    for (const [name, solPerDay, multiplier] of scenarios) {
        const daily = solPerDay * INITIAL_REWARD * multiplier;
        const monthly = daily * 30;
        console.log(`  │  ${name.padEnd(29)} │  ${fmt(daily).padStart(11)}  │  ${fmt(monthly).padStart(12)}  │`);
    }

    console.log('  └────────────────────────────────────────────────────────────────┘');
}

// ═══════════════════════════════════════════════════════════════════
//  3.  MULTI-MINER SCALING
// ═══════════════════════════════════════════════════════════════════
function simulateMultiMiner() {
    banner('MULTI-MINER SCALING — How rewards change as miners join');

    console.log('  Key insight: Each miner gets the SAME base reward per successful reveal.');
    console.log('  More miners = MORE total emission (until pool bucket drains).');
    console.log('  The pool emission bucket is the hard cap, NOT per-miner.\n');

    const poolEmissionBucket = 700_000_000; // default pool 0
    const rate = 86_400 * 0.5; // 0.5 solutions/sec/miner (realistic)

    console.log('  Miners │  Daily Total Emission  │  Pool Exhaustion Time  │  Per-Miner Daily');
    console.log('  ───────┼────────────────────────┼────────────────────────┼──────────────────');

    for (const minerCount of [1, 5, 10, 50, 100, 500, 1000]) {
        const dailyPerMiner = rate * INITIAL_REWARD;
        const dailyTotal = dailyPerMiner * minerCount;
        const daysToExhaust = poolEmissionBucket / dailyTotal;

        console.log(`  ${String(minerCount).padStart(5)}  │  ${fmt(dailyTotal).padStart(18)} TGBT  │  ${daysToExhaust.toFixed(1).padStart(14)} days    │  ${fmt(dailyPerMiner).padStart(10)} TGBT`);
    }

    console.log('\n  ⚠  With 1000 miners at full speed, pool 0 would drain in ~1.6 days!');
    console.log('     This is why multiple pools with separate emission buckets exist.');
    console.log('     The halving schedule also reduces rewards over time.\n');

    // Show the effect: What if initial emission bucket = 100M per pool?
    console.log('  With a more conservative pool bucket (100M TGBT per pool):');
    console.log('  Miners │  Pool Exhaust Time  │  Note');
    console.log('  ───────┼─────────────────────┼───────────────────────');
    for (const mc of [1, 10, 100, 1000]) {
        const dpe = (rate * INITIAL_REWARD * mc);
        const days = 100_000_000 / dpe;
        const note = days < 30 ? '⚠ needs more pools' : days < 365 ? 'manageable' : '✓ plenty';
        console.log(`  ${String(mc).padStart(5)}  │  ${days.toFixed(1).padStart(13)} days    │  ${note}`);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  4.  FULL EMISSION PROJECTION (year by year)
// ═══════════════════════════════════════════════════════════════════
function simulateEmissionProjection(halvingIntervalBlocks, activeMiners, label) {
    banner(`EMISSION PROJECTION — ${label}`);
    console.log(`  Active miners: ${activeMiners}, each doing ~43,200 reveals/day (0.5/sec)`);
    console.log(`  Halving every ${fmt(halvingIntervalBlocks, 0)} blocks`);
    console.log('');

    let reward = INITIAL_REWARD;
    let totalMined = 0;
    const revsPerDayPerMiner = 43_200;

    console.log('  Year │  Block Reward  │  Daily Emission   │  Yearly Emission  │  Cumulative   │  % Alloc');
    console.log('  ─────┼────────────────┼───────────────────┼───────────────────┼───────────────┼──────────');

    for (let year = 1; year <= 20; year++) {
        // Calculate how many halvings have happened by end of this year
        const blocksAtYearEnd = year * BLOCKS_PER_YEAR;
        const halvings = Math.floor(blocksAtYearEnd / halvingIntervalBlocks);
        reward = applyReductions(INITIAL_REWARD, halvings);

        const dailyEmission = reward * revsPerDayPerMiner * activeMiners;
        const yearlyEmission = Math.min(dailyEmission * 365.25, MINING_ALLOCATION - totalMined);
        if (yearlyEmission <= 0) {
            console.log(`  ${String(year).padStart(4)}  │  ${'ALLOCATION EXHAUSTED'.padStart(12)}  │`);
            break;
        }
        totalMined += yearlyEmission;
        if (totalMined > MINING_ALLOCATION) totalMined = MINING_ALLOCATION;

        const pct = (totalMined / MINING_ALLOCATION * 100);
        console.log(`  ${String(year).padStart(4)}  │  ${reward.toFixed(4).padStart(12)} TGBT  │  ${fmt(dailyEmission).padStart(13)} TGBT  │  ${fmt(yearlyEmission).padStart(13)} TGBT  │  ${fmt(totalMined).padStart(9)} TGBT  │  ${pct.toFixed(2).padStart(6)}%`);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  5.  HALVING INTERVAL ANALYSIS (Critical finding!)
// ═══════════════════════════════════════════════════════════════════
function analyzeHalvingConstraint() {
    banner('⚠  CRITICAL: MAX_HALVING_INTERVAL CONSTRAINT');

    const currentMax = 15_000_000;
    const currentMaxDays = (currentMax * BLOCK_TIME_SECS) / 86_400;
    const currentMaxYears = currentMaxDays / 365.25;

    console.log(`  Current MAX_HALVING_INTERVAL in TokenomicsLib.sol: ${fmt(currentMax, 0)} blocks`);
    console.log(`  On Arbitrum (0.25s blocks): ${currentMaxDays.toFixed(1)} days = ${currentMaxYears.toFixed(3)} years`);
    console.log('');
    console.log('  ❌ This means the LONGEST possible halving cycle is ~43 days!');
    console.log('     A Bitcoin-like 4-year halving requires ~504,576,000 blocks.');
    console.log('     A 2-year halving requires ~252,288,000 blocks.');
    console.log('     Even a 1-year halving requires ~126,144,000 blocks.');
    console.log('');
    console.log('  ┌──────────────────────────────────────────────────────────────────────┐');
    console.log('  │  RECOMMENDED FIX: Change MAX_HALVING_INTERVAL in TokenomicsLib.sol   │');
    console.log('  │                                                                      │');
    console.log('  │  Current:  uint256 private constant MAX_HALVING_INTERVAL = 15_000_000│');
    console.log('  │  Needed:   uint256 private constant MAX_HALVING_INTERVAL = 630_720_000│');
    console.log('  │           (~5 years on Arbitrum, leaves room for flexibility)         │');
    console.log('  └──────────────────────────────────────────────────────────────────────┘');
    console.log('');

    // Comparison table
    console.log('  Halving Period │  Interval (blocks)  │  Halvings in 20yr  │  Year-20 Reward  │  Total Mined (1 miner)');
    console.log('  ───────────────┼─────────────────────┼────────────────────┼──────────────────┼───────────────────────');

    const intervals = [
        ['~43 days (current max)', currentMax],
        ['6 months',               Math.round(BLOCKS_PER_YEAR / 2)],
        ['1 year',                 Math.round(BLOCKS_PER_YEAR)],
        ['2 years',                Math.round(BLOCKS_PER_YEAR * 2)],
        ['4 years (Bitcoin-like)', Math.round(BLOCKS_PER_YEAR * 4)],
        ['5 years',                Math.round(BLOCKS_PER_YEAR * 5)],
    ];

    for (const [label, interval] of intervals) {
        const halvingsIn20yr = Math.floor(20 * BLOCKS_PER_YEAR / interval);
        const yr20reward = applyReductions(INITIAL_REWARD, halvingsIn20yr);
        // 1 miner, 43200 reveals/day, simple model
        let mined = 0;
        let r = INITIAL_REWARD;
        for (let y = 1; y <= 20; y++) {
            const h = Math.floor(y * BLOCKS_PER_YEAR / interval);
            r = applyReductions(INITIAL_REWARD, h);
            const yearly = Math.min(r * 43_200 * 365.25, MINING_ALLOCATION - mined);
            if (yearly <= 0) break;
            mined += yearly;
        }
        if (mined > MINING_ALLOCATION) mined = MINING_ALLOCATION;

        console.log(`  ${label.padEnd(23)} │  ${fmt(interval, 0).padStart(17)}  │  ${String(halvingsIn20yr).padStart(16)}  │  ${yr20reward.toFixed(4).padStart(10)} TGBT    │  ${fmt(mined).padStart(15)} TGBT`);
    }
}

// ═══════════════════════════════════════════════════════════════════
//  6.  WALLET & REWARD FLOW
// ═══════════════════════════════════════════════════════════════════
function explainMinerWalletFlow() {
    banner('MINER WALLET & REWARD FLOW');

    console.log(`
  ✅ YES — Any wallet can mine and receive TGBT rewards. No registration needed.

  How it works:

  1. CONNECT WALLET
     └─ Miner calls submitMiningCommitment() from their own wallet (msg.sender)
     └─ No hold requirement (removed in commit 0ae4f2c)
     └─ No KYC / whitelist — fully permissionless

  2. COMMIT-REVEAL CYCLE
     └─ Submit commitment → wait ≥ ${MIN_COMMITMENT_AGE} blocks (0.5s) → reveal
     └─ Miner's wallet signs EIP-712 typed data (replay-proof)
     └─ On successful reveal: reward mints DIRECTLY to miner's wallet

  3. REWARD MINTING
     └─ TokenomicsModule.onBlockMined(miner, ...) → tgbtToken.mint(miner, reward)
     └─ TGBT appears in miner's wallet immediately (same tx)
     └─ No claim step, no staking, no lock-up

  4. BATCH MINING (alternative path)
     └─ BatchMiningModule: submit Merkle root of many solutions
     └─ REWARD_PER_SOLUTION = ${REWARD_PER_SOLUTION} TGBT per valid leaf
     └─ Also mints directly to the operator's wallet on finalization

  5. STALE BLOCK REWARDS
     └─ StaleBlockOracle: submit orphaned Bitcoin block headers
     └─ Separate allocation pool: ${fmt(STALE_BLOCK_ALLOC)} TGBT
     └─ Claim-based: submitStaleBlock() → claimReward()

  Summary: Plug in wallet → mine → TGBT appears. Like Bitcoin.
`);
}

// ═══════════════════════════════════════════════════════════════════
//  7.  DEPLOY PARAMETER RECOMMENDATIONS
// ═══════════════════════════════════════════════════════════════════
function recommendDeployParams() {
    banner('RECOMMENDED MAINNET DEPLOY PARAMETERS');

    // 2-year halving on Arbitrum
    const halvingBlocks = Math.round(BLOCKS_PER_YEAR * 2); // ~252,288,000

    console.log(`
  Token:
    TOTAL_SUPPLY_CAP      = 2,000,000,000 TGBT           ✓ (hardcoded)
    MINING_ALLOCATION     = 700,000,000 TGBT (35%)        ✓ (hardcoded)
    STALE_BLOCK_ALLOC     = 25,000,000 TGBT (1.25%)       ✓ (hardcoded)
    Remaining             = 1,275,000,000 TGBT (63.75%)    (treasury/team/etc)

  TokenomicsModule.initialize():
    initialReward         = 10 TGBT per block              ✓ (current default)
    blocksPerEpoch        = 345,600                        (= 1 day on Arbitrum)
    halvingInterval       = ${fmt(halvingBlocks, 0)} blocks          (= 2 years on Arbitrum)
    bonusThreshold        = 2                              ✓ (2× difficulty for bonus)
    bonusMultiplier       = 125                            ✓ (1.25× reward)

  MiningModule.initialize():
    initialDifficulty     = 1,000 – 10,000                 (tune for launch hashrate)
    initialEmission       = 350,000,000 TGBT               (half of mining alloc per pool)

  ⚠  REQUIRED CONTRACT CHANGE:
    TokenomicsLib.MAX_HALVING_INTERVAL must be increased from 15,000,000
    to at least 630,720,000 to support multi-year halving on Arbitrum.
`);
}

// ═══════════════════════════════════════════════════════════════════
//  RUN ALL SIMULATIONS
// ═══════════════════════════════════════════════════════════════════
console.log('\n' + '╔' + '═'.repeat(68) + '╗');
console.log('║' + '  TGBT TOKENOMICS SIMULATION'.padEnd(68) + '║');
console.log('║' + '  Arbitrum L2 · 0.25s blocks · 35% halving reduction'.padEnd(68) + '║');
console.log('╚' + '═'.repeat(68) + '╝');

// Critical issue first
analyzeHalvingConstraint();

// Halving schedules
simulateHalvingSchedule(Math.round(BLOCKS_PER_YEAR * 2), '2-YEAR HALVING (recommended)');
simulateHalvingSchedule(Math.round(BLOCKS_PER_YEAR * 4), '4-YEAR HALVING (Bitcoin-like)');
simulateHalvingSchedule(15_000_000, '~43-DAY HALVING (current MAX_HALVING_INTERVAL)');

// Miner economics
simulateMinerEconomics();
simulateMultiMiner();

// Full emission projections
simulateEmissionProjection(Math.round(BLOCKS_PER_YEAR * 2), 1, '1 miner, 2-year halving');
simulateEmissionProjection(Math.round(BLOCKS_PER_YEAR * 2), 10, '10 miners, 2-year halving');
simulateEmissionProjection(Math.round(BLOCKS_PER_YEAR * 2), 100, '100 miners, 2-year halving');

// Wallet flow
explainMinerWalletFlow();

// Deploy recommendations
recommendDeployParams();
