import requests
from web3 import Web3
from eth_account.messages import encode_defunct
from typing import List, Dict, Any, Optional, Union

class TemporalGradient:
    """
    Client for interacting with Temporal Gradient
    """
    
    def __init__(
        self,
        rpc_url: str,
        contract_address: Optional[str] = None,
        private_key: Optional[str] = None,
        api_key: Optional[str] = None,
        api_url: str = "https://api.temporalgradient.io"
    ):
        """
        Initialize the Temporal Gradient client
        
        Args:
            rpc_url: Ethereum RPC URL
            contract_address: Override default beacon contract address
            private_key: Private key for transactions
            api_key: API key for REST API access
            api_url: Override default API URL
        """
        self.w3 = Web3(Web3.HTTPProvider(rpc_url))
        self.contract_address = contract_address or "0x7F2b5E8c3D6A4F1c9E8D7B6A5F4E3D2C1B0A9F8E"
        self.contract = self.w3.eth.contract(address=self.contract_address, abi=BEACON_ABI)
        
        self.private_key = private_key
        self.account = self.w3.eth.account.from_key(private_key) if private_key else None
        
        self.api_key = api_key
        self.api_url = api_url
        self.headers = {"X-API-Key": api_key} if api_key else {}
    
    def get_latest_output(self) -> str:
        """
        Get the latest beacon output
        
        Returns:
            The latest beacon output as a hex string
        """
        return self.contract.functions.getLatestOutput().call()
    
    def get_beacon_state(self) -> Dict[str, Any]:
        """
        Get the current beacon state
        
        Returns:
            Dictionary containing the current beacon state
        """
        state = self.contract.functions.getBeaconState().call()
        return {
            "latestOutput": state[0].hex(),
            "currentDifficulty": state[1].hex(),
            "totalBlocks": state[2],
            "lastUpdateTime": state[3],
            "averageBlockTime": state[4]
        }
    
    def get_request_fee(self, num_words: int = 1) -> int:
        """
        Get the current fee for a randomness request
        
        Args:
            num_words: Number of random values
            
        Returns:
            The fee in wei
        """
        return self.contract.functions.getRequestFee(num_words).call()
    
    def get_randomness(self, num_words: int = 1, fee_multiplier: float = 1.0) -> Dict[str, Any]:
        """
        Request randomness from the beacon
        
        Args:
            num_words: Number of random values to generate
            fee_multiplier: Multiplier for the fee (for faster inclusion)
            
        Returns:
            Transaction receipt and randomness data
        """
        if not self.account:
            raise ValueError("Private key required for on-chain randomness requests")
        
        fee = self.get_request_fee(num_words)
        adjusted_fee = int(fee * fee_multiplier)
        
        tx = self.contract.functions.getRandomness(num_words).build_transaction({
            'from': self.account.address,
            'value': adjusted_fee,
            'gas': 200000,
            'nonce': self.w3.eth.get_transaction_count(self.account.address)
        })
        
        signed_tx = self.w3.eth.account.sign_transaction(tx, self.private_key)
        tx_hash = self.w3.eth.send_raw_transaction(signed_tx.rawTransaction)
        receipt = self.w3.eth.wait_for_transaction_receipt(tx_hash)
        
        # Extract randomness from logs
        event = self.contract.events.RandomnessRequested().process_receipt(receipt)[0]
        
        return {
            "transactionHash": receipt.transactionHash.hex(),
            "randomWords": [w.hex() for w in event.args.randomWords],
            "blockNumber": receipt.blockNumber
        }
    
    def generate_local_randomness(self, num_words: int = 1, seed: str = "") -> List[str]:
        """
        Generate randomness locally from the latest beacon output
        
        Args:
            num_words: Number of random values
            seed: Additional entropy
            
        Returns:
            List of random values as hex strings
        """
        output = self.get_latest_output()
        return self.derive_randomness(output, num_words, seed)
    
    def derive_randomness(self, beacon_output: str, num_words: int, seed: str = "") -> List[str]:
        """
        Derive randomness from a beacon output
        
        Args:
            beacon_output: Beacon output to use as source
            num_words: Number of random values
            seed: Additional entropy
            
        Returns:
            List of random values as hex strings
        """
        random_words = []
        for i in range(num_words):
            input_data = self.w3.solidity_keccak(
                ["bytes32", "string", "uint256"],
                [beacon_output, seed, i]
            )
            random_words.append(input_data.hex())
        
        return random_words
    
    def get_mining_challenge(self) -> Dict[str, Any]:
        """
        Get the current mining challenge
        
        Returns:
            Dictionary containing the current mining challenge
        """
        challenge = self.contract.functions.getMiningChallengeDetails().call()
        return {
            "previousOutput": challenge[0].hex(),
            "difficulty": challenge[1].hex(),
            "blockNumber": challenge[2],
            "timestamp": challenge[3]
        }
    
    def submit_mining_solution(self, solution: Dict[str, Any]) -> Dict[str, Any]:
        """
        Submit a mining solution
        
        Args:
            solution: Dictionary containing the mining solution
                - previousOutput: Previous beacon output hash
                - temporalSeed: Entropy seed used for mining
                - nonce: Nonce value for solution
                - signature: ECDSA signature
                - hmacOutput: HMAC output that meets difficulty
            
        Returns:
            Transaction receipt and mining event data
        """
        if not self.account:
            raise ValueError("Private key required for submitting mining solutions")
        
        # The contract now automatically includes msg.sender, block.prevrandao, and 
        # block.timestamp as part of the verification process
        tx = self.contract.functions.submitBeaconBlock(
            solution["previousOutput"],
            solution["temporalSeed"],
            solution["nonce"],
            solution["signature"],
            solution["hmacOutput"]
        ).build_transaction({
            'from': self.account.address,
            'gas': 300000,
            'nonce': self.w3.eth.get_transaction_count(self.account.address)
        })
        
        signed_tx = self.w3.eth.account.sign_transaction(tx, self.private_key)
        tx_hash = self.w3.eth.send_raw_transaction(signed_tx.rawTransaction)
        receipt = self.w3.eth.wait_for_transaction_receipt(tx_hash)
        
        # Extract event data
        event = self.contract.events.BeaconBlockMined().process_receipt(receipt)[0]
        
        return {
            "transactionHash": receipt.transactionHash.hex(),
            "output": event.args.output.hex(),
            "reward": event.args.reward,
            "blockNumber": receipt.blockNumber
        }
    
    def verify_solution(self, solution: Dict[str, Any]) -> bool:
        """
        Verify if a mining solution is valid
        
        Args:
            solution: Dictionary containing the mining solution
                - previousOutput: Previous beacon output hash
                - temporalSeed: Entropy seed used for mining
                - nonce: Nonce value for solution
                - signature: ECDSA signature
                - hmacOutput: HMAC output that meets difficulty
                - submitter: Address of solution submitter (required for verification)
            
        Returns:
            Whether the solution is valid
        """
        # Include the submitter address in the verification call
        # This is now required as the contract includes msg.sender in the hash
        if "submitter" not in solution:
            raise ValueError("Solution must include 'submitter' address for verification")
            
        return self.contract.functions.verifySolution(
            solution["previousOutput"],
            solution["temporalSeed"],
            solution["nonce"],
            solution["signature"],
            solution["hmacOutput"],
            solution["submitter"]  # Add submitter address parameter
        ).call()
        
    # Add helper method to create properly formatted mining input with on-chain data
    def prepare_mining_data(self, previous_output: str, temporal_seed: bytes, nonce: int) -> Dict[str, Any]:
        """
        Prepare mining data including simulated on-chain entropy
        
        Args:
            previous_output: Previous beacon output hash
            temporal_seed: Entropy seed bytes
            nonce: Nonce value for mining
            
        Returns:
            Dictionary with prepared mining data including simulated blockchain entropy
        """
        # Get latest block data to simulate on-chain environment
        block = self.w3.eth.get_block('latest')
        
        return {
            "previousOutput": previous_output,
            "temporalSeed": temporal_seed,
            "nonce": nonce,
            "submitter": self.account.address if self.account else None,
            # Simulated values that will be added by the contract
            "blockPrevrandao": block.get('prevrandao', block.get('mixHash', '0x0')),
            "timestamp": block.timestamp
        }
    
    def get_token_info(self) -> Dict[str, Any]:
        """
        Get TGBT token information
        
        Returns:
            Dictionary containing token information
        """
        response = requests.get(f"{self.api_url}/api/v1/token/info", headers=self.headers)
        response.raise_for_status()
        return response.json()
    
    def get_token_balance(self, address: str) -> Dict[str, Any]:
        """
        Get TGBT balance for an address
        
        Args:
            address: Ethereum address
            
        Returns:
            Dictionary containing balance information
        """
        response = requests.get(f"{self.api_url}/api/v1/token/balance/{address}", headers=self.headers)
        response.raise_for_status()
        return response.json()
