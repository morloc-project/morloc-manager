# Dependencies (DEP)

### DEP-1 The build hook is mim's only entry point during a build
Intent: proposed
Code: unaudited

Inside an environment, `morloc make` runs
`$MORLOC_BUILD_HOOK sync --name <key> --spec envspec.json --root <dir>
[--installed]` before compiling pools. Exit 0 means the toolchain now
satisfies the spec; any other status fails the build. The hook writes only
to standard error.

### DEP-2 A spec of another version is refused with the direction of the fix
Intent: proposed
Code: unaudited

The envspec and lang-support documents carry versions. mim accepts exactly
the envspec version it reads and any lang-support schema of the same major
version, and refuses others with "rebuild the program" or "upgrade mim".

### DEP-3 One solved world per environment
Intent: proposed
Code: unaudited

An environment has one set of packages: the solve of the union of every
installed program's spec, every scratch build's spec, the compiler's own
requirements, the user's extra packages and the interpreter pins (DEP-7).
One package has one version. Unsatisfiable requirements fail the command.

### DEP-4 Solves of one environment are serialized
Intent: proposed
Code: deviates F3

Recording a spec, gathering the world, solving it and recording the result
happen under one exclusive lock per environment, shared by the host and the
container. Every path that writes the solver's manifest or lock takes it.

### DEP-5 A failed solve leaves the recorded requirements as they were
Intent: proposed
Code: deviates F1, F2

If a sync or clean fails, every spec it added, replaced or removed is
restored. The solver's own files may be left changed; the next successful
solve reconciles them.

### DEP-6 A solve with unchanged requirements does nothing
Intent: proposed
Code: unaudited

The world's requirements reduce to a canonical digest. When it equals the
digest of the last successful solve and the solved prefix exists, the solve
is skipped. The digest is written last, so an interrupted solve is redone.

### DEP-7 Versions move only when the requirements force them
Intent: proposed
Code: deviates F18

A solve first tries the existing lock unchanged and re-solves only if the
lock cannot satisfy the new requirements. Once an interpreter is
provisioned, its minor version is pinned so that a later solve cannot break
the language bindings built against it; a binding that no longer matches is
rebuilt, or refused in an immutable environment.

### DEP-8 Every declared dependency is provided, delegated, or refused
Intent: proposed
Code: unaudited

conda and PyPI packages are solved into the world. Crates and Julia
packages are left to the language's own build tool. Sources mim does not
support are refused, never dropped. Two conflicting declarations of one
package (two channels, two local paths, a path and a version) are refused.

### DEP-9 An installed program does not depend on its project directory
Intent: proposed
Code: unaudited

A local Python package of an installed program is copied into the
environment under the hash of its contents and the requirement points at
the copy. Snapshots no installed program references are reclaimed.

### DEP-10 An immutable environment refuses any change to its world
Intent: proposed
Code: unaudited

In an environment marked immutable (a running server, a frozen image), a
sync that would change the world or rebuild a binding fails before writing
anything.
