# morloc-manager testing infrastructure
#
# Usage:
#   make up VM=fedora          Start a VM
#   make explore VM=fedora PROMPT=test/prompts/full-exploration.md
#   make push VM=fedora        Rebuild binary + rsync into running VM
#   make quick-test VM=fedora  Push + smoke test
#   make down VM=fedora        Destroy a VM

MORLOC_VERSION ?= edge
VM ?=
PROMPT ?=

# Require VM= for targets that need it
check-vm:
	@if [ -z "$(VM)" ]; then \
		echo "ERROR: VM= is required (e.g., make $@ VM=fedora)" >&2; \
		exit 1; \
	fi

# Require PROMPT= for targets that need it
check-prompt:
	@if [ -z "$(PROMPT)" ]; then \
		echo "ERROR: PROMPT= is required (e.g., make $@ PROMPT=test/prompts/full-exploration.md)" >&2; \
		exit 1; \
	fi

## VM management

up: check-vm ## Start a VM
	vagrant up $(VM)

down: check-vm ## Destroy a VM
	vagrant destroy -f $(VM)

ssh: check-vm ## SSH into a VM
	vagrant ssh $(VM)

sync: check-vm ## Rsync files into a running VM
	vagrant rsync $(VM)

## Build pipeline (delegated to compiler repo)

rust-build: ## Build static Rust binaries via the compiler repo
	./scripts/build-rust.sh

build-images: ## Build morloc-tiny + morloc-full containers locally
	./scripts/build-images.sh

build-all: rust-build build-images ## Build everything (binaries + containers)

## Binary distribution

push: check-vm rust-build sync ## Rebuild binary + rsync into running VM

push-image: check-vm build-images export-image load-image ## Build + export + load images into a VM

export-image: ## Export container images to tarballs
	./scripts/export-image.sh

load-image: check-vm ## Load exported images into a VM
	./scripts/load-image.sh $(VM)

## Testing

EXPLORE_TUNING_ARGS = \
    $(if $(OUTPUT),--output $(OUTPUT)) \
    $(if $(MODEL),--model $(MODEL)) \
    $(if $(EXPLORER_MODEL),--explorer-model $(EXPLORER_MODEL)) \
    $(if $(ANALYST_MODEL),--analyst-model $(ANALYST_MODEL)) \
    $(if $(MAX_TURNS),--max-turns $(MAX_TURNS)) \
    $(if $(EXPLORER_MAX_TURNS),--explorer-max-turns $(EXPLORER_MAX_TURNS)) \
    $(if $(ANALYST_MAX_TURNS),--analyst-max-turns $(ANALYST_MAX_TURNS))

explore: check-vm check-prompt ## Run all personas on a VM (vars: OUTPUT, MODEL, *_MODEL, MAX_TURNS, *_MAX_TURNS)
	./test/run-exploration.sh --vm $(VM) $(EXPLORE_TUNING_ARGS) $(PROMPT)

explore-sync: check-vm check-prompt ## Sync + run all personas on a VM
	./test/run-exploration.sh --vm $(VM) --sync $(EXPLORE_TUNING_ARGS) $(PROMPT)

quick-test: check-vm push ## Push binary + smoke test
	vagrant ssh $(VM) -c '/vagrant/morloc-manager --help'

## Cleanup

clean: ## Remove exploration findings
	rm -rf findings/*/

pristine: clean ## Remove all findings including the shared log and final report
	rm -f findings/log.md findings/report.md findings/HALT findings/analyst-session.log

## Help

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*## "}; {printf "  %-16s %s\n", $$1, $$2}'

.PHONY: check-vm check-prompt up down ssh sync rust-build build-images build-all push push-image export-image load-image explore explore-sync quick-test clean pristine help
.DEFAULT_GOAL := help
