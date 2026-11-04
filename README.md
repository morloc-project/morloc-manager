# morloc-manager

`mim` creates and manages Morloc environments, runs programs in them, and serves
them over a JSON API and MCP.

## Installation

```sh
curl -fsSL https://morloc.io/install.sh | sh
```

## Usage

Create an environment with the latest Morloc release in the local environment
(works on Linux and MacOS)

```sh
mim new
```

An alternative to a local environment is a containerized one.

```sh
mim new --engine docker    -- podman is also accepted
```

This is more reproducible and the morloc containerized environments can be
frozen and shared.

```
mim freeze --slim --save myenv.tar
```

These environments are similar to conda environments, except they run in a
subshell, so you leave with the `exit` command:

```sh
mim shell
```


## Executing Morloc code

Build and run a program without entering the environment:

```sh
cat > hello.loc <<'EOF'
module hw (hello)

--' A Morlock's hello world
hello = "Hello up there"
EOF
mim run -- morloc make -o hello hello.loc
mim run -- ./hello hello
```

Install the module and serve it over the JSON API and MCP:

```sh
mim install hello.loc
mim view add hw --as mcp,api
mim start -p 8080:8080
curl -s http://localhost:8080/call/hw/hello -d '[]'
claude mcp add --transport http hw http://localhost:8080/mcp
mim stop
```

Remove every environment and the container images they were built from:

```sh
mim nuke --images
```
