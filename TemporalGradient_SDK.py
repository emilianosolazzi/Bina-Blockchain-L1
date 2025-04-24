import requests
from web3 import Web3
from eth_account.messages import encode_defunct
from typing import List, Dict, Any, Optional, Union
import time # Added for polling

# Assume BEACON_ABI is defined correctly based on TemporalGradientBeacon.sol and its libraries
# Example placeholder - REPLACE WITH ACTUAL ABI
BEACON_ABI = """
[
    { "inputs": [], "name": "currentOutputIndex", "outputs": [ { "internalType": "uint64", "name": "", "type": "uint64" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [ { "internalType": "uint256", "name": "", "type": "uint256" } ], "name": "outputHistory", "outputs": [ { "internalType": "bytes32", "name": "", "type": "bytes32" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [], "name": "targetDifficulty", "outputs": [ { "internalType": "uint256", "name": "", "type": "uint256" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [], "name": "outputCount", "outputs": [ { "internalType": "uint64", "name": "", "type": "uint64" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [], "name": "lastOutputTimestamp", "outputs": [ { "internalType": "uint64", "name": "", "type": "uint64" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [], "name": "randomnessState", "outputs": [ { "internalType": "uint256", "name": "fee", "type": "uint256" }, { "internalType": "uint64", "name": "expiryBlocks", "type": "uint64" }, { "internalType": "uint8", "name": "minContributions", "type": "uint8" }, { "internalType": "uint8", "name": "maxContributions", "type": "uint8" }, { "internalType": "uint256", "name": "entropyAccumulator", "type": "uint256" }, { "internalType": "bytes32", "name": "entropyMerkleRoot", "type": "bytes32" }, { "internalType": "uint256", "name": "requestCount", "type": "uint256" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [ { "internalType": "bytes32", "name": "userSeed", "type": "bytes32" } ], "name": "requestRandomness", "outputs": [ { "internalType": "uint256", "name": "requestId", "type": "uint256" } ], "stateMutability": "payable", "type": "function" },
    { "inputs": [ { "internalType": "uint256", "name": "requestId", "type": "uint256" } ], "name": "getRandomnessResult", "outputs": [ { "internalType": "bytes32", "name": "", "type": "bytes32" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [ { "internalType": "uint8", "name": "poolId", "type": "uint8" } ], "name": "getMiningChallenge", "outputs": [ { "internalType": "bytes32[]", "name": "outputs", "type": "bytes32[]" }, { "internalType": "uint256", "name": "difficulty", "type": "uint256" } ], "stateMutability": "view", "type": "function" },
    { "inputs": [ { "internalType": "bytes32", "name": "previousOutput", "type": "bytes32" }, { "internalType": "bytes", "name": "temporalSeed", "type": "bytes" }, { "internalType": "uint64", "name": "nonce", "type": "uint64" }, { "internalType": "bytes", "name": "signature", "type": "bytes" }, { "internalType": "bytes32", "name": "secretValue", "type": "bytes32" }, { "internalType": "uint8", "name": "poolId", "type": "uint8" } ], "name": "revealMiningCommitment", "outputs": [], "stateMutability": "nonpayable", "type": "function" },
    { "anonymous": false, "inputs": [ { "indexed": true, "internalType": "uint256", "name": "requestId", "type": "uint256" }, { "indexed": true, "internalType": "address", "name": "requester", "type": "address" }, { "indexed": false, "internalType": "bytes32", "name": "userSeed", "type": "bytes32" } ], "name": "RandomnessRequested", "type": "event" },
    { "anonymous": false, "inputs": [ { "indexed": true, "internalType": "uint256", "name": "requestId", "type": "uint256" }, { "indexed": false, "internalType": "bytes32", "name": "result", "type": "bytes32" } ], "name": "RandomnessFulfilled", "type": "event" },
    { "anonymous": false, "inputs": [ { "indexed": true, "internalType": "address", "name": "miner", "type": "address" }, { "indexed": false, "internalType": "bytes32", "name": "hmacOutput", "type": "bytes32" }, { "indexed": false, "internalType": "uint256", "name": "reward", "type": "uint256" }, { "indexed": false, "internalType": "uint64", "name": "nonce", "type": "uint64" }, { "indexed": false, "internalType": "uint64", "name": "timestamp", "type": "uint64" }, { "indexed": false, "internalType": "uint8", "name": "poolId", "type": "uint8" } ], "name": "BeaconBlockMined", "type": "event" }
]
"""

