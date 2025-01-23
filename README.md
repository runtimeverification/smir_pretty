# Rust Stable MIR Pretty Printing

This package provides:

1.  a library crate that provides:
    -   a `rustc` compiler wrapper which can access stable MIR APIs
    -   a pretty-printer for a large fragment of stable MIR
2.  a `rustc` wrapper binary that uses (1)-(2) to pretty-print Rust source files as stable MIR using the `.smir.json` extension.

It is designed so that anyone can use this library crate as a jumping off point for their own tools which might use stable MIR APIs.

## Building

For first-time builds, run:

```shell
make setup build_all
```

If the underlying `rustc` branch is updated and this crate needs to be rebuilt on top of it, run:

```shell
make update build_all
```

If the source code changes locally for this crate only and it needs to be rebuilt, run:

```shell
make build
```

## Usage

Use the wrapper script `run.sh` (or `cargo run`, but this may also initiate a build).
The options that this tool accepts are identical to `rustc`.
To generate stable MIR output without building a binary, you can invoke the tool as follows:

```shell
./run.sh -Z no-codegen <crate_root>
```

There are a few environment variables that can be set to control the tools output:

1.  `LINK_ITEMS` - add entries to the link-time `functions` map for each monomorphic item in the crate;
2.  `LINK_INST`  - use a richer key-structure for the link-time `functions` map which uses keys that are pairs of a function type (`Ty`) _and_ an function instance kind (`InstanceKind`)
3.  `DEBUG` - serialize additional data in the JSON file and dump logs to stdout

### Usage For std-lib
To generate the stable MIR output for the entire standard libary, the most simple way is using the [cargo option build-std](https://doc.rust-lang.org/cargo/reference/unstable.html#build-std) on a new project while pointing the `RUSTC` environment variable to `run.sh`. For example:
```
cargo new example
cd example
RUSTC="/<PATH-TO>/smir_prety/run.sh" cargo build -Z build-std --target <TARGET>
```
If the `smir_pretty` is not built this will not initiate a build of `smir_pretty` and will crash instead. If the target is unknown it can be read using the `rustc_arch.sh` script in this repo. The generated files can be found in `example/target/<TARGET>/deps/` and will have extension `*.smir.json`.

WARNING: This option has been succeeding on cargo version `1.79.0-nightly` but has been failing on `1.82.0-nightly`

### Invocation Details

We use an uncommon build process where we link against a patched rustc installed in this repo.
However, since `cargo build` does not set `rpath` for dynamic linking, we must manually point the program loader/dynamic linker at the required runtime libraries.
Note that the `cargo run` command appears to prepard the rustlib directories automatically to the dynamic link search path.
If you wish to run the tool manually, you will need to tell the program loader/dynamic linker where to find the missing libraries by:

1.  setting `LD_LIBRARY_PATH`
2.  setting the `rpath` attribute on the binary ELF file
3.  manually invoking the loader (usually `/usr/lib/ld-linux.so.2`) with its specific options

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
