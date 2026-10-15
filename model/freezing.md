# Freezing (FRZ)

### FRZ-1 A frozen image is the environment with nothing mounted
Intent: proposed
Code: unaudited

`freeze` builds an image holding the environment's image, its toolchain
installed from its lock, its runtime, every installed program with its
source directory, and its views. The image runs every program and serves
the views without access to the host that built it.

### FRZ-2 Only a Docker or Podman release environment can be frozen
Intent: proposed
Code: unaudited

Native, Apptainer, development and local-runtime environments are refused.

### FRZ-3 The toolchain in the image is the locked one
Intent: proposed
Code: unaudited

Packages are installed from the environment's lock without re-solving; a
lock that does not install fails the freeze.

### FRZ-4 Freezing does not change the environment
Intent: proposed
Code: unaudited

`freeze` reads the environment and writes only the image (and an archive if
asked).

### FRZ-5 Freezing refuses what should not ship
Intent: proposed
Code: unaudited

Tool state in a program directory (version control, caches, virtual
environments, build output) is refused. Large programs, large files and an
environment with no programs are confirmed by the user; without a terminal,
confirmation is refusal. A force flag answers all of them.

### FRZ-6 The image decides nothing about who can reach it
Intent: proposed
Code: unaudited

Inside the image the server listens on every interface on one fixed port;
the publisher of the port decides reachability. A token, if set at run
time, is required as in SERVE-4. Eval stays locked without a token unless
waived at run time (SERVE-5).

### FRZ-7 The image describes itself
Intent: proposed
Code: unaudited

Labels on the image record the Morloc version, the source environment, the
programs, each adapter's modules, and the suggested shared-memory size.

### FRZ-8 A slim image runs every program it ships
Intent: proposed
Code: unaudited

A slim image drops compilers, the solver, the Morloc compiler and build-only
packages, except a tool a program declares and everything that tool depends
on, which stay whatever their names. It is refused when eval is enabled, and after it is built every
program is started and every runtime library's linkage is checked inside it.
