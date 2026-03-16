/**
 * remix-helper.js
 *
 * Quick CLI tool for interacting with the Sepolia-deployed system.
 * All contract addresses are pre-filled from the Remix deployment session.
 *
 * Usage:
 *   node remix-helper.js <command> [args...]
 *
 * Commands:
 *   status                       - Print all module addresses registered in Core
 *   grant-governance <address>   - Grant GOVERNANCE_ROLE to an address (run from admin wallet)
 *   set-module <moduleId> <addr> - Register a module in Core
 *   epoch-info <epochId>         - Get BatchMiningModule epoch info
 *   balance <address>            - TGBT balance of address
 *   mint-role-check              - Check if TokenomicsModule has MINTER_ROLE on TGBT
 *   modules                      - Print all known module slot bytes32 values
 *
 * Env:
 *   PRIVATE_KEY   - wallet private key (optional, needed for write ops)
 *   RPC_URL       - defaults to public Sepolia node
 */

const { ethers } = require('ethers');

// ── Deployed Addresses ──────────────────────────────────────────────────────
const ADDRESSES = {
	core:       '0x843fAc753610163776374Ab0261029BAEA0251b7',
	tokenomics: '0xcf0a632A88D759f4A4ad0eA0317B5BE5A10638A5',
	batch:      '0xd52467e0C442c0817665fdB11f86FC47dC56ef3E',
	tgbt:       '0x496598fDeab78fb2986e89d396249779595418E9',
	admin:      '0x3058bd411b9ec0dF6C7d0b04914C9bd2934b7fb3',
	deployer:   '0xF11676bc166E2427c8Ecf134911572cb5aEe6c52',
};

// ── Module Slot IDs ─────────────────────────────────────────────────────────
const MODULE_IDS = {
	MINING_MODULE:       ethers.utils.keccak256(ethers.utils.toUtf8Bytes('MINING_MODULE')),
	TOKENOMICS_MODULE:   ethers.utils.keccak256(ethers.utils.toUtf8Bytes('TOKENOMICS_MODULE')),
	BATCH_MINING_MODULE: ethers.utils.keccak256(ethers.utils.toUtf8Bytes('BATCH_MINING_MODULE')),
	RANDOMNESS_MODULE:   ethers.utils.keccak256(ethers.utils.toUtf8Bytes('RANDOMNESS_MODULE')),
	GOVERNANCE_MODULE:   ethers.utils.keccak256(ethers.utils.toUtf8Bytes('GOVERNANCE_MODULE')),
};

// ── Roles ───────────────────────────────────────────────────────────────────
const ROLES = {
	GOVERNANCE_ROLE: '0x71840dc4906352362b0cdaf79870196c8e42acafade72d5d5a6d59291253ceb1',
	MINTER_ROLE:     ethers.utils.keccak256(ethers.utils.toUtf8Bytes('MINTER_ROLE')),
	DEFAULT_ADMIN:   ethers.constants.HashZero,
};

// ── ABIs ────────────────────────────────────────────────────────────────────
const CORE_ABI = [
	'function moduleAddress(bytes32 moduleId) view returns (address)',
	'function setModule(bytes32 moduleId, address module) external',
	'function hasRole(bytes32 role, address account) view returns (bool)',
	'function grantRole(bytes32 role, address account) external',
	'function GOVERNANCE_ROLE() view returns (bytes32)',
	'function TOKENOMICS_MODULE() view returns (bytes32)',
	'function BATCH_MINING_MODULE() view returns (bytes32)',
	'function MINING_MODULE() view returns (bytes32)',
];

const TOKENOMICS_ABI = [
	'function getMiningEconomics() view returns (uint256 currentReward, uint256 currentEpoch, uint256 blocksPerEpoch, uint256 halvingInterval, uint256 nextHalvingBlock, uint256 currentBonusThreshold, uint256 currentBonusMultiplier, uint256 minedSoFar, uint256 remainingAllocation)',
	'function onlyMiningModuleAddress() view returns (address)',
	'function onlyBatchMiningModuleAddress() view returns (address)',
];

