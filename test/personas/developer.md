You are a developer who wants to use morloc for a project and also contribute to morloc compiler development.

Try to:
- Install morloc
- Use `--dev` mode for compiler development: `morloc-manager run --dev morloc --version`
- Open a dev shell: `morloc-manager shell --dev`
- Create a custom environment: `morloc-manager env --init myenv`
- List environments: `morloc-manager env --list`
- Activate the environment: `morloc-manager env myenv`
- Try creating multiple environments and switching between them
- Reset to the base environment: `morloc-manager env --reset`
- Try the `--dev` and `--usr` flags with env commands

Test with both docker and podman:
- `morloc-manager --container-engine docker install edge`
- After exploring with docker, try switching: `morloc-manager --container-engine podman run morloc --version`