class TemporalGradient:
    """
    Client for interacting with Temporal Gradient Beacon
    """
    # ... existing __init__ code ...

    def get_latest_output(self) -> str:
        """
        Get the latest beacon output hash.
        Reads the current index and retrieves the corresponding hash from the history.

        Returns:
            The latest beacon output as a hex string.
        """
        index = self.contract.functions.currentOutputIndex().call()
        output_hash = self.contract.functions.outputHistory(index).call()
        return output_hash.hex()

    def get_beacon_state(self) -> Dict[str, Any]:
        """
        Get the current beacon state by calling multiple view functions.

        Returns:
            Dictionary containing parts of the current beacon state.
        """
        # Note: This is a simplified state representation. Add more calls as needed.
        latest_output_index = self.contract.functions.currentOutputIndex().call()
        latest_output_hash = self.contract.functions.outputHistory(latest_output_index).call()
        # Assuming pool 0 for difficulty, make this configurable if needed
        _, difficulty = self.contract.functions.getMiningChallenge(0).call()
        output_count = self.contract.functions.outputCount().call()
        last_ts = self.contract.functions.lastOutputTimestamp().call()

        return {
            "latestOutput": latest_output_hash.hex(),
            "currentDifficulty": difficulty, # Difficulty for pool 0
            "totalOutputs": output_count,
            "lastOutputTimestamp": last_ts,
            # "averageBlockTime": Needs calculation based on history or API
        }

    def get_randomness_fee(self) -> int:
        """
        Get the current fee for a randomness request (fee is per request, not per word).

        Returns:
            The fee in wei.
        """
        # Reads the fee directly from the randomnessState struct view function
        state = self.contract.functions.randomnessState().call()
        # state tuple fields: fee, expiryBlocks, minContributions, maxContributions, etc.
        return state[0] # Index 0 corresponds to 'fee'

    def request_randomness(self, user_seed: Union[bytes, str], fee_multiplier: float = 1.0) -> Dict[str, Any]:
        """
        Request randomness from the beacon. This is the first step.
        Use poll_randomness_result to get the result later.

        Args:
            user_seed: User-provided entropy seed (bytes32 as bytes or hex string).
            fee_multiplier: Multiplier for the fee (for faster inclusion).

        Returns:
            Dictionary containing transaction hash and request ID.
        """
        if not self.account:
            raise ValueError("Private key required for on-chain randomness requests")

        fee = self.get_randomness_fee()
        adjusted_fee = int(fee * fee_multiplier)

        if isinstance(user_seed, str):
            user_seed_bytes = bytes.fromhex(user_seed.replace("0x", ""))
        else:
            user_seed_bytes = user_seed

        if len(user_seed_bytes) != 32:
             raise ValueError("user_seed must be 32 bytes long")

        tx = self.contract.functions.requestRandomness(user_seed_bytes).build_transaction({
            'from': self.account.address,
            'value': adjusted_fee,
            'gas': 200000, # Adjust gas limit as needed
            'nonce': self.w3.eth.get_transaction_count(self.account.address)
        })

        signed_tx = self.w3.eth.account.sign_transaction(tx, self.private_key)
        tx_hash = self.w3.eth.send_raw_transaction(signed_tx.rawTransaction)
        receipt = self.w3.eth.wait_for_transaction_receipt(tx_hash)

        # Extract request ID from logs
        try:
            event = self.contract.events.RandomnessRequested().process_receipt(receipt)[0]
            request_id = event.args.requestId
        except (IndexError, Exception) as e:
             raise RuntimeError(f"Could not find RandomnessRequested event in receipt: {e}")

        return {
            "transactionHash": receipt.transactionHash.hex(),
            "requestId": request_id,
            "blockNumber": receipt.blockNumber
        }

    def poll_randomness_result(self, request_id: int, timeout_secs: int = 120, interval_secs: int = 5) -> Optional[str]:
        """
        Polls the contract for the result of a randomness request.

        Args:
            request_id: The ID of the randomness request.
            timeout_secs: Maximum time to wait for the result.
            interval_secs: Time between polling attempts.

        Returns:
            The random value as a hex string if fulfilled within timeout, otherwise None.
        """
        start_time = time.time()
        while time.time() - start_time < timeout_secs:
            try:
                # Check if fulfilled using getRandomnessResult (which reverts if not fulfilled)
                # or potentially add a getRequestState view function to the contract
                result = self.contract.functions.getRandomnessResult(request_id).call()
                # Result is bytes32(0) if request exists but not fulfilled, need contract change or event check
                # For now, assume non-zero means fulfilled (needs contract adjustment for robustness)
                if result != b'\x00' * 32:
                     return result.hex()
            except Exception as e:
                # Handle specific exceptions like contract logic errors if needed
                print(f"Polling randomness result: {e}") # Log error or pass
                pass # Continue polling if view call reverts (e.g., not fulfilled yet)

            time.sleep(interval_secs)

        print(f"Timeout waiting for randomness result for request ID {request_id}")
        return None


    # ... existing generate_local_randomness / derive_randomness code ...
    # Note: derive_randomness depends on the specific derivation logic desired.
    # The current implementation using keccak256 is plausible.

    def get_mining_challenge(self, pool_id: int = 0) -> Dict[str, Any]:
        """
        Get the current mining challenge for a specific pool.

        Args:
            pool_id: The ID of the mining pool (default: 0).

        Returns:
            Dictionary containing the mining challenge details.
        """
        outputs, difficulty = self.contract.functions.getMiningChallenge(pool_id).call()
        # The contract returns the full history; typically only the latest is needed as previousOutput
        previous_output = outputs[-1] if outputs else None # Get the last output from history

        return {
            "previousOutput": previous_output.hex() if previous_output else None,
            "difficulty": difficulty,
            "outputHistory": [o.hex() for o in outputs] # Full history might be useful contextually
            # blockNumber and timestamp are not returned by this contract function
        }

    def submit_mining_commitment(self, commit_hash: Union[bytes, str], pool_id: int = 0) -> Dict[str, Any]:
        """
        Submits a mining commitment hash to the contract.

        Args:
            commit_hash: The keccak256 hash of the commitment parameters (bytes32 as bytes or hex string).
            pool_id: The ID of the mining pool.

        Returns:
            Transaction receipt.
        """
        if not self.account:
            raise ValueError("Private key required for submitting mining commitments")

        if isinstance(commit_hash, str):
            commit_hash_bytes = bytes.fromhex(commit_hash.replace("0x", ""))
        else:
            commit_hash_bytes = commit_hash

        if len(commit_hash_bytes) != 32:
             raise ValueError("commit_hash must be 32 bytes long")

        tx = self.contract.functions.submitMiningCommitment(commit_hash_bytes, pool_id).build_transaction({
            'from': self.account.address,
            'gas': 150000, # Adjust gas limit as needed
            'nonce': self.w3.eth.get_transaction_count(self.account.address)
        })

        signed_tx = self.w3.eth.account.sign_transaction(tx, self.private_key)
        tx_hash = self.w3.eth.send_raw_transaction(signed_tx.rawTransaction)
        receipt = self.w3.eth.wait_for_transaction_receipt(tx_hash)

        return {"transactionHash": receipt.transactionHash.hex(), "blockNumber": receipt.blockNumber}


    def reveal_mining_commitment(self, reveal_params: Dict[str, Any]) -> Dict[str, Any]:
        """
        Reveals a mining commitment and potentially mines a block.

        Args:
            reveal_params: Dictionary containing the reveal parameters:
                - previousOutput: Previous output hash (bytes32 hex string or bytes)
                - temporalSeed: Temporal seed used (bytes hex string or bytes)
                - nonce: Miner's nonce (int)
                - signature: ECDSA signature (bytes hex string or bytes)
                - secretValue: Secret value used in commitment (bytes32 hex string or bytes)
                - poolId: Mining pool ID (int)

        Returns:
            Transaction receipt and mining event data if successful.
        """
        if not self.account:
            raise ValueError("Private key required for revealing mining commitments")

        # --- Type Conversions and Validation ---
        def to_bytes(value: Union[str, bytes], length: Optional[int] = None) -> bytes:
            if isinstance(value, str):
                b = bytes.fromhex(value.replace("0x", ""))
            else:
                b = value
            if length is not None and len(b) != length:
                raise ValueError(f"Expected {length} bytes, got {len(b)}")
            return b

        try:
            prev_output_bytes = to_bytes(reveal_params["previousOutput"], 32)
            temporal_seed_bytes = to_bytes(reveal_params["temporalSeed"]) # No fixed length
            nonce_int = int(reveal_params["nonce"])
            signature_bytes = to_bytes(reveal_params["signature"]) # No fixed length for DER sig
            secret_value_bytes = to_bytes(reveal_params["secretValue"], 32)
            pool_id_int = int(reveal_params["poolId"])
        except KeyError as e:
            raise ValueError(f"Missing reveal parameter: {e}")
        except (ValueError, TypeError) as e:
            raise ValueError(f"Invalid reveal parameter format: {e}")
        # --- End Type Conversions ---


        tx = self.contract.functions.revealMiningCommitment(
            prev_output_bytes,
            temporal_seed_bytes,
            nonce_int,
            signature_bytes,
            secret_value_bytes,
            pool_id_int
        ).build_transaction({
            'from': self.account.address,
            'gas': 500000, # Adjust gas limit - revealing is more complex
            'nonce': self.w3.eth.get_transaction_count(self.account.address)
        })

        signed_tx = self.w3.eth.account.sign_transaction(tx, self.private_key)
        tx_hash = self.w3.eth.send_raw_transaction(signed_tx.rawTransaction)
        receipt = self.w3.eth.wait_for_transaction_receipt(tx_hash)

        # Extract event data if block was mined
        try:
            event = self.contract.events.BeaconBlockMined().process_receipt(receipt)[0]
            mined_event_data = {
                "output": event.args.hmacOutput.hex(), # Event uses hmacOutput
                "reward": event.args.reward,
                "miner": event.args.miner,
                "nonce": event.args.nonce,
                "poolId": event.args.poolId,
            }
        except IndexError:
            mined_event_data = None # No block mined in this reveal

        return {
            "transactionHash": receipt.transactionHash.hex(),
            "blockNumber": receipt.blockNumber,
            "minedBlockEvent": mined_event_data
        }

    # Removed verify_solution as it's not an external contract function

    # Removed prepare_mining_data as it's less relevant with commit/reveal

    # ... existing API call methods (get_token_info, get_token_balance) ...
    # These depend on the external API implementation.
