#![feature(rustc_private)]
use std::env;
pub mod driver;
pub mod printer;
pub mod kani_lib;
use driver::stable_mir_driver;
use printer::emit_smir;

fn main() {
    let args: Vec<_> = env::args().collect();
    stable_mir_driver(&args, emit_smir)
}
