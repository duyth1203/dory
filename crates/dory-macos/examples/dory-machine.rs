//! Prints what `MacMachine` reads on this Mac.

use dory_core::ports::Machine;
use dory_macos::MacMachine;

fn main() {
    println!("conditions: {:?}", MacMachine::new().conditions());
}
