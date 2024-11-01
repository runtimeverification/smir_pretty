TARGET ?= debug
STAGE  ?= 1
ifneq (0, $(shell test "${STAGE}" -gt 0 2>/dev/null; echo "$$?"))
$(error STAGE must be set to a number greater than 0)
endif
ifneq (${TARGET}, $(filter ${TARGET},debug release))
$(error TARGET must be set to one of debug/release)
endif
RUST_DIR=${CURDIR}/deps/rust
STAGE_FILE=${RUST_DIR}/stage
RUST_SRC=${RUST_DIR}/src
RUST_ARCH=$(shell "${PWD}"/rustc_arch.sh)
RUST_BUILD_DIR=${RUST_SRC}/build/${RUST_ARCH}
RUST_INSTALL_DIR=${RUST_BUILD_DIR}/stage${STAGE}
RUST_LIB_DIR=${RUST_INSTALL_DIR}/lib
RUST_DEP_DIR=${RUST_BUILD_DIR}/stage1-rustc/${RUST_ARCH}/release/deps
TARGET_DEP_DIR=${CURDIR}/target/${TARGET}/deps
TEMP_DIR=${RUST_DIR}/temp
RUST_REPO=https://github.com/runtimeverification/rust
RUST_BRANCH=master
TOOLCHAIN_NAME=smir_pretty
RELEASE_FLAG=
ifeq (${TARGET}, release)
RELEASE_FLAG=--release
endif

build_all: rust_build rust_set_toolchain build

setup: rust_clone

update: ${RUST_SRC}
	cd "${RUST_SRC}"; git fetch origin; git reset --hard origin/${RUST_BRANCH}

build:
	cargo build ${RELEASE_FLAG}

clean:
	cd "${RUST_SRC}" && ./x.py clean
	-rm -r "${TEMP_DIR}"
	-rm -r "${RUST_DIR}"/tests
	-rm -r ./target

distclean:
	cd "${RUST_SRC}" && git clean -dffx
	-rm -r "${TEMP_DIR}"
	-rm -r "${RUST_DIR}"/tests
	-rm -r ./target

# this clean removes old backup files which accumulate and lead to slow build times
prebuild_clean: ${RUST_SRC}
	-find -name '*.old' -delete
	-rm -r "${TEMP_DIR}"

# NOTE: a deeper clone depth is needed for the build process
rust_clone:
	git clone --depth 70 --single-branch --branch "${RUST_BRANCH}" "${RUST_REPO}" "${RUST_SRC}"


# rust_build for linking against custom rustc is involved
#
# 1. core rust compiler must be built via ./x.py build/install (we also build the test harness here)
# 2. rustc-dev component must be installed (./x.py build/install does _not_ handle, must be done manually)
# 3. HACK(only for ./x.py install) we copy required libraries to the libdir
# 4. finally, use rustup to create custom toolchain

rust_build: ${RUST_SRC} prebuild_clean
	cd "${RUST_SRC}"; ./x.py build src/tools/compiletest
	cd "${RUST_SRC}"; ./x.py build --stage ${STAGE} --set rust.debug-logging=true compiler/rustc library/std
	cd "${RUST_SRC}"; ./x.py dist --set rust.debug-logging=true rustc-dev
	mkdir -p "${TEMP_DIR}"
	cd "${RUST_SRC}"; tar xf ./build/dist/rustc-dev*tar.gz -C "${TEMP_DIR}"
	"${TEMP_DIR}"/rustc-dev*/install.sh --prefix="${RUST_INSTALL_DIR}" --sysconfdir="${RUST_INSTALL_DIR}" > "${RUST_DIR}"/rustc-dev-install.log 2>&1

rust_lib_copy:
	cd "${RUST_LIB_DIR}"; cp libLLVM* rustlib/*/lib/

rust_set_toolchain: ${RUST_LIB_DIR}
	rustup toolchain link "${TOOLCHAIN_NAME}" "${RUST_INSTALL_DIR}"
	rustup override set "${TOOLCHAIN_NAME}"
	echo ${STAGE} > ${STAGE_FILE}

generate_ui_tests:
	mkdir -p "${RUST_DIR}"/tests
	cd "${RUST_SRC}"; ./get_runpass.sh tests/ui > "${RUST_DIR}"/tests_ui_sources
	-cd "${RUST_SRC}"; ./ui_compiletest.sh "${RUST_SRC}" "${RUST_DIR}"/tests/ui/upstream "${RUST_DIR}"/tests_ui_sources --pass check --force-rerun 2>&1 > "${RUST_DIR}"/tests_ui_upstream.log
	-cd "${RUST_SRC}"; RUST_BIN="${PWD}"/run.sh ./ui_compiletest.sh "${RUST_SRC}" "${RUST_DIR}"/tests/ui/smir "${RUST_DIR}"/tests_ui_sources --pass check --force-rerun 2>&1 > "${RUST_DIR}"/tests_ui_smir.log