const BATCH_ABI = [
	'function currentEpochId() view returns (uint256)',
	'function REQUIRED_TSTAKE() view returns (uint256)',
	'function REWARD_PER_SOLUTION() view returns (uint256)',
	'function getEpochInfo(uint256 epochId) view returns (tuple(bytes32 merkleRoot, uint64 startBlock, uint64 endBlock, uint32 leafCount, address operator, uint8 poolId, bool finalized, uint256 totalReward, bool storageAttested, bytes32 attestationHash))',
];

const ERC20_ABI = [
	'function balanceOf(address) view returns (uint256)',
	'function hasRole(bytes32 role, address account) view returns (bool)',
	'function symbol() view returns (string)',
];

// ── Setup ────────────────────────────────────────────────────────────────────
const RPC_URL = process.env.RPC_URL || 'https://ethereum-sepolia-rpc.publicnode.com';
const provider = new ethers.providers.JsonRpcProvider(RPC_URL);

function getSigner() {
	const key = process.env.PRIVATE_KEY;
	if (!key) { console.error('Set PRIVATE_KEY env var for write operations'); process.exit(1); }
	return new ethers.Wallet(key.startsWith('0x') ? key : '0x' + key, provider);
}

function getCore(signerOrProvider = provider) {
	return new ethers.Contract(ADDRESSES.core, CORE_ABI, signerOrProvider);
}

// ── Commands ─────────────────────────────────────────────────────────────────

async function cmdStatus() {
	const core      = getCore();
	const tokenomics = new ethers.Contract(ADDRESSES.tokenomics, TOKENOMICS_ABI, provider);
	const tgbt      = new ethers.Contract(ADDRESSES.tgbt, ERC20_ABI, provider);

	console.log('\n=== System Status (Sepolia) ===\n');
	console.log('Core:        ', ADDRESSES.core);
	console.log('Tokenomics:  ', ADDRESSES.tokenomics);
	console.log('BatchMining: ', ADDRESSES.batch);
	console.log('TGBT:        ', ADDRESSES.tgbt);

	console.log('\n--- Module Slots in Core ---');
	for (const [name, id] of Object.entries(MODULE_IDS)) {
		const addr = await core.moduleAddress(id);
		const registered = addr !== ethers.constants.AddressZero;
		console.log(`  ${name.padEnd(25)} ${registered ? '✅' : '❌'} ${addr}`);
	}

	console.log('\n--- Roles ---');
	const adminHasGov = await core.hasRole(ROLES.GOVERNANCE_ROLE, ADDRESSES.admin);
	const deployerHasGov = await core.hasRole(ROLES.GOVERNANCE_ROLE, ADDRESSES.deployer);
	console.log(`  admin    has GOVERNANCE_ROLE: ${adminHasGov ? '✅' : '❌'}`);
	console.log(`  deployer has GOVERNANCE_ROLE: ${deployerHasGov ? '✅' : '❌'}`);

	const tokenomicsHasMinter = await tgbt.hasRole(ROLES.MINTER_ROLE, ADDRESSES.tokenomics);
	console.log(`  tokenomics has MINTER_ROLE on TGBT: ${tokenomicsHasMinter ? '✅' : '❌'}`);

	console.log('\n--- Tokenomics ---');
	const econ = await tokenomics.getMiningEconomics();
	console.log(`  rewardPerEpoch:       ${ethers.utils.formatEther(econ.currentReward)} TGBT`);
	console.log(`  currentEpoch:         ${econ.currentEpoch}`);
	console.log(`  blocksPerEpoch:       ${econ.blocksPerEpoch}`);
	console.log(`  totalMinedSoFar:      ${ethers.utils.formatEther(econ.minedSoFar)} TGBT`);
	console.log(`  remainingAllocation:  ${ethers.utils.formatEther(econ.remainingAllocation)} TGBT`);

	console.log('\n--- BatchMiningModule ---');
	const batch = new ethers.Contract(ADDRESSES.batch, BATCH_ABI, provider);
	const epochId = await batch.currentEpochId();
	const stakeReq = await batch.REQUIRED_TSTAKE();
	const rewardPerSol = await batch.REWARD_PER_SOLUTION();
	console.log(`  currentEpochId:      ${epochId}`);
	console.log(`  REQUIRED_TSTAKE:     ${ethers.utils.formatEther(stakeReq)} TGBT`);
	console.log(`  REWARD_PER_SOLUTION: ${ethers.utils.formatEther(rewardPerSol)} TGBT`);
	console.log('');
}

