// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

interface IERC20 {
    function transfer(address to, uint256 amount) external returns (bool);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
}

library SafeERC20 {
    error SafeERC20CallFailed();

    function safeTransfer(IERC20 token, address to, uint256 amount) internal {
        (bool ok, bytes memory data) = address(token).call(
            abi.encodeWithSelector(token.transfer.selector, to, amount)
        );
        if (!ok || (data.length != 0 && !abi.decode(data, (bool)))) {
            revert SafeERC20CallFailed();
        }
    }

    function safeTransferFrom(IERC20 token, address from, address to, uint256 amount) internal {
        (bool ok, bytes memory data) = address(token).call(
            abi.encodeWithSelector(token.transferFrom.selector, from, to, amount)
        );
        if (!ok || (data.length != 0 && !abi.decode(data, (bool)))) {
            revert SafeERC20CallFailed();
        }
    }
}

/// @title  FastPathIdentity V2
/// @author Emiliano Solazzi (FastPath / proof160)
/// @notice V2 adds:
///         1. BINA quantum-safe key registry (Ed25519 + Falcon-512)
///         2. expMod precompile (saves ~50k gas per registration)
///         3. receivePreference default fix (ViaHash160 by default)
///         4. getFullProfile view function
///         5. PreferenceAlreadySet error removed (was UX friction)
///         6. BINA address resolution (three-chain in one call)
contract FastPathIdentityV2 {
    using SafeERC20 for IERC20;

    // ════════════════════════════════════════
    // ERRORS — all original + new
    // ════════════════════════════════════════

    error NotOwner();
    error InsufficientFee();
    error AddressAlreadyRegistered();
    error AddressNotRegistered();
    error InvalidSignature();
    error InvalidPublicKey();
    error TransferFailed();
    error ReentrantCall();
    error RelinkDisabled();
    error CooldownActive();
    error PendingRelinkExists();
    error PendingRelinkMissing();
    error NewEvmAlreadyRegistered();
    error NotCurrentOwner();
    error CooldownTooSmall();
    error NoFeesToWithdraw();
    error ZeroHash160();
    error SignatureSMustBeLowOrder();
    error FeeTooHigh();
    error NotPendingOwner();
    error ZeroAddress();
    error NotNewEvmOwner();
    error NotPendingRelinkOwner();
    error EmergencyStopActive();
    error ZeroValue();
    error Hash160NotRegistered();
    error DirectEvmPreferred();
    error NoPendingFunds();
    error ZeroAmount();
    error InvalidToken();
    error PointNotOnCurve();

    // NEW V2 errors
    error BinaKeyAlreadyRegistered();
    error NoBinaKey();
    error BinaKeyCooldownActive();
    error InvalidBinaKey();

    // ════════════════════════════════════════
    // CONSTANTS — unchanged
    // ════════════════════════════════════════

    uint256 private constant HALF_ORDER =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    uint256 public constant MAX_REGISTRATION_FEE = 1 ether;

    // NEW V2
    uint256 public constant BINA_KEY_ROTATION_COOLDOWN = 3 days;
    bytes32 public constant BINA_DERIVATION_MESSAGE_HASH =
        keccak256("Sign to derive your BINA wallet. This is free.");
    bytes32 public constant BINA_DERIVATION_COMMITMENT_DOMAIN =
        keccak256("BINA_METAMASK_DERIVATION_COMMITMENT_V1");

    // ════════════════════════════════════════
    // STATE — all original preserved
    // ════════════════════════════════════════

    address public owner;
    address public pendingOwner;
    uint256 public registrationFee;
    bool private locked;
    bool public relinkEnabled;
    uint256 public relinkCooldown;
    bool public emergencyStop;

    mapping(bytes20 => address) public btcToEvm;
    mapping(address => bytes20) public evmToBtc;
    mapping(bytes20 => uint256) public lastLinkTime;

    enum ReceivePreference { DirectEVM, ViaHash160 }
    mapping(address => ReceivePreference) public receivePreference;

    // NEW V2: track whether preference was explicitly set
    // Allows default to be ViaHash160 without
    // breaking existing setReceivePreference callers
    mapping(address => bool) private _preferenceSet;

    struct PendingRelink {
        address newEvm;
        uint256 unlockTime;
        bool exists;
    }

    mapping(bytes20 => PendingRelink) public pendingRelinks;
    mapping(bytes20 => address) private activeEvm;
    mapping(address => uint256) public pendingWithdrawals;
    uint256 public accumulatedFees;

    // NEW V2: BINA quantum-safe key registry
    struct BinaKey {
        bytes32 ed25519PublicKey;   // 32 bytes — stored directly
        bytes32 publicKeyHash;      // keccak256(full BINA public key: ed25519 || falcon_pk)
        bytes32 derivationCommitment; // public commitment to the client-side derivation context
        bytes20 binaAddress;        // computed off-chain by BINA L1: BLAKE3("BINA-ADDR-v1" || ed25519 || falcon_pk)[0..20]
        uint256 registeredAt;
        address registeredBy;       // EVM address at time of registration
        bool active;
    }

    mapping(bytes20 => BinaKey) public binaKeys;
    mapping(bytes20 => bytes20) public hash160ToBinaAddress;
    mapping(bytes20 => bytes20) public binaAddressToHash160;
    mapping(bytes20 => uint256) public lastBinaKeyRotation;

    // ════════════════════════════════════════
    // EVENTS — all original preserved
    // ════════════════════════════════════════

    event BitcoinAddressRegistered(
        address indexed user,
        bytes20 btcHash160,
        uint256 feePaid
    );
    event FeeUpdated(uint256 newFee);
    event FeesWithdrawn(address indexed recipient, uint256 amount);
    event RelinkInitiated(
        bytes20 indexed btcHash160,
        address indexed newEvm,
        uint256 unlockTime
    );
    event RelinkCompleted(
        bytes20 indexed btcHash160,
        address indexed oldEvm,
        address indexed newEvm
    );
    event RelinkCancelled(
        bytes20 indexed btcHash160,
        address indexed cancelledBy
    );
    event RelinkCooldownUpdated(uint256 newCooldown);
    event RelinkToggled(bool enabled);
    event FundsReceived(
        bytes20 indexed btcHash160,
        address indexed receiver,
        uint256 amount,
        address token
    );
    event PendingFundsDeposited(
        bytes20 indexed btcHash160,
        address indexed receiver,
        uint256 amount
    );
    event PendingFundsWithdrawn(address indexed receiver, uint256 amount);
    event ReceivePreferenceUpdated(
        address indexed user,
        ReceivePreference preference
    );
    event OwnershipTransferred(
        address indexed previousOwner,
        address indexed newOwner
    );
    event OwnershipTransferStarted(
        address indexed previousOwner,
        address indexed newOwner
    );
    event EmergencyStopToggled(bool disabled);

    // NEW V2 events
    event BinaKeyRegistered(
        bytes20 indexed hash160,
        bytes20 indexed binaAddress,
        bytes32 ed25519PublicKey,
        bytes32 publicKeyHash,
        bytes32 derivationCommitment,
        address indexed registeredBy
    );
    event BinaKeyRotated(
        bytes20 indexed hash160,
        bytes20 indexed oldBinaAddress,
        bytes20 indexed newBinaAddress,
        address rotatedBy
    );
    event BinaKeyDeactivated(
        bytes20 indexed hash160,
        bytes20 binaAddress
    );

    // ════════════════════════════════════════
    // CONSTRUCTOR
    // ════════════════════════════════════════

    constructor(uint256 _fee) {
        if (_fee > MAX_REGISTRATION_FEE) revert FeeTooHigh();
        owner = msg.sender;
        registrationFee = _fee;
        locked = false;
        relinkEnabled = false;
        relinkCooldown = 3 days;
        emergencyStop = false;
    }

    // ════════════════════════════════════════
    // MODIFIERS — unchanged
    // ════════════════════════════════════════

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    modifier nonReentrant() {
        if (locked) revert ReentrantCall();
        locked = true;
        _;
        locked = false;
    }

    modifier noEmergency() {
        if (emergencyStop) revert EmergencyStopActive();
        _;
    }

    // ════════════════════════════════════════
    // REGISTRATION — unchanged except expMod
    // ════════════════════════════════════════

    function registerBitcoinAddressV2(
        uint8 pubkeyPrefix,
        bytes32 pubkeyX,
        bytes32 r,
        bytes32 s,
        uint8 v,
        bool bitcoinStyle
    ) external payable nonReentrant {
        if (msg.value < registrationFee) revert InsufficientFee();
        if (evmToBtc[msg.sender] != bytes20(0)) revert AddressAlreadyRegistered();

        bytes memory message = bytes(toHex(msg.sender));

        if (pubkeyPrefix != 2 && pubkeyPrefix != 3) revert InvalidPublicKey();
        bytes memory pubkeyDyn = abi.encodePacked(bytes1(pubkeyPrefix), pubkeyX);
        address expectedSigner = ethAddressFromXY(_pubkeyToXYMem(pubkeyDyn));

        uint8 vv = v;
        if (vv < 27) vv += 27;
        if (vv != 27 && vv != 28) revert InvalidSignature();

        if (uint256(s) > HALF_ORDER) revert SignatureSMustBeLowOrder();

        bytes32 digest = bitcoinStyle
            ? toBitcoinSignedMessageHashMem(message)
            : toEthSignedMessageHashMem(message);

        if (ecrecover(digest, vv, r, s) != expectedSigner) revert InvalidSignature();

        bytes20 btcHash160 = btcHash160FromPubkeyMem(pubkeyDyn);
        if (btcHash160 == bytes20(0)) revert ZeroHash160();
        if (btcToEvm[btcHash160] != address(0)) revert AddressAlreadyRegistered();

        btcToEvm[btcHash160] = msg.sender;
        evmToBtc[msg.sender] = btcHash160;
        lastLinkTime[btcHash160] = block.timestamp;
        activeEvm[btcHash160] = msg.sender;
        accumulatedFees += msg.value;

        emit BitcoinAddressRegistered(msg.sender, btcHash160, msg.value);
    }

    // ════════════════════════════════════════
    // NEW V2: BINA KEY REGISTRY
    // ════════════════════════════════════════

    /// @notice Register a BINA hybrid public key for your hash160 identity.
    /// @dev Caller must be currentController of this hash160.
    ///      In the FastPath pipeline, BINA is derived server-side from
    ///      deriveSeed(evmAddress, "bina-identity", network), the same
    ///      salt + pepper + evmAddress root used for other derived wallets.
    ///      The EVM cannot cheaply reproduce BINA's BLAKE3 + Falcon key flow,
    ///      so this contract stores the resulting BINA address and public-key
    ///      commitments computed off-chain. A native l1-wallet.exe key is a
    ///      separate, user-sovereign BINA key and must not be silently conflated
    ///      with this FastPath-derived identity key.
    /// @param hash160          Your Bitcoin hash160
    /// @param binaAddress      BINA L1 address computed off-chain from the full hybrid public key
    /// @param ed25519PublicKey Ed25519 public key derived off-chain (32 bytes)
    /// @param fullPublicKeyHash keccak256(ed25519_pubkey || falcon512_pubkey), computed off-chain
    /// @param derivationCommitment Non-secret commitment from binaDerivationCommitment(...)
    function registerBinaKey(
        bytes20 hash160,
        bytes20 binaAddress,
        bytes32 ed25519PublicKey,
        bytes32 fullPublicKeyHash,
        bytes32 derivationCommitment
    ) external {
        if (hash160 == bytes20(0)) revert ZeroHash160();
        if (activeEvm[hash160] != msg.sender) revert NotCurrentOwner();
        if (binaAddress == bytes20(0)) revert InvalidBinaKey();
        if (ed25519PublicKey == bytes32(0)) revert InvalidBinaKey();
        if (fullPublicKeyHash == bytes32(0)) revert InvalidBinaKey();
        if (derivationCommitment == bytes32(0)) revert InvalidBinaKey();
        if (binaKeys[hash160].active) revert BinaKeyAlreadyRegistered();
        if (binaAddressToHash160[binaAddress] != bytes20(0)) revert BinaKeyAlreadyRegistered();
        if (derivationCommitment != binaDerivationCommitment(hash160, binaAddress, fullPublicKeyHash, msg.sender)) {
            revert InvalidBinaKey();
        }

        binaKeys[hash160] = BinaKey({
            ed25519PublicKey: ed25519PublicKey,
            publicKeyHash: fullPublicKeyHash,
            derivationCommitment: derivationCommitment,
            binaAddress: binaAddress,
            registeredAt: block.timestamp,
            registeredBy: msg.sender,
            active: true
        });

        hash160ToBinaAddress[hash160] = binaAddress;
        binaAddressToHash160[binaAddress] = hash160;
        lastBinaKeyRotation[hash160] = block.timestamp;

        emit BinaKeyRegistered(
            hash160,
            binaAddress,
            ed25519PublicKey,
            fullPublicKeyHash,
            derivationCommitment,
            msg.sender
        );
    }

    /// @notice Rotate to a new BINA key.
    /// @dev Enforces BINA_KEY_ROTATION_COOLDOWN.
    ///      Mirrors FastPathIdentity relink philosophy —
    ///      deliberate delay prevents panic rotations.
    function rotateBinaKey(
        bytes20 hash160,
        bytes20 newBinaAddress,
        bytes32 newEd25519PublicKey,
        bytes32 newFullPublicKeyHash,
        bytes32 newDerivationCommitment
    ) external {
        if (hash160 == bytes20(0)) revert ZeroHash160();
        if (activeEvm[hash160] != msg.sender) revert NotCurrentOwner();
        if (newBinaAddress == bytes20(0)) revert InvalidBinaKey();
        if (newEd25519PublicKey == bytes32(0)) revert InvalidBinaKey();
        if (newFullPublicKeyHash == bytes32(0)) revert InvalidBinaKey();
        if (newDerivationCommitment == bytes32(0)) revert InvalidBinaKey();
        if (!binaKeys[hash160].active) revert NoBinaKey();
        if (newDerivationCommitment != binaDerivationCommitment(hash160, newBinaAddress, newFullPublicKeyHash, msg.sender)) {
            revert InvalidBinaKey();
        }

        bytes20 registeredHash160 = binaAddressToHash160[newBinaAddress];
        if (registeredHash160 != bytes20(0) && registeredHash160 != hash160) {
            revert BinaKeyAlreadyRegistered();
        }

        if (block.timestamp < lastBinaKeyRotation[hash160]
            + BINA_KEY_ROTATION_COOLDOWN) {
            revert BinaKeyCooldownActive();
        }

        bytes20 oldBinaAddress = hash160ToBinaAddress[hash160];
        delete binaAddressToHash160[oldBinaAddress];

        binaKeys[hash160].ed25519PublicKey = newEd25519PublicKey;
        binaKeys[hash160].publicKeyHash = newFullPublicKeyHash;
        binaKeys[hash160].derivationCommitment = newDerivationCommitment;
        binaKeys[hash160].binaAddress = newBinaAddress;
        binaKeys[hash160].registeredBy = msg.sender;
        binaKeys[hash160].registeredAt = block.timestamp;

        hash160ToBinaAddress[hash160] = newBinaAddress;
        binaAddressToHash160[newBinaAddress] = hash160;
        lastBinaKeyRotation[hash160] = block.timestamp;

        emit BinaKeyRotated(
            hash160,
            oldBinaAddress,
            newBinaAddress,
            msg.sender
        );
    }

    /// @notice Deactivate BINA key. Does not delete history.
    function deactivateBinaKey(bytes20 hash160) external {
        if (hash160 == bytes20(0)) revert ZeroHash160();
        if (activeEvm[hash160] != msg.sender) revert NotCurrentOwner();
        if (!binaKeys[hash160].active) revert NoBinaKey();

        bytes20 binaAddress = hash160ToBinaAddress[hash160];
        binaKeys[hash160].active = false;
        delete binaAddressToHash160[binaAddress];
        delete hash160ToBinaAddress[hash160];

        emit BinaKeyDeactivated(hash160, binaAddress);
    }

    // ════════════════════════════════════════
    // VIEW FUNCTIONS
    // ════════════════════════════════════════

    function hasControl(address evm) external view returns (bool) {
        bytes20 btc = evmToBtc[evm];
        return btc != bytes20(0) && activeEvm[btc] == evm;
    }

    function currentController(bytes20 btcHash160)
        external view returns (address) {
        return activeEvm[btcHash160];
    }

    function isQuantumSafe(bytes20 hash160)
        external view returns (bool) {
        return binaKeys[hash160].active;
    }

    function getBinaAddress(bytes20 hash160)
        external view returns (bytes20) {
        return hash160ToBinaAddress[hash160];
    }

    function resolveFromBina(bytes20 binaAddress)
        external view
        returns (bytes20 hash160, address evmController) {
        hash160 = binaAddressToHash160[binaAddress];
        if (hash160 == bytes20(0)) revert NoBinaKey();
        evmController = activeEvm[hash160];
    }

    /// @notice Public helper for browser/client code preparing registerBinaKey.
    /// @dev This binds the server-computed BINA identity fields to the current
    ///      chain, contract, EVM controller, and hash160 identity.
    function binaDerivationCommitment(
        bytes20 hash160,
        bytes20 binaAddress,
        bytes32 fullPublicKeyHash,
        address evmController
    ) public view returns (bytes32) {
        return keccak256(
            abi.encodePacked(
                BINA_DERIVATION_COMMITMENT_DOMAIN,
                BINA_DERIVATION_MESSAGE_HASH,
                block.chainid,
                address(this),
                evmController,
                hash160,
                binaAddress,
                fullPublicKeyHash
            )
        );
    }

    /// @notice NEW V2: Full identity profile in one call.
    /// @dev Eliminates need for multiple RPC calls from frontend.
    function getFullProfile(bytes20 hash160)
        external view
        returns (
            address evmController,
            bytes20 binaAddress,
            bytes32 ed25519Key,
            bytes32 binaKeyHash,
            bytes32 derivationCommitment,
            bool hasQuantumKey,
            uint256 binaRegisteredAt,
            uint256 lastLink
        )
    {
        evmController = activeEvm[hash160];
        BinaKey memory key = binaKeys[hash160];
        hasQuantumKey = key.active;
        binaAddress = key.binaAddress;
        ed25519Key = key.ed25519PublicKey;
        binaKeyHash = key.publicKeyHash;
        derivationCommitment = key.derivationCommitment;
        binaRegisteredAt = key.registeredAt;
        lastLink = lastLinkTime[hash160];
    }

    function getRelinkStatus(bytes20 btcHash160)
        external view
        returns (
            bool hasPending,
            address pendingNewEvm,
            uint256 unlockTime,
            uint256 cooldownRemaining
        )
    {
        PendingRelink memory pending = pendingRelinks[btcHash160];
        hasPending = pending.exists;
        pendingNewEvm = pending.newEvm;
        unlockTime = pending.unlockTime;

        if (pending.exists) {
            cooldownRemaining = pending.unlockTime > block.timestamp
                ? pending.unlockTime - block.timestamp
                : 0;
        } else {
            uint256 nextTime = lastLinkTime[btcHash160] + relinkCooldown;
            cooldownRemaining = nextTime > block.timestamp
                ? nextTime - block.timestamp
                : 0;
        }
    }

    // ════════════════════════════════════════
    // RECEIVE PREFERENCE
    // V2 fix: default is ViaHash160 not DirectEVM
    // Opt-out instead of opt-in
    // ════════════════════════════════════════

    function setReceivePreference(ReceivePreference preference) external {
        receivePreference[msg.sender] = preference;
        _preferenceSet[msg.sender] = true;
        emit ReceivePreferenceUpdated(msg.sender, preference);
    }

    /// @dev Internal preference check with ViaHash160 default.
    function _getPreference(address user)
        internal view returns (ReceivePreference) {
        if (!_preferenceSet[user]) return ReceivePreference.ViaHash160;
        return receivePreference[user];
    }

    function receiveFunds(bytes20 btcHash160)
        external payable nonReentrant {
        if (btcHash160 == bytes20(0)) revert ZeroHash160();
        if (msg.value == 0) revert ZeroValue();
        address receiver = activeEvm[btcHash160];
        if (receiver == address(0)) revert Hash160NotRegistered();

        // V2 fix: use _getPreference not raw mapping
        if (_getPreference(receiver) != ReceivePreference.ViaHash160) {
            revert DirectEvmPreferred();
        }

        pendingWithdrawals[receiver] += msg.value;
        emit PendingFundsDeposited(btcHash160, receiver, msg.value);
    }

    function receiveTokens(
        bytes20 btcHash160,
        address token,
        uint256 amount
    ) external nonReentrant {
        if (btcHash160 == bytes20(0)) revert ZeroHash160();
        if (amount == 0) revert ZeroAmount();
        if (token == address(0)) revert InvalidToken();
        address receiver = activeEvm[btcHash160];
        if (receiver == address(0)) revert Hash160NotRegistered();

        // V2 fix: use _getPreference
        if (_getPreference(receiver) != ReceivePreference.ViaHash160) {
            revert DirectEvmPreferred();
        }

        IERC20(token).safeTransferFrom(msg.sender, receiver, amount);
        emit FundsReceived(btcHash160, receiver, amount, token);
    }

    function withdrawPendingFunds() external nonReentrant {
        uint256 amount = pendingWithdrawals[msg.sender];
        if (amount == 0) revert NoPendingFunds();
        pendingWithdrawals[msg.sender] = 0;
        emit PendingFundsWithdrawn(msg.sender, amount);
        (bool success, ) = msg.sender.call{value: amount}("");
        if (!success) revert TransferFailed();
    }

    // ════════════════════════════════════════
    // ADMIN — unchanged
    // ════════════════════════════════════════

    function setRegistrationFee(uint256 _fee) external onlyOwner {
        if (_fee > MAX_REGISTRATION_FEE) revert FeeTooHigh();
        registrationFee = _fee;
        emit FeeUpdated(_fee);
    }

    function withdrawFees() external onlyOwner nonReentrant {
        uint256 amount = accumulatedFees;
        if (amount == 0) revert NoFeesToWithdraw();
        accumulatedFees = 0;
        emit FeesWithdrawn(owner, amount);
        (bool success, ) = owner.call{value: amount}("");
        if (!success) revert TransferFailed();
    }

    function rescueERC20(
        address token,
        address to,
        uint256 amount
    ) external onlyOwner {
        if (token == address(0)) revert ZeroAddress();
        if (to == address(0)) revert ZeroAddress();
        IERC20(token).safeTransfer(to, amount);
    }

    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        pendingOwner = newOwner;
        emit OwnershipTransferStarted(owner, newOwner);
    }

    function acceptOwnership() external {
        if (msg.sender != pendingOwner) revert NotPendingOwner();
        emit OwnershipTransferred(owner, pendingOwner);
        owner = pendingOwner;
        pendingOwner = address(0);
    }

    function setRelinkEnabled(bool enabled) external onlyOwner {
        relinkEnabled = enabled;
        emit RelinkToggled(enabled);
    }

    function setRelinkCooldown(uint256 cooldown) external onlyOwner {
        if (cooldown < 1 hours) revert CooldownTooSmall();
        relinkCooldown = cooldown;
        emit RelinkCooldownUpdated(cooldown);
    }

    function setEmergencyStop(bool disable) external onlyOwner {
        emergencyStop = disable;
        emit EmergencyStopToggled(disable);
    }

    // ════════════════════════════════════════
    // RELINK — unchanged
    // ════════════════════════════════════════

    function initiateRelink(
        bytes20 btcHash160,
        address newEvm,
        bytes calldata pubkey,
        bytes calldata signature
    ) external noEmergency {
        if (pubkey.length > 65) revert InvalidPublicKey();
        if (signature.length != 65) revert InvalidSignature();
        if (btcHash160 == bytes20(0)) revert ZeroHash160();
        if (!relinkEnabled) revert RelinkDisabled();
        if (btcToEvm[btcHash160] == address(0)) revert AddressNotRegistered();
        if (newEvm == address(0)) revert ZeroAddress();
        if (newEvm != msg.sender) revert NotNewEvmOwner();
        if (pendingRelinks[btcHash160].exists) revert PendingRelinkExists();
        if (evmToBtc[newEvm] != bytes20(0)) revert NewEvmAlreadyRegistered();
        if (block.timestamp < lastLinkTime[btcHash160] + relinkCooldown) {
            revert CooldownActive();
        }
        if (btcHash160FromPubkey(pubkey) != btcHash160) revert InvalidPublicKey();

        bytes memory msgMem = bytes(toHex(newEvm));
        if (!_verifySignatureFromMemory(pubkey, signature, msgMem)) {
            revert InvalidSignature();
        }

        uint256 unlockTime = block.timestamp + relinkCooldown;
        pendingRelinks[btcHash160] = PendingRelink({
            newEvm: newEvm,
            unlockTime: unlockTime,
            exists: true
        });

        emit RelinkInitiated(btcHash160, newEvm, unlockTime);
    }

    function finalizeRelink(bytes20 btcHash160) external noEmergency {
        PendingRelink memory pending = pendingRelinks[btcHash160];
        if (!pending.exists) revert PendingRelinkMissing();
        if (msg.sender != pending.newEvm) revert NotPendingRelinkOwner();
        if (block.timestamp < pending.unlockTime) revert CooldownActive();

        address oldEvm = btcToEvm[btcHash160];
        if (oldEvm == address(0)) revert AddressNotRegistered();
        if (evmToBtc[pending.newEvm] != bytes20(0)) revert NewEvmAlreadyRegistered();

        evmToBtc[pending.newEvm] = btcHash160;
        activeEvm[btcHash160] = pending.newEvm;
        lastLinkTime[btcHash160] = block.timestamp;

        delete pendingRelinks[btcHash160];

        emit RelinkCompleted(btcHash160, oldEvm, pending.newEvm);
    }

    function cancelRelink(bytes20 btcHash160) external noEmergency {
        address currentOwner = activeEvm[btcHash160];
        if (currentOwner == address(0)) revert AddressNotRegistered();
        if (msg.sender != currentOwner) revert NotCurrentOwner();
        if (!pendingRelinks[btcHash160].exists) revert PendingRelinkMissing();
        delete pendingRelinks[btcHash160];
        emit RelinkCancelled(btcHash160, msg.sender);
    }

    // ════════════════════════════════════════
    // CRYPTO HELPERS
    // V2 change: expMod uses precompile
    // Everything else unchanged
    // ════════════════════════════════════════

    function _verifySignatureFromMemory(
        bytes calldata pubkey,
        bytes calldata signature,
        bytes memory message
    ) internal view returns (bool) {
        if (pubkey.length != 65 && pubkey.length != 64
            && pubkey.length != 33) return false;
        if (signature.length != 65) return false;

        if (pubkey.length == 33) {
            uint8 prefix = uint8(pubkey[0]);
            if (prefix != 0x02 && prefix != 0x03) return false;
        }

        address expectedSigner = ethAddressFromXY(_pubkeyToXY(pubkey));

        uint8 header = uint8(signature[0]);
        if (header >= 27 && header <= 34) {
            uint8 recId = (header - 27) & 3;
            if (recId <= 1) {
                (bytes32 r, bytes32 s, uint8 v) = _splitCompact(signature);
                bytes32 digest = toBitcoinSignedMessageHashMem(message);
                if (ecrecover(digest, v, r, s) == expectedSigner) return true;
            }
        }

        uint8 vLast = uint8(signature[64]);
        if (vLast == 0 || vLast == 1 || vLast == 27 || vLast == 28) {
            (bytes32 r, bytes32 s, uint8 v) = _splitExpanded(signature);
            bytes32 digest = toEthSignedMessageHashMem(message);
            if (ecrecover(digest, v, r, s) == expectedSigner) return true;
        }

        return false;
    }

    function _splitCompact(bytes calldata sig)
        internal pure returns (bytes32 r, bytes32 s, uint8 v) {
        uint8 header = uint8(sig[0]);
        uint8 recId = (header - 27) & 3;
        v = 27 + recId;
        assembly {
            r := calldataload(add(sig.offset, 1))
            s := calldataload(add(sig.offset, 33))
        }
        if (uint256(s) > HALF_ORDER) revert SignatureSMustBeLowOrder();
    }

    function _splitExpanded(bytes calldata sig)
        internal pure returns (bytes32 r, bytes32 s, uint8 v) {
        uint8 vLast = uint8(sig[64]);
        v = vLast;
        if (v < 27) v += 27;
        assembly {
            r := calldataload(sig.offset)
            s := calldataload(add(sig.offset, 32))
        }
        if (uint256(s) > HALF_ORDER) revert SignatureSMustBeLowOrder();
    }

    function _pubkeyToXY(bytes calldata pubkey)
        internal view returns (bytes memory xy) {
        if (pubkey.length == 65) {
            xy = new bytes(64);
            for (uint256 i = 0; i < 64; i++) xy[i] = pubkey[i + 1];
        } else if (pubkey.length == 64) {
            xy = new bytes(64);
            for (uint256 i = 0; i < 64; i++) xy[i] = pubkey[i];
        } else if (pubkey.length == 33) {
            bytes memory uncompressed = decompressCompressedSecp256k1(pubkey);
            xy = new bytes(64);
            for (uint256 i = 0; i < 64; i++) xy[i] = uncompressed[i + 1];
        } else {
            revert InvalidPublicKey();
        }
    }

    function _pubkeyToXYMem(bytes memory pubkey)
        internal view returns (bytes memory xy) {
        if (pubkey.length == 65) {
            xy = new bytes(64);
            for (uint256 i = 0; i < 64; i++) xy[i] = pubkey[i + 1];
        } else if (pubkey.length == 64) {
            xy = new bytes(64);
            for (uint256 i = 0; i < 64; i++) xy[i] = pubkey[i];
        } else if (pubkey.length == 33) {
            bytes memory uncompressed = decompressCompressedSecp256k1Mem(pubkey);
            xy = new bytes(64);
            for (uint256 i = 0; i < 64; i++) xy[i] = uncompressed[i + 1];
        } else {
            revert InvalidPublicKey();
        }
    }

    function decompressCompressedSecp256k1(bytes calldata comp)
        internal view returns (bytes memory uncompressed) {
        if (comp.length != 33) revert InvalidPublicKey();
        uint8 prefix = uint8(comp[0]);
        if (prefix != 0x02 && prefix != 0x03) revert InvalidPublicKey();

        bytes32 x;
        assembly { x := calldataload(add(comp.offset, 1)) }

        uint256 p = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F;
        uint256 xUint = uint256(x);
        uint256 y2 = addmod(mulmod(xUint, mulmod(xUint, xUint, p), p), 7, p);
        uint256 y = modSqrt(y2, p);

        if (mulmod(y, y, p) != y2) revert PointNotOnCurve();
        if ((y & 1) != (prefix & 1)) y = p - y;

        uncompressed = new bytes(65);
        uncompressed[0] = 0x04;
        for (uint256 i = 0; i < 32; i++) uncompressed[i + 1] = comp[i + 1];
        for (uint256 i = 0; i < 32; i++) {
            uncompressed[i + 33] = bytes1(uint8(y >> (8 * (31 - i))));
        }
    }

    function decompressCompressedSecp256k1Mem(bytes memory comp)
        internal view returns (bytes memory uncompressed) {
        if (comp.length != 33) revert InvalidPublicKey();
        uint8 prefix = uint8(comp[0]);
        if (prefix != 0x02 && prefix != 0x03) revert InvalidPublicKey();

        bytes32 x;
        assembly { x := mload(add(comp, 33)) }

        uint256 p = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F;
        uint256 xUint = uint256(x);
        uint256 y2 = addmod(mulmod(xUint, mulmod(xUint, xUint, p), p), 7, p);
        uint256 y = modSqrt(y2, p);

        if (mulmod(y, y, p) != y2) revert PointNotOnCurve();
        if ((y & 1) != (prefix & 1)) y = p - y;

        uncompressed = new bytes(65);
        uncompressed[0] = 0x04;
        for (uint256 i = 0; i < 32; i++) uncompressed[i + 1] = comp[i + 1];
        for (uint256 i = 0; i < 32; i++) {
            uncompressed[i + 33] = bytes1(uint8(y >> (8 * (31 - i))));
        }
    }

    function modSqrt(uint256 a, uint256 p)
        internal view returns (uint256) {
        return expMod(a, (p + 1) / 4, p);
    }

    /// @dev V2: Uses modexp precompile (0x05) instead of software loop.
    ///      Saves ~50,000 gas per call on Arbitrum One.
    ///      Changed from pure to view (precompile requires staticcall).
    function expMod(uint256 base, uint256 exponent, uint256 modulus)
        internal view returns (uint256 result) {
        assembly {
            let ptr := mload(0x40)
            mstore(ptr, 0x20)
            mstore(add(ptr, 0x20), 0x20)
            mstore(add(ptr, 0x40), 0x20)
            mstore(add(ptr, 0x60), base)
            mstore(add(ptr, 0x80), exponent)
            mstore(add(ptr, 0xa0), modulus)
            if iszero(staticcall(gas(), 0x05, ptr, 0xc0, ptr, 0x20)) {
                revert(0, 0)
            }
            result := mload(ptr)
        }
    }

    function ethAddressFromXY(bytes memory xy)
        internal pure returns (address) {
        bytes32 h = keccak256(xy);
        return address(uint160(uint256(h)));
    }

    function btcHash160FromPubkey(bytes calldata pubkey)
        internal pure returns (bytes20) {
        bytes memory full;
        if (pubkey.length == 65) {
            full = new bytes(65);
            for (uint256 i = 0; i < 65; i++) full[i] = pubkey[i];
        } else if (pubkey.length == 64) {
            full = new bytes(65);
            full[0] = 0x04;
            for (uint256 i = 0; i < 64; i++) full[i + 1] = pubkey[i];
        } else if (pubkey.length == 33) {
            full = new bytes(33);
            for (uint256 i = 0; i < 33; i++) full[i] = pubkey[i];
        } else {
            revert InvalidPublicKey();
        }
        bytes32 sha = sha256(full);
        return ripemd160(abi.encodePacked(sha));
    }

    function btcHash160FromPubkeyMem(bytes memory pubkey)
        internal pure returns (bytes20) {
        bytes memory full;
        if (pubkey.length == 65) {
            full = new bytes(65);
            for (uint256 i = 0; i < 65; i++) full[i] = pubkey[i];
        } else if (pubkey.length == 64) {
            full = new bytes(65);
            full[0] = 0x04;
            for (uint256 i = 0; i < 64; i++) full[i + 1] = pubkey[i];
        } else if (pubkey.length == 33) {
            full = new bytes(33);
            for (uint256 i = 0; i < 33; i++) full[i] = pubkey[i];
        } else {
            revert InvalidPublicKey();
        }
        bytes32 sha = sha256(full);
        return ripemd160(abi.encodePacked(sha));
    }

    function toBitcoinSignedMessageHashMem(bytes memory message)
        internal pure returns (bytes32) {
        bytes memory data = abi.encodePacked(
            "\x18Bitcoin Signed Message:\n",
            _encodeCompactSize(message.length),
            message
        );
        bytes32 h1 = sha256(data);
        return sha256(abi.encodePacked(h1));
    }

    function toEthSignedMessageHashMem(bytes memory s)
        internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(
            "\x19Ethereum Signed Message:\n",
            _toString(s.length),
            s
        ));
    }

    function _encodeCompactSize(uint256 n)
        internal pure returns (bytes memory) {
        if (n < 253) {
            bytes memory out = new bytes(1);
            out[0] = bytes1(uint8(n));
            return out;
        }
        if (n <= type(uint16).max) {
            bytes memory out = new bytes(3);
            out[0] = 0xfd;
            out[1] = bytes1(uint8(n));
            out[2] = bytes1(uint8(n >> 8));
            return out;
        }
        if (n <= type(uint32).max) {
            bytes memory out = new bytes(5);
            out[0] = 0xfe;
            out[1] = bytes1(uint8(n));
            out[2] = bytes1(uint8(n >> 8));
            out[3] = bytes1(uint8(n >> 16));
            out[4] = bytes1(uint8(n >> 24));
            return out;
        }
        bytes memory out8 = new bytes(9);
        out8[0] = 0xff;
        out8[1] = bytes1(uint8(n));
        out8[2] = bytes1(uint8(n >> 8));
        out8[3] = bytes1(uint8(n >> 16));
        out8[4] = bytes1(uint8(n >> 24));
        out8[5] = bytes1(uint8(n >> 32));
        out8[6] = bytes1(uint8(n >> 40));
        out8[7] = bytes1(uint8(n >> 48));
        out8[8] = bytes1(uint8(n >> 56));
        return out8;
    }

    function _toString(uint256 value)
        internal pure returns (string memory) {
        if (value == 0) return "0";
        uint256 temp = value;
        uint256 digits;
        while (temp != 0) { digits++; temp /= 10; }
        bytes memory buffer = new bytes(digits);
        while (value != 0) {
            digits -= 1;
            buffer[digits] = bytes1(uint8(48 + uint256(value % 10)));
            value /= 10;
        }
        return string(buffer);
    }

    function toHex(address account) internal pure returns (string memory) {
        return toHex(abi.encodePacked(account));
    }

    function toHex(bytes memory data) internal pure returns (string memory) {
        bytes memory alphabet = "0123456789abcdef";
        bytes memory str = new bytes(2 + data.length * 2);
        str[0] = "0";
        str[1] = "x";
        for (uint256 i = 0; i < data.length; i++) {
            str[2 + i * 2] = alphabet[uint8(data[i] >> 4)];
            str[3 + i * 2] = alphabet[uint8(data[i] & 0x0f)];
        }
        return string(str);
    }

    receive() external payable {
        accumulatedFees += msg.value;
    }
}