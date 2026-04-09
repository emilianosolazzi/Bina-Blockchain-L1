// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import { Script, console2 } from "forge-std/Script.sol";
import { TGL3Treasury } from "../contracts/l3/TGL3Treasury.sol";
import { TGL3EpochSettlement } from "../contracts/l3/TGL3EpochSettlement.sol";
import { TGL3ProofMarket } from "../contracts/l3/TGL3ProofMarket.sol";
import { TGL3CertificateRegistry } from "../contracts/l3/TGL3CertificateRegistry.sol";

contract DeployL3SettlementScaffoldScript is Script {
    function run()
        external
        returns (
            address treasuryAddress,
            address epochSettlementAddress,
            address proofMarketAddress,
            address certificateRegistryAddress
        )
    {
        uint256 deployerPrivateKey = vm.envUint("DEPLOYER_PRIVATE_KEY");
        address admin = vm.envAddress("L3_ADMIN_ADDRESS");
        address paymentToken = vm.envAddress("L3_PAYMENT_TOKEN");
        address protocolTreasury = vm.envAddress("L3_PROTOCOL_TREASURY");
        address operatorTreasury = vm.envAddress("L3_OPERATOR_TREASURY");
        address initialOperator = vm.envAddress("L3_INITIAL_OPERATOR");
        address initialIssuer = vm.envAddress("L3_INITIAL_ISSUER");

        uint16 protocolShareBps = uint16(vm.envOr("L3_PROTOCOL_SHARE_BPS", uint256(3_000)));
        uint256 challengeWindowSeconds = vm.envOr("L3_CHALLENGE_WINDOW_SECONDS", uint256(3_600));
        uint256 standardProofFee = vm.envOr("L3_STANDARD_PROOF_FEE", uint256(5 ether));
        uint256 anchoredProofFee = vm.envOr("L3_ANCHORED_PROOF_FEE", uint256(15 ether));
        uint256 enterpriseProofFee = vm.envOr("L3_ENTERPRISE_PROOF_FEE", uint256(50 ether));
        uint256 certificateIssuanceFee = vm.envOr("L3_CERTIFICATE_ISSUANCE_FEE", uint256(25 ether));

        vm.startBroadcast(deployerPrivateKey);

        TGL3Treasury treasury = new TGL3Treasury(
            admin,
            paymentToken,
            protocolTreasury,
            operatorTreasury,
            protocolShareBps
        );

        TGL3EpochSettlement epochSettlement = new TGL3EpochSettlement(admin, challengeWindowSeconds);
        TGL3ProofMarket proofMarket = new TGL3ProofMarket(
            admin,
            address(epochSettlement),
            address(treasury),
            standardProofFee,
            anchoredProofFee,
            enterpriseProofFee
        );
        TGL3CertificateRegistry certificateRegistry = new TGL3CertificateRegistry(
            admin,
            address(epochSettlement),
            address(treasury),
            certificateIssuanceFee
        );

        treasury.setCollector(address(proofMarket), true);
        treasury.setCollector(address(certificateRegistry), true);
        epochSettlement.setOperator(initialOperator, true);
        certificateRegistry.setIssuer(initialIssuer, true);

        vm.stopBroadcast();

        console2.log("L3 settlement scaffold deployed");
        console2.log("Treasury:", address(treasury));
        console2.log("EpochSettlement:", address(epochSettlement));
        console2.log("ProofMarket:", address(proofMarket));
        console2.log("CertificateRegistry:", address(certificateRegistry));

        return (
            address(treasury),
            address(epochSettlement),
            address(proofMarket),
            address(certificateRegistry)
        );
    }
}