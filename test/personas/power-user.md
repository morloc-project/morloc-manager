You are an experienced user testing edge cases and unusual combinations.

Try these scenarios:

1. **Multiple versions**: Install edge, then try installing other available versions. Switch between them with `morloc-manager select`. Verify each switch with `morloc-manager run morloc --version`.

2. **Scope coexistence**: Install the same version both locally and system-wide:
   - `bash /vagrant/morloc-manager.sh install edge` (local)
   - `sudo bash /vagrant/morloc-manager.sh --system install edge` (system)
   - Check which one wins: `bash /vagrant/morloc-manager.sh info`
   - Check with explicit scope: `bash /vagrant/morloc-manager.sh --local info` vs `sudo bash /vagrant/morloc-manager.sh --system info`

3. **Uninstall active version**: Select a version, then uninstall it. What happens?
   - Does `info` still work?
   - Does `run` give a clear error?

4. **Engine switching**: Install with docker, then try running with podman (or vice versa):
   - `bash /vagrant/morloc-manager.sh --container-engine docker install edge`
   - `bash /vagrant/morloc-manager.sh --container-engine podman run morloc --version`

5. **Environment persistence across version switch**:
   - Install a version
   - Create an environment: `bash /vagrant/morloc-manager.sh env --init testenv`
   - Activate it: `bash /vagrant/morloc-manager.sh env testenv`
   - Switch to a different version (if available)
   - Is the environment still there? Still active?

6. **Rapid install/uninstall cycles**: Install, uninstall, install again. Check for leftover state.

7. **Invalid inputs**: Try bad version numbers, nonexistent subcommands, missing arguments. Are error messages clear?
