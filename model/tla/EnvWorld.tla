------------------------------- MODULE EnvWorld -------------------------------
(* One environment's declared world and its solved prefix (DEP-3, DEP-4,     *)
(* DEP-6, DEP-11, DEP-12). The environment's store S holds the specs; the    *)
(* pixi project holds the manifest, the prefix and the solve digest. Agent s *)
(* builds program a under the environment's own state; agent p builds b      *)
(* under a private MORLOC_STATE P; the manager re-solves the store's world;  *)
(* the legacy agent is an older mim that keys its store and lock on P.       *)
(* Ids: 0 is absent. Only the manager writes them.                          *)
EXTENDS Naturals, FiniteSets

CONSTANT Variant, Legacy, WrongMarker, SRuns
\* "design": the design.
\* "state_store": the hook keys its store and lock on MORLOC_STATE, no ids.
\* "no_manager_lock": the manager solves without the environment's lock.
\* "digest_only": the fast path trusts the digest without the manifest.
\* "no_id_check": the hook does not compare the store's and pixi ids.
\* Legacy: an older agent also runs. WrongMarker: p's MORLOC_ENV_STORE
\* names P.

Actors == {"s", "p", "m", "l"}
Stores == {"S", "P"}
Progs == {"a", "b", "c"}
ProgOf == [s |-> "a", p |-> "b", l |-> "c", m |-> "none"]

StoreOf(x) ==
    CASE x = "s" -> "S"
      [] x = "l" -> "P"
      [] x = "m" -> "S"
      [] x = "p" -> IF Variant = "state_store" \/ WrongMarker THEN "P" ELSE "S"

IdCheck(x) == x # "l" /\ Variant \notin {"state_store", "no_id_check"}
ManifestCheck(x) == x # "l" /\ Variant # "digest_only"
Locks(x) == x # "m" \/ Variant # "no_manager_lock"

VARIABLES
    spec,      \* store -> programs recorded in it
    manifest,  \* programs in the pixi manifest on disk
    prefix,    \* programs whose packages the prefix holds
    dset,      \* a solve digest is recorded
    dworld,    \* the world the digest names
    dman,      \* the manifest the digest names; "old" from the legacy agent
    lock,      \* store -> holder of the lock file under it, or "free"
    storeId,   \* store -> id under its requirements
    pixiId,    \* id in the pixi project
    pc,
    runs,      \* actor -> completed runs
    w,         \* actor -> world it gathered
    built,     \* programs whose sync succeeded
    stale,     \* a sync fast-pathed while its program was missing from the prefix
    foreign    \* the shared pixi project was solved from a store other than S

vars == <<spec, manifest, prefix, dset, dworld, dman, lock, storeId, pixiId,
          pc, runs, w, built, stale, foreign>>

MaxRuns(x) == CASE x = "s" -> SRuns [] x = "l" -> (IF Legacy THEN 1 ELSE 0)
                [] OTHER -> 1

Init ==
    /\ spec = [st \in Stores |-> {}]
    /\ manifest = {}
    /\ prefix = {}
    /\ dset = FALSE
    /\ dworld = {}
    /\ dman = {}
    /\ lock = [st \in Stores |-> "free"]
    /\ storeId = [st \in Stores |-> 0]
    /\ pixiId = 0
    /\ pc = [x \in Actors |-> IF MaxRuns(x) = 0 THEN "done" ELSE "idle"]
    /\ runs = [x \in Actors |-> 0]
    /\ w = [x \in Actors |-> {}]
    /\ built = {}
    /\ stale = FALSE
    /\ foreign = FALSE

Keep == <<spec, manifest, prefix, dset, dworld, dman, lock, storeId, pixiId,
          runs, w, built, stale, foreign>>

Goto(x, l) == pc' = [pc EXCEPT ![x] = l]

Acquire(x) ==
    /\ pc[x] = "idle"
    /\ IF Locks(x)
       THEN /\ lock[StoreOf(x)] = "free"
            /\ lock' = [lock EXCEPT ![StoreOf(x)] = x]
       ELSE UNCHANGED lock
    /\ Goto(x, IF x = "m" THEN "ids" ELSE "check")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, storeId, pixiId,
                   runs, w, built, stale, foreign>>

\* The manager's data dir holds both the store and the pixi project, so it
\* knows they belong together: it creates the store's id if absent and
\* copies it into the pixi project.
ManagerIds ==
    /\ pc["m"] = "ids"
    /\ storeId' = [storeId EXCEPT !["S"] = IF @ = 0 THEN 1 ELSE @]
    /\ pixiId' = storeId'["S"]
    /\ w' = [w EXCEPT !["m"] = spec["S"]]
    /\ Goto("m", "write")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, lock, runs,
                   built, stale, foreign>>

\* An agent compares ids, records its spec and gathers its store's world.
Check(x) ==
    /\ x # "m"
    /\ pc[x] = "check"
    /\ IF IdCheck(x) /\ (storeId[StoreOf(x)] = 0 \/ storeId[StoreOf(x)] # pixiId)
       THEN /\ Goto(x, "refused")
            /\ UNCHANGED <<spec, w>>
       ELSE /\ spec' = [spec EXCEPT ![StoreOf(x)] = @ \cup {ProgOf[x]}]
            /\ w' = [w EXCEPT ![x] = spec'[StoreOf(x)]]
            /\ Goto(x, "fast")
    /\ UNCHANGED <<manifest, prefix, dset, dworld, dman, lock, storeId, pixiId,
                   runs, built, stale, foreign>>

Fast(x) ==
    /\ pc[x] = "fast"
    /\ IF dset /\ dworld = w[x] /\ (ManifestCheck(x) => dman = manifest)
       THEN /\ stale' = (stale \/ ProgOf[x] \notin prefix)
            /\ built' = built \cup {ProgOf[x]}
            /\ Goto(x, "release")
            /\ UNCHANGED dset
       ELSE /\ dset' = FALSE
            /\ Goto(x, "write")
            /\ UNCHANGED <<stale, built>>
    /\ UNCHANGED <<spec, manifest, prefix, dworld, dman, lock, storeId, pixiId,
                   runs, w, foreign>>

Write(x) ==
    /\ pc[x] = "write"
    /\ manifest' = w[x]
    /\ foreign' = (foreign \/ (x # "l" /\ StoreOf(x) # "S"))
    /\ Goto(x, "solve")
    /\ UNCHANGED <<spec, prefix, dset, dworld, dman, lock, storeId, pixiId,
                   runs, w, built, stale>>

\* pixi installs whatever manifest is on disk when it runs.
Solve(x) ==
    /\ pc[x] = "solve"
    /\ prefix' = manifest
    /\ Goto(x, "record")
    /\ UNCHANGED <<spec, manifest, dset, dworld, dman, lock, storeId, pixiId,
                   runs, w, built, stale, foreign>>

\* The agent records the digest of the world it rendered; the manager drops
\* the agent's digest instead.
Record(x) ==
    /\ pc[x] = "record"
    /\ IF x = "m"
       THEN /\ dset' = FALSE
            /\ UNCHANGED <<dworld, dman, built>>
       ELSE /\ dset' = TRUE
            /\ dworld' = w[x]
            /\ dman' = IF x = "l" THEN "old" ELSE w[x]
            /\ built' = built \cup {ProgOf[x]}
    /\ Goto(x, "release")
    /\ UNCHANGED <<spec, manifest, prefix, lock, storeId, pixiId, runs, w,
                   stale, foreign>>

Release(x) ==
    /\ pc[x] \in {"release", "refused"}
    /\ IF Locks(x) THEN lock' = [lock EXCEPT ![StoreOf(x)] = "free"]
       ELSE UNCHANGED lock
    /\ runs' = [runs EXCEPT ![x] = @ + 1]
    /\ Goto(x, IF runs[x] + 1 < MaxRuns(x) THEN "idle" ELSE "done")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, storeId, pixiId,
                   w, built, stale, foreign>>

Terminated ==
    /\ \A x \in Actors : pc[x] = "done"
    /\ UNCHANGED vars

Next ==
    \/ \E x \in Actors :
        \/ Acquire(x) \/ Check(x) \/ Fast(x) \/ Write(x) \/ Solve(x)
        \/ Record(x) \/ Release(x)
    \/ ManagerIds
    \/ Terminated

Spec == Init /\ [][Next]_vars

\* Every program a sync built still has its packages in the prefix.
NoShrink == built \subseteq prefix
NoStaleFastPath == ~stale
NoForeignSolve == ~foreign
================================================================================
