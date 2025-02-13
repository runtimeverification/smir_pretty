#![feature(rustc_private)]
use std::env;
pub mod driver;
pub mod printer;
use driver::stable_mir_driver;
use printer::emit_smir;
use stable_mir_json::mk_graph::emit_dotfile;

fn main() {
    let mut args: Vec<String> = env::args().collect();

    match args.get(1) {
        None => stable_mir_driver(&args, emit_smir), // backward compatibility
        Some(arg) if arg == "--json" => {
            args.remove(1);
            stable_mir_driver(&args, emit_smir)
        }
        Some(arg) if arg == "--dot" => {
            args.remove(1);
            stable_mir_driver(&args, emit_dotfile)
        }
        Some(_other) => stable_mir_driver(&args, emit_smir), // backward compatibility
    }
}
