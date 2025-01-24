TESTDIR=$(CURDIR)/tests/integration/programs

.PHONY: integration-test
integration-test: TESTS     ?= $(shell find $(TESTDIR) -type f -name "*.rs")
integration-test: SMIR      ?= cargo run -- "-Zno-codegen"
# override this to tweak how expectations are formatted
integration-test: NORMALIZE ?= jq -S -e -f $(TESTDIR)/../normalise-filter.jq
# override this to re-make golden files
integration-test: DIFF      ?= | diff -
integration-test:
	errors=""; \
	report() { echo "$$1: $$2"; errors="$$errors\n$$1: $$2"; }; \
	for rust in ${TESTS}; do \
		target=$${rust%.rs}.smir.json; \
		dir=$$(dirname $${rust}); \
		echo "$$rust"; \
		${SMIR} --out-dir $${dir} $${rust} || report "$$rust" "Conversion failed"; \
		[ -f $${target} ] \
			&& ${NORMALIZE} $${target} ${DIFF} $${target}.expected \
			&& rm $${target} \
			|| report "$$rust" "Unexpected json output"; \
		done; \
	[ -z "$$errors" ] || (echo "===============\nFAILING TESTS:$$errors"; exit 1)


golden:
	make integration-test DIFF=">"
