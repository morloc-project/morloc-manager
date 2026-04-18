.PHONY: rust-build build-images build-all up up-fast down sync push quick-test \
       export-image load-image push-image explore explore-vm explore-fast clean help

BUILD_SCRIPT := ../../compiler/morloc/scripts/build-rust.sh
MORLOC_VERSION ?= edge
IMAGE_TAR := /tmp/morloc-full-$(MORLOC_VERSION).tar
IMAGE_REF := ghcr.io/morloc-project/morloc/morloc-full:$(MORLOC_VERSION)

## ---- Build (delegates to compiler repo) ----

## Build static Rust binaries -> compiler/morloc/out/
rust-build:
	MORLOC_VERSION=$(MORLOC_VERSION) $(BUILD_SCRIPT) rust

## Build morloc-tiny + morloc-full containers locally
build-images:
	MORLOC_VERSION=$(MORLOC_VERSION) $(BUILD_SCRIPT) tiny
	MORLOC_VERSION=$(MORLOC_VERSION) $(BUILD_SCRIPT) full

## Build everything (binaries + containers)
build-all:
	MORLOC_VERSION=$(MORLOC_VERSION) $(BUILD_SCRIPT) all

## ---- VM management ----

## Start VM(s) with provisioning (e.g., make up VM=fedora)
up:
	vagrant up $(VM)

## Start VM(s) without re-provisioning
up-fast:
	vagrant up $(VM) --no-provision

## Destroy VM(s)
down:
	vagrant destroy -f $(VM)

## ---- Fast dev cycle ----

## Sync updated binary + out/ files into running VM
sync:
	vagrant rsync $(VM)

## Build Rust binaries + sync into VM
push: rust-build sync

## Quick smoke test after push
quick-test: push
	vagrant ssh $(VM) -c '/vagrant/morloc-manager --version'

## ---- Container image loading ----

## Export image to tarball
export-image:
	MORLOC_VERSION=$(MORLOC_VERSION) $(BUILD_SCRIPT) export

## Load image tarball into VM's Docker + Podman (e.g., make load-image VM=fedora)
load-image:
	cat $(IMAGE_TAR) | vagrant ssh $(VM) -c 'sudo docker load'
	cat $(IMAGE_TAR) | vagrant ssh $(VM) -c 'podman load'

## Build images locally + export + load into VM
push-image: build-images export-image load-image

## ---- Agent exploration ----

## Full overnight exploration (original behavior)
explore:
	bash test/run-exploration.sh

## Exploration on a single VM (e.g., make explore-vm VM=fedora)
explore-vm:
	bash test/run-exploration.sh --vms $(VM)

## Exploration on already-running VMs (no create/destroy)
explore-fast:
	vagrant rsync $(VM)
	bash test/run-exploration.sh --persistent --vms $(VM)

## ---- Cleanup ----

## Remove exploration findings
clean:
	rm -rf findings/*/

## ---- Help ----

help:
	@echo "Build targets (delegates to compiler repo):"
	@echo "  rust-build     Build static Rust binaries"
	@echo "  build-images   Build morloc-tiny + morloc-full containers"
	@echo "  build-all      Build everything"
	@echo ""
	@echo "VM management:"
	@echo "  up             Start VMs (with provisioning)"
	@echo "  up-fast        Start VMs (skip provisioning)"
	@echo "  down           Destroy VMs"
	@echo ""
	@echo "Fast dev cycle:"
	@echo "  sync           Rsync files into running VM"
	@echo "  push           Build binaries + sync"
	@echo "  quick-test     Build + sync + smoke test"
	@echo ""
	@echo "Container images:"
	@echo "  export-image   Save image to tarball"
	@echo "  load-image     Load tarball into VM"
	@echo "  push-image     Build + export + load (full pipeline)"
	@echo ""
	@echo "Exploration:"
	@echo "  explore        Full overnight run"
	@echo "  explore-vm     Single VM (VM=fedora)"
	@echo "  explore-fast   On already-running VMs"
	@echo ""
	@echo "Variables: VM=fedora MORLOC_VERSION=edge"
