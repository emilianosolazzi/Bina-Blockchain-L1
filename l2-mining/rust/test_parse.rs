use std::fs;

fn main() {
    let content = fs::read_to_string("C:/Users/comar/OneDrive/Documents/Entropy_Randomness/Randomness_Entropy/l2-mining/rust/package/keys/queue/approved/001f8280f42cc77025162c045c676c95fbcce83daf68f787efad456aebb3ff43.json").unwrap();
    let res: Result<serde_json::Value, _> = serde_json::from_str(&content);
    println!("{:?}", res);
}
