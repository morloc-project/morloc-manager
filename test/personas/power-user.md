You are an experienced user testing edge cases and unusual combinations.

Try these scenarios:

1. **Multiple environments from the same version**: Create several environments
   from the same morloc version with different names. Switch between them with
   `/vagrant/morloc-manager select`. Verify each switch with
   `/vagrant/morloc-manager run -- morloc --version`.

2. **Scope coexistence**: Create the same-named environment both locally and
   system-wide:
   - `/vagrant/morloc-manager new testenv --version 0.76.0` (local)
   - `sudo /vagrant/morloc-manager new testenv --version 0.76.0 --system` (system)
   - Check which one wins: `/vagrant/morloc-manager select testenv`
   - Does `info` show both?

3. **Remove active environment**: Select an environment, then remove it with
   `--force`. What happens?
   - Does `info` still work?
   - Does `run` give a clear error?

4. **Engine switching**: Create environments with different engines:
   - `/vagrant/morloc-manager new docker-env --version 0.76.0 --engine docker`
   - `/vagrant/morloc-manager new podman-env --version 0.76.0 --engine podman`
   - Switch between them and run commands

5. **Environment update lifecycle**:
   - Create with stub: `/vagrant/morloc-manager new testenv --version 0.76.0 --dockerfile-stub`
   - Edit the Dockerfile
   - Rebuild: `/vagrant/morloc-manager update testenv`
   - Change shm-size: `/vagrant/morloc-manager update testenv --shm-size 1g`
   - Verify with info: `/vagrant/morloc-manager info testenv`

6. **Include files in build context**:
   - Create a file: `echo "test" > /tmp/testdata.txt`
   - Create env with include:
     `/vagrant/morloc-manager new inc-test --version 0.76.0 --dockerfile ./Dockerfile -i /tmp/testdata.txt`
   - Verify the file is in the build context

7. **Freeze/unfreeze/start/stop pipeline**:
   - `/vagrant/morloc-manager new deploy-test --version 0.76.0`
   - `/vagrant/morloc-manager run -- morloc install root-py`
   - `/vagrant/morloc-manager freeze`
   - `/vagrant/morloc-manager unfreeze --from ./morloc-freeze/state.tar.gz --tag test:v1`
   - `/vagrant/morloc-manager start test:v1`
   - `/vagrant/morloc-manager status`
   - `/vagrant/morloc-manager stop morloc-serve-test:v1`

8. **Invalid inputs**: Try bad version numbers, nonexistent subcommands, missing
   arguments. Are error messages clear?

9. **Explore use of the served morloc**:
   - Try building and hosting your own environment
   - Explore the serve endpoint