async function cmdGrantGovernance(target) {
	if (!target) { console.error('Usage: node remix-helper.js grant-governance <address>'); process.exit(1); }
	const signer = getSigner();
	const core = getCore(signer);
	console.log(`Granting GOVERNANCE_ROLE to ${target} ...`);
	const tx = await core.grantRole(ROLES.GOVERNANCE_ROLE, target);
	console.log('tx:', tx.hash);
	await tx.wait();
	console.log('✅ Done');
}

async function cmdSetModule(moduleId, moduleAddr) {
	if (!moduleId || !moduleAddr) {
		console.error('Usage: node remix-helper.js set-module <moduleId|name> <address>');
		console.error('Names:', Object.keys(MODULE_IDS).join(', '));
		process.exit(1);
	}
	const id = MODULE_IDS[moduleId] || moduleId;
	const signer = getSigner();
	const core = getCore(signer);
	console.log(`setModule(${id}, ${moduleAddr}) ...`);
	const tx = await core.setModule(id, moduleAddr);
	console.log('tx:', tx.hash);
	await tx.wait();
	console.log('✅ Done');
}

async function cmdEpochInfo(epochId) {
	const id = epochId !== undefined ? Number(epochId) : undefined;
	const batch = new ethers.Contract(ADDRESSES.batch, BATCH_ABI, provider);
	const current = Number(await batch.currentEpochId());
	const target = id !== undefined ? id : (current > 0 ? current - 1 : 0);
	console.log(`\nEpoch ${target} info:`);
	if (current === 0) { console.log('  No epochs committed yet'); return; }
	const info = await batch.getEpochInfo(target);
	console.log('  merkleRoot: ', info.merkleRoot);
	console.log('  startBlock: ', info.startBlock.toString());
	console.log('  endBlock:   ', info.endBlock.toString());
	console.log('  leafCount:  ', info.leafCount.toString());
	console.log('  operator:   ', info.operator);
	console.log('  poolId:     ', info.poolId.toString());
	console.log('  finalized:  ', info.finalized);
	console.log('  totalReward:', ethers.utils.formatEther(info.totalReward), 'TGBT');
	console.log('  storageAttested:', info.storageAttested);
	console.log('  attestationHash:', info.attestationHash);
}

async function cmdBalance(address) {
	const addr = address || ADDRESSES.deployer;
	const tgbt = new ethers.Contract(ADDRESSES.tgbt, ERC20_ABI, provider);
	const bal = await tgbt.balanceOf(addr);
	console.log(`TGBT balance of ${addr}: ${ethers.utils.formatEther(bal)} TGBT`);
}

async function cmdMintRoleCheck() {
	const tgbt = new ethers.Contract(ADDRESSES.tgbt, ERC20_ABI, provider);
	const has = await tgbt.hasRole(ROLES.MINTER_ROLE, ADDRESSES.tokenomics);
	console.log(`TokenomicsModule has MINTER_ROLE on TGBT: ${has ? '✅ YES' : '❌ NO'}`);
}

async function cmdModules() {
	console.log('\n=== Module Slot bytes32 IDs ===\n');
	for (const [name, id] of Object.entries(MODULE_IDS)) {
		console.log(`  ${name.padEnd(25)} ${id}`);
	}
	console.log('\n=== Role bytes32 IDs ===\n');
	for (const [name, id] of Object.entries(ROLES)) {
		console.log(`  ${name.padEnd(25)} ${id}`);
	}
	console.log('');
}

// ── Main ─────────────────────────────────────────────────────────────────────
const [,, cmd, ...args] = process.argv;

(async () => {
	switch (cmd) {
		case 'status':          await cmdStatus(); break;
		case 'grant-governance': await cmdGrantGovernance(args[0]); break;
		case 'set-module':      await cmdSetModule(args[0], args[1]); break;
		case 'epoch-info':      await cmdEpochInfo(args[0]); break;
		case 'balance':         await cmdBalance(args[0]); break;
		case 'mint-role-check': await cmdMintRoleCheck(); break;
		case 'modules':         await cmdModules(); break;
		default:
			console.log('Commands: status | grant-governance | set-module | epoch-info | balance | mint-role-check | modules');
	}
})().catch(e => { console.error(e.message || e); process.exit(1); });
