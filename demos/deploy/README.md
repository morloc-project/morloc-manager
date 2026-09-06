# Deploying a morloc module

This directory is the target for the deployment work: the walkthrough below is
what should pass when it is done. It is a specification, not a report. Where a
step does not work yet, the last section says so.

The story it describes is the whole point of the deployment path. You start a
pliable container, you install and experiment and modify inside it, and when
you like what you have you freeze it into something you can hand to someone
else. The thing you hand over serves your functions over HTTP and MCP, runs
them from a command line, and can compile new programs out of them.

## The module

One exported function, in Python, with a type.

```morloc
--' Operations on DNA sequences
module dna (revcomp)

import root-py

source Py from "dna.py" ("revcomp")

--' Reverse complement a DNA sequence
revcomp ::
  --' A DNA sequence over the alphabet ACGT
  Str ->
  --' Its reverse complement
  Str
```

Everything downstream comes from that declaration. The command line, its help
text, the JSON API, the MCP tool description a model reads, and the checks at
every boundary are derived from the type and the docstrings, not written by
hand.

The file is `main.loc` by convention, and the program is named after its
`module` declaration, so this installs as `dna`. `package.yaml` lists `dna.py`
under `include`, without which the Python source does not travel with the
installed program.

You can see the whole surface without any of the deployment machinery:

```console
$ morloc make -o dna main.loc
$ ./dna revcomp ACGTA
"TACGT"
$ ./dna --help
Operations on DNA sequences

Reverse complement a DNA sequence

Usage: ./dna <nexus_options> @ <command_options>
...
Positional arguments:
  1:  A DNA sequence over the alphabet ACGT
      type: Str
      format: literal string

Return: Str
  Its reverse complement
```

A module with a single export collapses to a bare command line, so `./dna
ACGTA` works too. The network surface always names the command, so the API path
is `/call/dna/revcomp` either way.

## 1. Create the environment

```console
$ mim new --engine docker
```

Three things now exist. An image, `localhost/morloc-env:<name>`, holding a slim
base, pixi, the morloc compiler, the Rust runtime sources and an activation
wrapper. A directory, `~/.local/share/morloc/environments/<name>/`, holding the
environment's state, its runtime, and its pixi manifest. And two engine
volumes, one for the solved conda toolchain and one a package cache shared
across environments.

That is what "pliable" means here: an image plus three mounted pieces. Nothing
you build is baked into a layer, so you can keep changing it.

## 2. Work in it

```console
$ mim install main.loc
Installed: dna
Expose to serve:
  mim expose add dna --as mcp,api
```

`mim shell` and `mim run -- ...` reach the same environment for anything else.
The program is named after its module, not the file, which is why the install
tells you the name to use next.

## 3. Declare what is reachable

```console
$ mim expose add dna --as mcp,api
$ mim expose eval --allow dna
```

Installing a program does not expose it. This writes the declared set to
`expose.yaml`; nothing is served until you ask. `expose eval` turns on the
sandboxed eval capability, with an allow-list of the modules an expression may
import.

## 4. Serve it, still pliable

```console
$ mim start
$ mim status
$ mim eval 'import dna (revcomp); revcomp "ACGTA"'
$ mim logs -f
$ mim stop
```

This is the iteration loop. Reinstall, re-expose, restart. The environment is
still mounted, so a rebuild is a rebuild and not an image build.

## 5. Freeze

```console
$ mim freeze --tag dna-service:v1
```

This builds a self-contained image: the environment's own image, plus the
runtime copied in, plus the conda toolchain installed from the environment's
lock, plus the programs and the module sources behind them, with the exposed
set compiled into the default command.

## 6. Share it

The artifact is an image, so the ways to move it are the ordinary ones.

```console
$ docker push ghcr.io/you/dna-service:v1
$ docker save -o dna-service-v1.tar dna-service:v1   # a file, for an airgap
$ docker load -i dna-service-v1.tar                  # on the far side
```

## 7. Use it

```console
$ docker run -e MORLOC_MCP_TOKEN=$TOK -p 8080:8080 dna-service:v1

$ curl -H "Authorization: Bearer $TOK" -X POST \
       localhost:8080/call/dna/revcomp -d '["ACGTA"]'

$ docker run dna-service:v1 dna revcomp ACGTA
```

The same image is a server and a command line. MCP is at `/mcp`, the JSON API
at `/call/<module>/<command>`, discovery at `/discover`, and `/health` answers
without a token so an orchestrator can probe it.

The service binds every interface inside the container, because a container's
loopback belongs to the container and a published port never reaches
`127.0.0.1` in there. It therefore refuses to start without a bearer token.
Set `MORLOC_MCP_TOKEN`, or override the command with `--allow-no-auth` if you
mean to serve openly.

## Where the image lives

Freezing leaves nothing in your working directory. The artifact is a tag in the
engine's local image store: content-addressed layers and a manifest, under
`/var/lib/docker/` for docker, `~/.local/share/containers/storage/` for
rootless podman, and inside the VM's disk image on Docker Desktop. You do not
hand someone that directory. You reference the image by tag and move it with a
registry push, a `docker save` tarball, or a rebuild.

This is why the artifact is an image rather than an archive of state. An
archive of state is only rebuildable where the environment it came from already
exists, which is not portability. `docker save` produces a tarball that carries
every layer including the base, and `docker load` restores it anywhere.

## What this should exercise

- The environment is pliable: install, rebuild and re-expose without an image
  build.
- A frozen image runs with nothing mounted, on a machine that has never seen
  the environment.
- It serves the declared set and only that. A module you installed but did not
  expose is not reachable.
- It is a command line and a server from the same declarations.
- `/eval` compiles a new program out of the installed ones inside the image,
  which is why the compiler, the Rust sources and the conda toolchain stay in
  it rather than being trimmed for size.
- A missing piece is named. Freezing an environment that was never provisioned
  fails saying what it lacks, rather than producing an artifact with holes.

## Status

Steps 1 through 4 work today.

Step 5 does not yet build an image. `mim freeze` currently writes an archive
(`state.tar.gz` plus a manifest) that `mim unfreeze --from ... -t <tag>` turns
into an image, and that archive is only usable on a machine that already has
the environment image. Collapsing the two commands into one that builds the
image directly is the work this demo specifies. See
`plans/deployment/NOTE-01` in the workspace.

No container engine was available where this was written, so every `mim` and
`docker` transcript above is the intended invocation rather than a recorded
one. The `morloc make` and `./dna` output in "The module" is real.
