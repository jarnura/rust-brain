//! rustbrain-validator — evaluate rust-brain agent code generation accuracy.
//!
//! Usage: `validator validate <repo> <pr_number> [--runs 2] [--inverted]`

pub mod judge;
pub mod models;
pub mod scorer;

fn main() {
    println!("rustbrain-validator: use the library API from judge.rs and scorer.rs");
}
