# Serving (SERVE)

### SERVE-1 Only what a view names is served
Intent: proposed
Code: unaudited

An environment has a view per adapter (MCP, the JSON API) listing installed
modules, and an optional eval allow-list. Installing a module never serves
it. Views are stored with the environment's configuration and read when a
server starts.

### SERVE-2 A view names only installed modules
Intent: proposed
Code: deviates F17
Tests: remove_clears_the_eval_allow_list

Adding a module to a view or to the eval allow-list requires that it be
installed, and removing a module from an environment's views removes it from
the allow-list too.

### SERVE-3 One server per environment
Intent: proposed
Code: deviates F8, F62
Tests: stop_clears_a_record_whose_container_is_gone, stop_never_signals_a_process_that_reused_the_server_pid, a_serve_leader_is_judged_by_its_start_time

`start` launches one server and records its handle; it refuses while that
server is alive unless told to replace it. A handle is judged alive only if
it still names the process or container mim started. `stop` ends it and
removes the record, and a record whose server is gone is removed too.

### SERVE-4 A server reachable from other machines requires a credential
Intent: proposed
Code: unaudited

A server binds loopback unless exposed. An exposed server requires an
acknowledgement that it speaks plaintext, and a token unless the operator
explicitly waives it. The token is chosen by the operator; mim never
generates it, writes it to disk, or puts it on a command line. NET-1 in the
compiler spec governs the server's side.

### SERVE-5 Eval on an exposed server requires the token
Intent: proposed
Code: unaudited

On a loopback server eval needs no token. On an exposed one it runs only for
callers presenting the token, unless the operator waives that separately
from the server-wide waiver.

### SERVE-6 A served environment cannot change its installation
Intent: proposed
Code: deviates F8

A container server runs with a read-only root filesystem and marks the
environment immutable (DEP-10).

### SERVE-7 Every backend that can serve can be listed, logged and stopped
Intent: proposed
Code: deviates F8

`status`, `logs`, `stop` and `eval` act on a server of every backend that
`start` supports.
