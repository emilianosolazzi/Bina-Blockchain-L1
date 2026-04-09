// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

/// @title TGL3Treasury
/// @notice Minimal devnet treasury helper for first-wave L3 settlement flows.
/// @dev Scaffold only. Intended for Orbit devnet bring-up, not production use.
contract TGL3Treasury is Ownable {
    uint16 public constant BPS_SCALE = 10_000;

    IERC20 public immutable paymentToken;

    address public protocolTreasury;
    address public operatorTreasury;
    uint16 public protocolShareBps;

    mapping(address => bool) public collectors;

    error ZeroAddress();
    error InvalidBps();
    error CollectorNotAuthorized();
    error InvalidAmount();
    error TransferFailed();

    event CollectorUpdated(address indexed collector, bool allowed);
    event DistributionUpdated(address indexed protocolTreasury, address indexed operatorTreasury, uint16 protocolShareBps);
    event PaymentCollected(
        address indexed collector,
        address indexed payer,
        bytes32 indexed paymentReference,
        uint256 grossAmount,
        uint256 protocolAmount,
        uint256 operatorAmount
    );

    constructor(
        address initialOwner,
        address paymentTokenAddress,
        address protocolTreasuryAddress,
        address operatorTreasuryAddress,
        uint16 protocolShare
    ) Ownable(initialOwner) {
        if (paymentTokenAddress == address(0) || protocolTreasuryAddress == address(0) || operatorTreasuryAddress == address(0)) {
            revert ZeroAddress();
        }
        if (protocolShare > BPS_SCALE) revert InvalidBps();

        paymentToken = IERC20(paymentTokenAddress);
        protocolTreasury = protocolTreasuryAddress;
        operatorTreasury = operatorTreasuryAddress;
        protocolShareBps = protocolShare;
    }

    function setCollector(address collector, bool allowed) external onlyOwner {
        if (collector == address(0)) revert ZeroAddress();
        collectors[collector] = allowed;
        emit CollectorUpdated(collector, allowed);
    }

    function updateDistribution(
        address newProtocolTreasury,
        address newOperatorTreasury,
        uint16 newProtocolShareBps
    ) external onlyOwner {
        if (newProtocolTreasury == address(0) || newOperatorTreasury == address(0)) revert ZeroAddress();
        if (newProtocolShareBps > BPS_SCALE) revert InvalidBps();

        protocolTreasury = newProtocolTreasury;
        operatorTreasury = newOperatorTreasury;
        protocolShareBps = newProtocolShareBps;

        emit DistributionUpdated(newProtocolTreasury, newOperatorTreasury, newProtocolShareBps);
    }

    function collectPaymentFrom(address payer, uint256 amount, bytes32 paymentReference)
        external
        returns (uint256 protocolAmount, uint256 operatorAmount)
    {
        if (!collectors[msg.sender]) revert CollectorNotAuthorized();
        if (amount == 0) revert InvalidAmount();

        bool pulled = paymentToken.transferFrom(payer, address(this), amount);
        if (!pulled) revert TransferFailed();

        protocolAmount = (amount * protocolShareBps) / BPS_SCALE;
        operatorAmount = amount - protocolAmount;

        if (protocolAmount > 0) {
            bool protocolPaid = paymentToken.transfer(protocolTreasury, protocolAmount);
            if (!protocolPaid) revert TransferFailed();
        }
        if (operatorAmount > 0) {
            bool operatorPaid = paymentToken.transfer(operatorTreasury, operatorAmount);
            if (!operatorPaid) revert TransferFailed();
        }

        emit PaymentCollected(msg.sender, payer, paymentReference, amount, protocolAmount, operatorAmount);
    }
}