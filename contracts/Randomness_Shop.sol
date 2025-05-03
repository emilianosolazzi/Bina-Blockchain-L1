// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Initializable } from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import { Ownable2StepUpgradeable } from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import { UUPSUpgradeable } from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import { IERC20Upgradeable } from "@openzeppelin/contracts-upgradeable/token/ERC20/IERC20Upgradeable.sol";
import { ITGBT } from "./interfaces/ITGBT.sol";
import { Address } from "@openzeppelin/contracts/utils/Address.sol";

/**
 * @title RandomnessShop
 * @notice Buy TGBT with USDC at a fixed exchange rate. USDC is stored in contract treasury.
 */
contract RandomnessShop is Initializable, Ownable2StepUpgradeable, UUPSUpgradeable {
    using Address for address;

    IERC20Upgradeable public usdc;
    ITGBT public tgbt;

    uint256 public exchangeRate; // How many TGBT per 1 USDC (with 18 decimals)
    address public treasury;     // Address that owns collected USDC

    event TokensPurchased(address indexed buyer, uint256 usdcIn, uint256 tgbtOut);
    event ExchangeRateUpdated(uint256 oldRate, uint256 newRate);
    event TreasuryUpdated(address oldTreasury, address newTreasury);
    event FundsWithdrawn(address token, uint256 amount, address to);

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    function initialize(
        address _usdc,
        address _tgbt,
        uint256 _exchangeRate,
        address _treasury
    ) public initializer {
        require(_usdc != address(0), "USDC required");
        require(_tgbt != address(0), "TGBT required");
        require(_exchangeRate > 0, "Zero rate");
        require(_treasury != address(0), "Treasury required");

        __Ownable2Step_init();
        __UUPSUpgradeable_init();

        usdc = IERC20Upgradeable(_usdc);
        tgbt = ITGBT(_tgbt);
        exchangeRate = _exchangeRate;
        treasury = _treasury;
    }

    function buyTGBT(uint256 usdcAmount) external {
        require(usdcAmount > 0, "Zero input");
        require(usdc.allowance(msg.sender, address(this)) >= usdcAmount, "Insufficient allowance");

        // Transfer USDC from buyer to this contract
        require(usdc.transferFrom(msg.sender, address(this), usdcAmount), "USDC transfer failed");

        // Direct mint instead of pulling from liquidity pool
        uint256 tgbtAmount = (usdcAmount * exchangeRate);
        tgbt.mint(msg.sender, tgbtAmount);

        emit TokensPurchased(msg.sender, usdcAmount, tgbtAmount);
    }

    // --- Admin Functions ---

    function setExchangeRate(uint256 newRate) external onlyOwner {
        require(newRate > 0, "Invalid rate");
        emit ExchangeRateUpdated(exchangeRate, newRate);
        exchangeRate = newRate;
    }

    function setTreasury(address newTreasury) external onlyOwner {
        require(newTreasury != address(0), "Zero address");
        emit TreasuryUpdated(treasury, newTreasury);
        treasury = newTreasury;
    }

    function withdrawFunds(address token, uint256 amount) external onlyOwner {
        require(amount > 0, "Zero amount");
        IERC20Upgradeable(token).transfer(treasury, amount);
        emit FundsWithdrawn(token, amount, treasury);
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyOwner {}
}

// ANALYSIS: Token Distribution Economics for Different Miner Scales
// With MINING_ALLOCATION of 700,000,000 TGBT tokens from TemporalGradientBeacon:
//
// 1,000 Miners (Current MAX_MINER_COUNT):
// - 700,000 TGBT per miner average
// - ~$70,000-$140,000 per miner at current rates
// - Attractive ROI potential while maintaining decentralization
//
// 10,000 Miners (10x Scaling Analysis):
// - 70,000 TGBT per miner average
// - ~$7,000-$14,000 per miner at current rates
// - More decentralized, still viable for professional operations
// - Would require increasing MAX_MINER_COUNT constant in MiningLib.sol
// - BloomFilter implementation already scales to 10M+ entities
//
// Recommendation: 2,500-5,000 miners represents optimal balance between:
// - Sufficient decentralization for quantum-resistant security
// - Economically attractive mining rewards to maintain participation
// - Manageable computational overhead for consensus operations

