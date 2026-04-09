#![cfg_attr(not(any(test, feature = "export-abi")), no_main)]
extern crate alloc;

use alloy_primitives::U256;
use stylus_sdk::prelude::*;

sol_storage! {
    #[entrypoint]
    pub struct Counter {
        uint256 number;
    }
}

#[public]
impl Counter {
    pub fn number(&self) -> U256 {
        self.number.get()
    }

    pub fn set_number(&mut self, new_number: U256) {
        self.number.set(new_number);
    }

    pub fn increment(&mut self) {
        let current = self.number.get();
        self.number.set(current + U256::from(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stylus_sdk::testing::*;

    #[test]
    fn counter_increments() {
        let vm = TestVM::default();
        let mut contract = Counter::from(&vm);

        assert_eq!(contract.number(), U256::ZERO);
        contract.increment();
        assert_eq!(contract.number(), U256::from(1));
    }
}
