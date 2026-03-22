SHELLCHECK := shellcheck

.PHONY: lint explore vm-up vm-destroy clean help

## Run ShellCheck
lint:
	$(SHELLCHECK) -s sh morloc-manager.sh

## ---- Agent-based exploratory testing (requires Vagrant + libvirt + Claude Code) ----

## Run full overnight exploration across all VMs (sequential, one at a time)
explore:
	bash test/run-exploration.sh

## Run exploration on a single VM (e.g., make explore-vm VM=fedora)
explore-vm:
	bash test/run-exploration.sh $(VM)

## Start Vagrant VMs
vm-up:
	vagrant up --parallel $(VM)

## Destroy Vagrant VMs
vm-destroy:
	vagrant destroy -f $(VM)

## ---- Cleanup ----

## Remove exploration findings
clean:
	rm -rf findings/*/

## ---- Help ----

## Show available targets
help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "Lint:"
	@echo "  lint          Run ShellCheck on morloc-manager.sh"
	@echo ""
	@echo "Exploratory testing (requires Vagrant + Claude Code):"
	@echo "  explore       Run all personas on all VMs (overnight, sequential)"
	@echo "  explore-vm    Run all personas on one VM (e.g., VM=fedora)"
	@echo ""
	@echo "VM management:"
	@echo "  vm-up         Start Vagrant VMs"
	@echo "  vm-destroy    Destroy Vagrant VMs"
	@echo ""
	@echo "Other:"
	@echo "  clean         Remove exploration findings"
	@echo "  help          Show this help"
