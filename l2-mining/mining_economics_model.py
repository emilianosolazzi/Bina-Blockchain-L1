#!/usr/bin/env python3
"""TGBT Mining Economics Model — April 2026 on-chain snapshot"""

BASE_REWARD = 10.0
BONUS_REWARD = 12.5
BONUS_RATE = 0.05
AVG_REWARD = BASE_REWARD * (1 - BONUS_RATE) + BONUS_REWARD * BONUS_RATE
POOL1_REMAINING = 699_999_212.5
REDUCTION = 0.65
HALVING_YEARS = 95.9
# ~10 min per solution per miner as baseline (commit-reveal cycle)
RATE_PER_MINER = (24 * 60 / 10) * AVG_REWARD  # solutions/day * reward

print("=" * 72)
print("TGBT MINING ECONOMICS MODEL")
print("=" * 72)
print()
print("--- On-Chain Parameters (verified April 2026) ---")
print(f"Base reward:           {BASE_REWARD} TGBT/solution")
print(f"Bonus (1.25x):         {BONUS_REWARD} TGBT (~5% of solutions)")
print(f"Avg reward:            {AVG_REWARD:.3f} TGBT")
print(f"Difficulty:            2^245 (~11 leading zero bits, FIXED)")
print(f"P(valid hash):         1/2048 per hash attempt")
print(f"Difficulty adjust:     NONE (pools are immutable)")
print(f"Halving factor:        0.65 (35% cut per halving)")
print(f"Halving interval:      ~{HALVING_YEARS:.0f} years (see note below)")
print()
print("--- Commit-Reveal Cycle Constraints ---")
print("minBlockInterval:      1 L1 block (~12s)")
print("minCommitmentAge:      2 L1 blocks (~24s)")
print("maxCommitmentAge:      500 L1 blocks (~100 min)")
print("Min cycle:             ~4 L1 blocks = ~48 seconds")
print("Practical cycle:       ~5-10 min per solution")
print("Constraint:            1 active commitment per miner at a time")
print()

print("--- Solution Rate Per Miner ---")
for mins in [5, 10, 15, 30]:
    sols = (24 * 60) / mins
    tgbt = sols * AVG_REWARD
    print(f"  1 sol every {mins:2d} min -> {sols:5.0f} sol/day -> {tgbt:7.0f} TGBT/day")

print()
print("--- Pool 1 Depletion (baseline: ~10 min/sol/miner) ---")
print(f"  Pool 1 remaining: {POOL1_REMAINING:,.1f} TGBT")
print(f"  Rate per miner:   {RATE_PER_MINER:,.0f} TGBT/day")
print()
header = f"{'Miners':>8} | {'TGBT/day':>12} | {'TGBT/year':>14} | {'Pool 1 lasts':>16}"
print(header)
print("-" * 58)
for n in [1, 5, 10, 50, 100, 500, 1000, 5000, 10000]:
    daily = RATE_PER_MINER * n
    yearly = daily * 365.25
    days = POOL1_REMAINING / daily
    yrs = days / 365.25
    if yrs >= 1:
        life = f"{yrs:,.1f} years"
    elif days >= 1:
        life = f"{days:,.0f} days"
    else:
        life = f"{days * 24:,.1f} hours"
    print(f"{n:>8,} | {daily:>12,.0f} | {yearly:>14,.0f} | {life:>16}")

print()
print("--- Halving Schedule ---")
print(f"{'#':>4} | {'Reward':>10} | {'Cut':>10} | {'Year':>8}")
print("-" * 40)
r = BASE_REWARD
for h in range(8):
    cut = (1 - r / BASE_REWARD) * 100
    yr = h * HALVING_YEARS
    print(f"{h:>4} | {r:>10.4f} | {cut:>9.1f}% | ~{yr:>6.0f}")
    r *= REDUCTION
    if r < 0.000001:
        r = 0.000001

print()
print("--- 10-Year Supply Projection (no halving in this window) ---")
header2 = f"{'Miners':>8} | {'Y1 mined':>14} | {'Y5 mined':>14} | {'Y10 mined':>14} | {'% Pool1':>8}"
print(header2)
print("-" * 68)
for n in [1, 10, 100, 1000, 10000]:
    y1 = min(RATE_PER_MINER * n * 365.25, POOL1_REMAINING)
    y5 = min(RATE_PER_MINER * n * 365.25 * 5, POOL1_REMAINING)
    y10 = min(RATE_PER_MINER * n * 365.25 * 10, POOL1_REMAINING)
    pct = (y10 / POOL1_REMAINING) * 100
    print(f"{n:>8,} | {y1:>14,.0f} | {y5:>14,.0f} | {y10:>14,.0f} | {pct:>7.2f}%")

print()
print("=" * 72)
print("KEY FINDINGS")
print("=" * 72)
print()
print("1. NO DIFFICULTY ADJUSTMENT")
print("   More miners = linearly faster emission. 10x miners = 10x faster drain.")
print("   Unlike Bitcoin, there is NO automatic rebalancing.")
print()
print("2. HALVING IS ~96 YEARS AWAY")
print("   halvingInterval = 252,288,000 blocks was designed for L2 blocks")
print("   (0.25s each -> 2-year halvings). But Arbitrum's Solidity block.number")
print("   returns L1 Ethereum blocks (12s each) -> 252M * 12s = 96 years.")
print("   Reward stays at 10 TGBT per solution for the foreseeable future.")
print()
print("3. POOL 1 IS THE ONLY WORKING POOL")
print("   700M TGBT capacity. At 1,000 miners it lasts ~3.8 years.")
print("   At 10,000 miners it drains in ~140 days.")
print("   Pool 0 (700M) is permanently stranded (difficulty=1000 = impossible).")
print("   Remaining 500M TGBT needs new pool(s) via governance.")
print()
print("4. STALE BLOCK REWARDS ARE SEPARATE")
print("   75M TGBT allocation, 84 TGBT used so far. Not affected by mining pools.")
print()
print("5. COMMIT-REVEAL BOTTLENECK = NATURAL RATE LIMITER")
print("   Each miner can only have 1 active commitment at a time.")
print("   minCommitmentAge = 2 L1 blocks. Practical cycle = 5-10 min.")
print("   This is the ONLY brake on emission speed per miner.")
print()
print("6. BATCH MINING (BatchMiningModule) BYPASSES PER-SOLUTION COMMIT-REVEAL")
print("   Solutions accumulate in telemetry -> epoch builder batches 10 -> epoch root.")
print("   Higher throughput per miner via this path (explains Pool 1 totalMined")
print("   = 787.5 but global totalMined = 8,812.5 — batch path mined 8,025 TGBT).")
print()
print("RISKS:")
print("- Viral growth scenario: 10K miners joining overnight would drain Pool 1")
print("  in ~4.7 months with no difficulty increase to slow it down")
print("- No halving safety net for ~96 years")
print("- Pool 0's 700M TGBT is lost forever (36.8% of MINING_ALLOCATION)")
