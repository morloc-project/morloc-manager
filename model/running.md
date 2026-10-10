# Running (RUN)

### RUN-1 A command runs with the environment's installation and the user's directory
Intent: proposed
Code: unaudited

`run` and `shell` execute with the environment's compiler, runtime,
toolchain and installed programs first on `PATH`, `MORLOC_HOME` set to its
runtime, and the user's current directory as the working directory (mounted
at `/work` in a container). Files written there belong to the invoking user.

### RUN-2 A managed command can build
Intent: proposed
Code: deviates F14

Every command mim launches in an environment exports `MORLOC_ENV` (the
environment's name), `MORLOC_BUILD_HOOK` (the agent, PROV-8) and
`MORLOC_BIN` (the compiler), together, so `morloc make` inside it keeps the
toolchain in step (DEP-1).

### RUN-3 The exit status is the command's
Intent: proposed
Code: deviates F15
Tests: a_command_status_is_never_taken_for_an_engine_error

mim exits with the status of the command it ran. A failure of mim or the
engine before the command starts is reported as such and never confused
with a status the command chose.

### RUN-4 Foreign activations never leak into an environment
Intent: proposed
Code: unaudited

Before a solve, a build or a native command, mim removes the variables of
any conda, mamba or pixi environment active on the host, so two package
worlds never mix compiler flags or library paths.

### RUN-5 Only declared variables cross into a container
Intent: proposed
Code: unaudited

A container sees the host's environment variables only as named with
`--env-var` or `--env-file`. Extra engine flags come from the environment's
flag file, by phase and engine, then the command line.
