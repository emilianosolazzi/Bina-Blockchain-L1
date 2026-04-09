const http = require('http');
const fs = require('fs');
const path = require('path');
const { ethers } = require('ethers');

const HOST = process.env.L3_READ_API_HOST || '127.0.0.1';
const PORT = Number(process.env.L3_READ_API_PORT || 4385);

const RPC_URL = process.env.L3_RPC_URL || '';
const EPOCH_SETTLEMENT_ADDRESS = process.env.L3_EPOCH_SETTLEMENT_ADDRESS || '';
const PROOF_MARKET_ADDRESS = process.env.L3_PROOF_MARKET_ADDRESS || '';
const CERTIFICATE_REGISTRY_ADDRESS = process.env.L3_CERTIFICATE_REGISTRY_ADDRESS || '';
const FALLBACK_DATA_FILE = process.env.L3_READ_API_FALLBACK_FILE || path.resolve(__dirname, 'sample-read-model.json');

const EPOCH_ABI = [
  'function nextEpochId() view returns (uint256)',
  'function getEpoch(uint256 epochId) view returns ((bytes32 merkleRoot,uint32 leafCount,address operator,uint64 sourceChainId,uint64 committedAt,uint64 finalizedAt,bytes32 sourceRef,bool finalized,string dataUri))',
];

const PROOF_ABI = [
  'function nextReceiptId() view returns (uint256)',
  'function receipts(uint256 receiptId) view returns (address buyer,uint256 epochId,uint8 tier,bytes32 proofHash,bytes32 anchorId,uint256 feePaid,uint64 purchasedAt,string receiptUri)',
];

const CERT_ABI = [
  'function nextCertificateId() view returns (uint256)',
  'function certificates(uint256 certificateId) view returns (address recipient,address issuer,uint256 epochId,bytes32 documentHash,bytes32 anchorId,uint256 feePaid,uint64 issuedAt,string metadataUri)',
];

function sendJson(res, status, payload) {
  const body = JSON.stringify(payload, null, 2);
  res.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Access-Control-Allow-Origin': '*',
    'Cache-Control': 'no-cache',
  });
  res.end(body);
}

function parseUrl(req) {
  return new URL(req.url, `http://${HOST}:${PORT}`);
}

function fileExists(filePath) {
  try {
    return fs.existsSync(filePath);
  } catch {
    return false;
  }
}

function loadFallbackData() {
  if (!fileExists(FALLBACK_DATA_FILE)) {
    return {
      source: 'empty-fallback',
      latestRandomness: null,
      epochs: [],
      proofReceipts: [],
      certificates: [],
    };
  }
  return JSON.parse(fs.readFileSync(FALLBACK_DATA_FILE, 'utf8'));
}

function providerReady() {
  return Boolean(RPC_URL && EPOCH_SETTLEMENT_ADDRESS);
}

function getProvider() {
  return new ethers.JsonRpcProvider(RPC_URL);
}

async function listEpochs(limit = 20) {
  if (!providerReady()) {
    return loadFallbackData().epochs.slice(0, limit);
  }

  const provider = getProvider();
  const contract = new ethers.Contract(EPOCH_SETTLEMENT_ADDRESS, EPOCH_ABI, provider);
  const nextEpochId = Number(await contract.nextEpochId());
  const start = Math.max(0, nextEpochId - limit);
  const epochs = [];

  for (let epochId = start; epochId < nextEpochId; epochId++) {
    const epoch = await contract.getEpoch(epochId);
    epochs.push(projectEpoch(epochId, epoch));
  }

  return epochs.reverse();
}

function projectEpoch(epochId, epoch) {
  return {
    epochId,
    merkleRoot: epoch.merkleRoot,
    leafCount: Number(epoch.leafCount),
    operator: epoch.operator,
    sourceChainId: Number(epoch.sourceChainId),
    committedAt: Number(epoch.committedAt),
    finalizedAt: Number(epoch.finalizedAt),
    sourceRef: epoch.sourceRef,
    finalized: Boolean(epoch.finalized),
    dataUri: epoch.dataUri,
    trustLabel: epoch.finalized ? 'epoch-settled' : 'experimental',
  };
}

async function getEpoch(epochId) {
  if (!providerReady()) {
    return loadFallbackData().epochs.find((epoch) => Number(epoch.epochId) === Number(epochId)) || null;
  }

  const provider = getProvider();
  const contract = new ethers.Contract(EPOCH_SETTLEMENT_ADDRESS, EPOCH_ABI, provider);
  return projectEpoch(Number(epochId), await contract.getEpoch(epochId));
}

