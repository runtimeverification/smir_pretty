# Rust Stable MIR Pretty Printing

This package provides a program that will emit a JSON serialisation of the Stable MIR of a Rust program

## Building

```shell
cargo build
```

NOTE: requries [rustup](https://www.rust-lang.org/tools/install)

The `build.rs` script will ensure that the correct version of rust and the required components are installed and defaulted. What `rustup` commands are run can be seen by adding verbosity flag `-vv` to `cargo`.

## Usage

Use the wrapper script `run.sh` (or `cargo run`, but this may also initiate a build).
The options that this tool accepts are identical to `rustc`.
To generate stable MIR output without building a binary, you can invoke the tool as follows:

```shell
cargo run -- <rustc_flags> <path_from_crate_root>
```

There are a few environment variables that can be set to control the tools output:

1.  `LINK_ITEMS` - add entries to the link-time `functions` map for each monomorphic item in the crate;
2.  `LINK_INST`  - use a richer key-structure for the link-time `functions` map which uses keys that are pairs of a function type (`Ty`) _and_ an function instance kind (`InstanceKind`)
3.  `DEBUG` - serialize additional data in the JSON file and dump logs to stdout

## Tests

### Running the Tests

To run the tests, do the following:

```shell
make generate_ui_tests
```

This will generate four outputs:

| Path                              | Comment                                                   |
| ---                               | ---                                                       |
| `deps/rust/tests/ui/upstream`     | Upstream `rustc` test outputs                             |
| `deps/rust/tests_ui_upstream.log` | Upstream test log                                         |
| `deps/rust/tests/ui/smir`         | `smir_pretty` test outputs (including `.smir.json` files) |
| `deps/rust/tests_ui_smir.log`     | `smir_pretty` test log                                    |

### Test Rationale

Since this crate is a Stable MIR serialization tool, there are two main features we are interested in:

1.  the serialization facilities should be stable (i.e. not crash)
2.  the serialized output should be correct

Since this tool is currently in its early stages, it is hard to test (2).
However, to test (1) and to make progress towards (2), we currently do the following:

1.  in the rustc test suite, we gather all of the run-pass tests, i.e., tests where the compiler is able to generate a binary _and_ subsequently execute the binary such that it exits successfully
2.  we extract the test runner invocation from the `x.py test` command
3.  we execute the test runner with upstream `rustc` against the test inputs from (1) --- this gives us a baseline on which tests should pass/fail
4.  we re-execute the test runner but use our wrapper binary against the test inputs from (1) --- this generates the corresponding `.smir.json` files and shows us where any regressions occur


**NOTE:** In order to speed up test time, we setup the test runner, by default, such that it skips codegen and compiler-generated binary execution.  
**NOTE:** Points (1,4) also means that our test _outputs_ from this phase can become test _inputs_ for KMIR.
