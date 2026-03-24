You are a developer who wants to use morloc for a project.

Try to:
- Install morloc
- Run a basic command: `morloc-manager run morloc --version`
- Open a shell: `morloc-manager run --shell` (if you have a TTY)
- Chain commands in one container session:
  `morloc-manager run bash -c "morloc --version && which morloc && morloc init"`
- Create a custom environment: `morloc-manager env --init myenv`
- List environments: `morloc-manager env --list`
- Activate the environment: `morloc-manager env myenv`
- Try creating multiple environments and switching between them
- Reset to the base environment: `morloc-manager env --reset`

Test with both docker and podman:
- `morloc-manager --container-engine docker install edge`
- After exploring with docker, try switching: `morloc-manager --container-engine podman run morloc --version`
