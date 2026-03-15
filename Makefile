BATS := test/lib/bats/bin/bats
SHELLCHECK := shellcheck

.PHONY: test unit integration e2e lint check clean help \
        vm-up vm-test vm-test-quick vm-destroy \
        distro-build distro-test

## Default: run the fast, offline tests
test: unit integration

## Run unit tests (no container engine needed)
unit:
	$(BATS) test/unit/

## Run integration tests (no container engine needed, uses mock engine)
integration:
	$(BATS) test/integration/

## Run end-to-end tests (requires Docker or Podman)
e2e:
	$(BATS) test/e2e/

## Run all test tiers
all: unit integration e2e

## Run ShellCheck
lint:
	$(SHELLCHECK) morloc-manager.sh

## Lint + fast tests
check: lint test

## ---- VM tests (Tier 2, requires Vagrant + libvirt) ----

## Start all Vagrant VMs
vm-up:
	vagrant up --parallel

## Run full test suite inside all Vagrant VMs (unit + integration + vm + e2e)
vm-test:
	bash test/run-vm-tests.sh

## Quick VM tests: unit + integration only inside VMs (no e2e, faster)
vm-test-quick:
	@for vm in fedora ubuntu debian; do \
		echo "=== $$vm: unit + integration ==="; \
		vagrant ssh $$vm -c "cd /vagrant && bats test/unit/ test/integration/" || true; \
	done

## Destroy all Vagrant VMs
vm-destroy:
	vagrant destroy -f

## ---- Distro matrix (requires Docker) ----

DISTROS := ubuntu-22.04 ubuntu-24.04 fedora-39 debian-12 alpine-3.19

## Build all distro test containers
distro-build:
	@for d in $(DISTROS); do \
		echo "=== Building mm-test-$$d ==="; \
		docker build -t mm-test-$$d -f test/distro/Dockerfile.$$d . ; \
	done

## Run e2e tests inside each distro container
distro-test: distro-build
	@for d in $(DISTROS); do \
		echo "=== Testing on $$d ==="; \
		docker run --rm \
			-v /var/run/docker.sock:/var/run/docker.sock \
			-v $(CURDIR):/workspace \
			mm-test-$$d \
			bats /workspace/test/e2e/ ; \
	done

## ---- Submodule setup ----

## Initialize BATS submodules (run once after clone)
setup:
	git submodule update --init --recursive

## ---- Cleanup ----

## Remove generated test artifacts
clean:
	rm -rf /tmp/morloc-test-home.*
	rm -rf /tmp/mock-engine.*

## ---- Help ----

## Show available targets
help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Fast (offline, no container engine):"
	@echo "  test          Run unit + integration tests (default)"
	@echo "  unit          Run unit tests only"
	@echo "  integration   Run integration tests only"
	@echo "  lint          Run ShellCheck"
	@echo "  check         Run lint + unit + integration"
	@echo ""
	@echo "Requires container engine:"
	@echo "  e2e           Run end-to-end tests"
	@echo "  all           Run unit + integration + e2e"
	@echo "  distro-build  Build distro test containers"
	@echo "  distro-test   Run e2e inside each distro container"
	@echo ""
	@echo "Requires Vagrant + libvirt:"
	@echo "  vm-up         Start Vagrant VMs"
	@echo "  vm-test       Run full test suite across all VMs"
	@echo "  vm-test-quick Run unit + integration only inside VMs"
	@echo "  vm-destroy    Destroy Vagrant VMs"
	@echo ""
	@echo "Other:"
	@echo "  setup         Initialize BATS git submodules"
	@echo "  clean         Remove temp test artifacts"
	@echo "  help          Show this help"
