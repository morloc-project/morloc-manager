# Serving (SERVE)

### SERVE-1 Only what a view names is served
Intent: proposed
Code: deviates F105

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
Intent: ruled 2026-11-20
Code: deviates F8
Tests: stop_clears_a_record_whose_container_is_gone, a_serve_leader_is_classified_by_start_time_and_boot, a_spawned_serve_leader_is_recognised_as_ours, stop_never_signals_the_group_of_a_gone_leader, stop_refuses_an_unverifiable_server_until_forgotten

`start` launches one server and records its handle; it refuses while that
server is alive unless told to replace it. A native handle holds the
leader's pid, its start time and the boot id. A leader that is gone, or
one recorded in another boot or now naming another process, is stale: its
record is removed and nothing is signalled. A leader whose identity cannot
be read is unknown: `stop` and `start` refuse and name the pid and group,
and `stop --forget` removes the record. A process group is never signalled
on its id alone. `stop` ends a live server and removes the record, and
says which of the two happened.

### SERVE-4 A server reachable from other machines requires a credential
Intent: ruled 2026-11-15
Code: deviates F106
Tests: a_secret_variable_never_appears_on_the_engine_command_line, a_served_containers_token_is_off_the_command_line, serve_auth_variables_are_refused_as_user_variables

A server binds loopback unless exposed. An exposed server requires an
acknowledgement that it speaks plaintext, and a token unless the operator
explicitly waives it. The token is chosen by the operator; mim never
generates it, writes it to disk, or puts it on a command line. NET-1 in the
compiler spec governs the server's side.

### SERVE-5 Eval on an exposed server requires the token
Intent: ruled 2026-11-15
Code: conforms 2026-11-15
Tests: a_native_serve_drops_the_callers_auth_variables

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
