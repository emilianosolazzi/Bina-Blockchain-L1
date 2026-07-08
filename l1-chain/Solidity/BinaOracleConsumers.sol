// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

import "./BinaOracle.sol";

/// @title BinaRaffle
/// @notice Reference integration: a raffle whose winner is picked from BINA
///         L1 randomness via commit-then-reveal (`requestUtility` /
///         `fulfillUtility`). Entries close before the target BINA height
///         is even mined, so neither entrants nor the relaying publisher(s)
///         can know or influence the winner at entry time.
/// @dev Reference only — not audited, no reentrancy guard beyond
///      checks-effects-interactions ordering, no fee-splitting/refund logic.
contract BinaRaffle {
    BinaOracle public immutable oracle;
    bytes32 public immutable purpose;
    uint256 public immutable entryFee;
    address public immutable organizer;

    address[] public entrants;
    mapping(address => bool) public entered;
    uint256 public requestId;
    uint64 public drawMinHeight;
    address public winner;
    bool public drawn;

    event Entered(address indexed entrant, uint256 pot);
    event DrawRequested(uint256 indexed requestId, uint64 minHeight);
    event WinnerPicked(address indexed winner, bytes32 utilityWord, uint256 payout);

    error AlreadyEntered();
    error WrongEntryFee();
    error DrawAlreadyRequested();
    error DrawNotRequested();
    error AlreadyDrawn();
    error NoEntrants();
    error TransferFailed();

    constructor(BinaOracle oracle_, bytes32 purpose_, uint256 entryFee_) {
        oracle = oracle_;
        purpose = purpose_ == bytes32(0) ? oracle_.PURPOSE_GAMING() : purpose_;
        entryFee = entryFee_;
        organizer = msg.sender;
    }

    function enter() external payable {
        if (msg.value != entryFee) revert WrongEntryFee();
        if (entered[msg.sender]) revert AlreadyEntered();
        if (requestId != 0) revert DrawAlreadyRequested();
        entered[msg.sender] = true;
        entrants.push(msg.sender);
        emit Entered(msg.sender, address(this).balance);
    }

    /// @notice Close entries and commit to a future BINA height for the
    ///         draw. Anyone can call once entries should close — the salt
    ///         is derived from the final entrant count so it can't be
    ///         chosen to favor a particular outcome.
    function requestDraw(uint64 minHeight) external returns (uint256) {
        if (requestId != 0) revert DrawAlreadyRequested();
        if (entrants.length == 0) revert NoEntrants();
        drawMinHeight = minHeight;
        bytes32 salt = keccak256(abi.encodePacked(address(this), entrants.length));
        requestId = oracle.requestUtility(purpose, salt, minHeight);
        emit DrawRequested(requestId, minHeight);
        return requestId;
    }

    /// @notice Fulfill the draw once BINA has published at/after minHeight.
    function drawWinner() external {
        if (requestId == 0) revert DrawNotRequested();
        if (drawn) revert AlreadyDrawn();
        bytes32 utilityWord = oracle.fulfillUtility(requestId);
        drawn = true;
        uint256 winnerIndex = uint256(utilityWord) % entrants.length;
        winner = entrants[winnerIndex];
        uint256 payout = address(this).balance;
        emit WinnerPicked(winner, utilityWord, payout);
        (bool ok, ) = winner.call{value: payout}("");
        if (!ok) revert TransferFailed();
    }

    function entrantCount() external view returns (uint256) {
        return entrants.length;
    }
}

/// @title BinaDice
/// @notice Reference integration: a 1-6 dice roll bound to a future BINA
///         height, so the result can't be known by the player or biased by
///         the publisher before the player commits to play.
/// @dev Reference only — no stake/payout logic, no house edge.
contract BinaDice {
    BinaOracle public immutable oracle;
    bytes32 public immutable purpose;

    struct Roll {
        address player;
        uint64 minHeight;
        bool resolved;
        uint8 result;
    }
    mapping(uint256 => Roll) public rolls;

    event RollRequested(uint256 indexed requestId, address indexed player, uint64 minHeight);
    event RollResolved(uint256 indexed requestId, address indexed player, uint8 result);

    error NotYourRoll();
    error AlreadyResolved();

    constructor(BinaOracle oracle_, bytes32 purpose_) {
        oracle = oracle_;
        purpose = purpose_ == bytes32(0) ? oracle_.PURPOSE_GAMING() : purpose_;
    }

    function play(uint64 minHeight) external returns (uint256 requestId) {
        bytes32 salt = keccak256(abi.encodePacked(msg.sender, block.number, minHeight));
        requestId = oracle.requestUtility(purpose, salt, minHeight);
        rolls[requestId] = Roll({player: msg.sender, minHeight: minHeight, resolved: false, result: 0});
        emit RollRequested(requestId, msg.sender, minHeight);
    }

    function resolve(uint256 requestId) external returns (uint8 result) {
        Roll storage roll = rolls[requestId];
        if (roll.player != msg.sender) revert NotYourRoll();
        if (roll.resolved) revert AlreadyResolved();
        bytes32 word = oracle.fulfillUtility(requestId);
        result = uint8((uint256(word) % 6) + 1);
        roll.resolved = true;
        roll.result = result;
        emit RollResolved(requestId, msg.sender, result);
    }
}

/// @title BinaValidatorSelector
/// @notice Reference integration for AI/validator-committee selection:
///         shuffles a candidate list using the oracle's latest randomness
///         for a chosen purpose (e.g. PURPOSE_VALIDATOR_SELECTION or
///         PURPOSE_AI) and derives a stable committee id.
/// @dev Uses the *already-published* latest seed rather than a future-height
///      commitment — fine for committee rotation where the candidate list
///      itself isn't chosen adaptively in response to the seed, but not a
///      substitute for `requestUtility`/`fulfillUtility` in adversarial
///      settings where that would matter.
contract BinaValidatorSelector {
    BinaOracle public immutable oracle;
    bytes32 public immutable purpose;

    event CommitteeSelected(bytes32 indexed committeeId, uint64 height, address[] committee);

    constructor(BinaOracle oracle_, bytes32 purpose_) {
        oracle = oracle_;
        purpose = purpose_ == bytes32(0) ? oracle_.PURPOSE_VALIDATOR_SELECTION() : purpose_;
    }

    function selectCommittee(address[] calldata candidates, uint256 committeeSize)
        external
        returns (address[] memory committee, bytes32 committeeId)
    {
        (bytes32 seed, uint64 height, , ) = oracle.getLatestSeed(purpose);
        address[] memory shuffled = oracle.shuffleValidators(seed, candidates);

        uint256 size = committeeSize < shuffled.length ? committeeSize : shuffled.length;
        committee = new address[](size);
        for (uint256 i = 0; i < size; i++) {
            committee[i] = shuffled[i];
        }

        committeeId = oracle.generateValidatorSetId(seed, size);
        emit CommitteeSelected(committeeId, height, committee);
    }
}
