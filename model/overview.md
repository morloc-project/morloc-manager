# Overview

mim manages *environments*. An environment is one Morloc installation (a
compiler, its runtime, a solved set of language toolchains and packages) plus
the programs installed into it. mim creates environments, runs commands in
them, serves their programs, and freezes them into images.

mim and the compiler are coupled only across process boundaries. In every
contract the compiler (or its release) writes and mim reads, so mim carries
the compatibility burden:

| Contract | Writer | When mim reads it |
|---|---|---|
| release manifest | the release | provisioning a compiler (PROV) |
| `morloc lang-support` | the compiler | every solve (DEP) |
| `envspec.json` | `morloc make`, `morloc envspec` | every solve (DEP) |
| the build hook | `morloc make` invokes mim | during a build (DEP) |
| the build directory and manifest | `morloc make` | serving, freezing (SERVE, FRZ) |

The same mim binary is both the manager (the user's commands) and the
in-environment *agent* that the build hook invokes. One binary keeps the two
version-coherent by construction (PROV-8).
