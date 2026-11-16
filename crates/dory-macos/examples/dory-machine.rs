//! Prints what `MacMachine` reads on this Mac, and the QoS class a pace leaves on a thread and
//! on a thread it then creates.

use dory_core::ports::{Machine, Pace};
use dory_macos::{MacMachine, thread_qos};

fn main() {
    let machine = MacMachine::new();
    println!("conditions: {:?}", machine.conditions());
    println!("qos before: {:#04x}", thread_qos());
    for pace in [Pace::Light, Pace::Fast] {
        machine.set_pace(pace);
        let child = std::thread::spawn(thread_qos).join().unwrap_or_default();
        println!(
            "after {pace:?}: thread {:#04x}, new thread {child:#04x}",
            thread_qos()
        );
    }
}
