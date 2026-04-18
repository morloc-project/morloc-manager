You are a developer who wants to use morloc for a project.

Try to:
- Create an environment: `/vagrant/morloc-manager new dev --version 0.76.0`
- Run a basic command: `/vagrant/morloc-manager run -- morloc --version`
- Chain commands in one container session:
  `/vagrant/morloc-manager run -- bash -c "morloc --version && which morloc"`
- Install modules: `/vagrant/morloc-manager run -- morloc install root-py`
- Create another environment with a stub Dockerfile:
  `/vagrant/morloc-manager new ml --version 0.76.0 --dockerfile-stub`
- Edit the Dockerfile to add a dependency (e.g., uncomment the jq line)
- Rebuild: `/vagrant/morloc-manager update ml`
- Switch between environments: `/vagrant/morloc-manager select dev` and
  `/vagrant/morloc-manager select ml`
- List environments: `/vagrant/morloc-manager ls`
- Show detailed info: `/vagrant/morloc-manager info ml`
- Remove an environment: `/vagrant/morloc-manager rm ml`
- And more, explore and be creative, try a few things that are not in this list

Test the deployment pipeline (Steps 5-7 of the tutorial):
- Install a program: `/vagrant/morloc-manager run -- morloc make --install dnd.loc`
- Start serving: `/vagrant/morloc-manager start`
- Test endpoints:
  `curl -s localhost:8080/health`
  `curl -s localhost:8080/programs`
  `curl -s -X POST localhost:8080/call/dnd/rollAdv`
- Stop: `/vagrant/morloc-manager stop`
- Freeze: `/vagrant/morloc-manager freeze -o ./dnd-freeze`
- Unfreeze: `/vagrant/morloc-manager unfreeze --from ./dnd-freeze/state.tar.gz --tag dnd-serve:v1`
- Run frozen image and test endpoints again

Test with both docker and podman by creating environments with different engines:
- `/vagrant/morloc-manager new docker-env --version 0.76.0 --engine docker`
- `/vagrant/morloc-manager new podman-env --version 0.76.0 --engine podman`