async function listProofReceipts(limit = 20) {
  if (!(RPC_URL && PROOF_MARKET_ADDRESS)) {
    return loadFallbackData().proofReceipts.slice(0, limit);
  }

  const provider = getProvider();
  const contract = new ethers.Contract(PROOF_MARKET_ADDRESS, PROOF_ABI, provider);
  const nextReceiptId = Number(await contract.nextReceiptId());
  const start = Math.max(0, nextReceiptId - limit);
  const rows = [];

  for (let receiptId = start; receiptId < nextReceiptId; receiptId++) {
    const receipt = await contract.receipts(receiptId);
    rows.push(projectProofReceipt(receiptId, receipt));
  }

  return rows.reverse();
}

function projectProofReceipt(receiptId, receipt) {
  return {
    receiptId,
    buyer: receipt.buyer,
    epochId: Number(receipt.epochId),
    tier: Number(receipt.tier),
    proofHash: receipt.proofHash,
    anchorId: receipt.anchorId,
    feePaid: receipt.feePaid.toString(),
    purchasedAt: Number(receipt.purchasedAt),
    receiptUri: receipt.receiptUri,
    trustLabel: receipt.anchorId !== ethers.ZeroHash ? 'externally-anchored' : 'proof-verifiable',
  };
}

async function listCertificates(limit = 20) {
  if (!(RPC_URL && CERTIFICATE_REGISTRY_ADDRESS)) {
    return loadFallbackData().certificates.slice(0, limit);
  }

  const provider = getProvider();
  const contract = new ethers.Contract(CERTIFICATE_REGISTRY_ADDRESS, CERT_ABI, provider);
  const nextCertificateId = Number(await contract.nextCertificateId());
  const start = Math.max(0, nextCertificateId - limit);
  const rows = [];

  for (let certificateId = start; certificateId < nextCertificateId; certificateId++) {
    const cert = await contract.certificates(certificateId);
    rows.push(projectCertificate(certificateId, cert));
  }

  return rows.reverse();
}

function projectCertificate(certificateId, cert) {
  return {
    certificateId,
    recipient: cert.recipient,
    issuer: cert.issuer,
    epochId: Number(cert.epochId),
    documentHash: cert.documentHash,
    anchorId: cert.anchorId,
    feePaid: cert.feePaid.toString(),
    issuedAt: Number(cert.issuedAt),
    metadataUri: cert.metadataUri,
  };
}

async function getLatestRandomness() {
  const epochs = await listEpochs(1);
  const latestEpoch = epochs[0] || null;

  if (!latestEpoch) {
    return null;
  }

  return {
    epochId: latestEpoch.epochId,
    outputHash: latestEpoch.merkleRoot,
    finalized: latestEpoch.finalized,
    trustLabel: latestEpoch.trustLabel,
    dataUri: latestEpoch.dataUri,
    sourceChainId: latestEpoch.sourceChainId,
  };
}

async function handleRequest(req, res) {
  const url = parseUrl(req);

  try {
    if (req.method === 'GET' && url.pathname === '/api/health') {
      return sendJson(res, 200, {
        status: 'ok',
        rpcConfigured: Boolean(RPC_URL),
        contractsConfigured: {
          epochSettlement: Boolean(EPOCH_SETTLEMENT_ADDRESS),
          proofMarket: Boolean(PROOF_MARKET_ADDRESS),
          certificateRegistry: Boolean(CERTIFICATE_REGISTRY_ADDRESS),
        },
        fallbackFile: fileExists(FALLBACK_DATA_FILE) ? FALLBACK_DATA_FILE : null,
      });
    }

    if (req.method === 'GET' && url.pathname === '/api/randomness/latest') {
      const latest = await getLatestRandomness();
      return latest
        ? sendJson(res, 200, latest)
        : sendJson(res, 404, { error: 'No randomness available yet' });
    }

    if (req.method === 'GET' && url.pathname === '/api/epochs') {
      const limit = Number(url.searchParams.get('limit') || 20);
      return sendJson(res, 200, { epochs: await listEpochs(limit) });
    }

    const epochMatch = url.pathname.match(/^\/api\/epochs\/(\d+)$/);
    if (req.method === 'GET' && epochMatch) {
      const epoch = await getEpoch(Number(epochMatch[1]));
      return epoch
        ? sendJson(res, 200, epoch)
        : sendJson(res, 404, { error: 'Epoch not found' });
    }

    if (req.method === 'GET' && url.pathname === '/api/proof-receipts') {
      const limit = Number(url.searchParams.get('limit') || 20);
      return sendJson(res, 200, { proofReceipts: await listProofReceipts(limit) });
    }

    if (req.method === 'GET' && url.pathname === '/api/certificates') {
      const limit = Number(url.searchParams.get('limit') || 20);
      return sendJson(res, 200, { certificates: await listCertificates(limit) });
    }

    return sendJson(res, 404, { error: 'Not found' });
  } catch (error) {
    return sendJson(res, 500, { error: error.message });
  }
}

const server = http.createServer(handleRequest);
server.listen(PORT, HOST, () => {
  console.log(`L3 read API listening on http://${HOST}:${PORT}`);
});