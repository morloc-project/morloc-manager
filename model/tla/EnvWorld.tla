------------------------------- MODULE EnvWorld -------------------------------
(* One environment's declared world and its solved prefix (DEP-3, DEP-4,     *)
(* DEP-6, DEP-11, DEP-12). The environment's store S holds the specs; the    *)
(* pixi project holds the manifest, the prefix and the solve digest. Agent s *)
(* builds program a under the environment's own state; agent p builds b      *)
(* under a private MORLOC_STATE P; the manager m re-solves the store's       *)
(* world; the legacy agent l is an older mim that keys its store and lock on *)
(* P; r is a manager rebuild that fails and restores the manifest it         *)
(* snapshotted. Before its solve an agent may write into the pixi project    *)
(* (a snapshot of a program's local packages). s installs a program whose    *)
(* scratch build z it supersedes. An agent may die before it records its     *)
(* solve. Ids: 0 is absent. Only the manager writes them.                    *)
EXTENDS Naturals, FiniteSets

CONSTANT Variant, Legacy, WrongMarker, Modify, SRuns
\* "design": the design.
\* "state_store": the hook keys its store and lock on MORLOC_STATE, no ids.
\* "no_manager_lock": the manager solves without the environment's lock.
\* "digest_only": the fast path trusts the digest without the manifest.
\* "no_id_check": the hook does not compare the store's and pixi ids.
\* "write_before_check": the agent writes into the pixi project before it
\* compares ids.
\* "unlocked_snapshot": r snapshots without the lock and takes the lock
\* again for the restore.
\* "commit_first": an agent writes its spec and removes the specs it
\* supersedes before its solve, instead of after it.
\* Legacy: an older agent also runs. WrongMarker: p's MORLOC_ENV_STORE
\* names P. Modify: r runs.

Actors == {"s", "p", "m", "l", "r"}
Managers == {"m", "r"}
Stores == {"S", "P"}
Progs == {"a", "b", "c", "z"}
ProgOf == [s |-> "a", p |-> "b", l |-> "c", m |-> "none", r |-> "none"]

StoreOf(x) ==
    CASE x = "s" -> "S"
      [] x = "l" -> "P"
      [] x \in Managers -> "S"
      [] x = "p" -> IF Variant = "state_store" \/ WrongMarker THEN "P" ELSE "S"

IdCheck(x) == x # "l" /\ Variant \notin {"state_store", "no_id_check"}
ManifestCheck(x) == x # "l" /\ Variant # "digest_only"
Locks(x) == x # "m" \/ Variant # "no_manager_lock"
EarlyWrite == Variant = "write_before_check"
UnlockedSnap == Variant = "unlocked_snapshot"
CommitFirst == Variant = "commit_first"
Sup(x) == IF x = "s" THEN {"z"} ELSE {}

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
    snap,      \* the manifest r snapshotted
    pc,
    runs,      \* actor -> completed runs
    w,         \* actor -> world it gathered
    built,     \* programs whose sync succeeded and are not superseded
    stale,     \* a sync fast-pathed while its program was missing from the prefix
    foreign    \* the shared pixi project was written from a store other than S

vars == <<spec, manifest, prefix, dset, dworld, dman, lock, storeId, pixiId,
          snap, pc, runs, w, built, stale, foreign>>

Committed(x) == (spec[StoreOf(x)] \ Sup(x)) \cup {ProgOf[x]}

MaxRuns(x) == CASE x = "s" -> SRuns [] x = "l" -> (IF Legacy THEN 1 ELSE 0)
                [] x = "r" -> (IF Modify THEN 1 ELSE 0)
                [] OTHER -> 1

Init ==
    /\ spec = [st \in Stores |-> IF st = "S" THEN {"z"} ELSE {}]
    /\ manifest = {"z"}
    /\ prefix = {"z"}
    /\ dset = FALSE
    /\ dworld = {}
    /\ dman = {}
    /\ lock = [st \in Stores |-> "free"]
    /\ storeId = [st \in Stores |-> 0]
    /\ pixiId = 0
    /\ snap = {}
    /\ pc = [x \in Actors |-> IF MaxRuns(x) = 0 THEN "done" ELSE "idle"]
    /\ runs = [x \in Actors |-> 0]
    /\ w = [x \in Actors |-> {}]
    /\ built = {"z"}
    /\ stale = FALSE
    /\ foreign = FALSE

Goto(x, l) == pc' = [pc EXCEPT ![x] = l]

TakeLock(x) ==
    IF Locks(x)
    THEN /\ lock[StoreOf(x)] = "free"
         /\ lock' = [lock EXCEPT ![StoreOf(x)] = x]
    ELSE UNCHANGED lock

FreeLock(x) ==
    IF Locks(x) THEN lock' = [lock EXCEPT ![StoreOf(x)] = "free"]
    ELSE UNCHANGED lock

Acquire(x) ==
    /\ x # "r"
    /\ pc[x] = "idle"
    /\ TakeLock(x)
    /\ Goto(x, CASE x = "m" -> "ids"
                 [] EarlyWrite -> "early"
                 [] OTHER -> "check")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, storeId, pixiId,
                   snap, runs, w, built, stale, foreign>>

\* The manager's data dir holds both the store and the pixi project, so it
\* knows they belong together: it creates the store's id if absent and
\* copies it into the pixi project.
ManagerIds(x) ==
    /\ x \in Managers
    /\ pc[x] = "ids"
    /\ storeId' = [storeId EXCEPT !["S"] = IF @ = 0 THEN 1 ELSE @]
    /\ pixiId' = storeId'["S"]
    /\ w' = [w EXCEPT ![x] = spec["S"]]
    /\ Goto(x, "write")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, lock, snap, runs,
                   built, stale, foreign>>

\* An agent compares ids and gathers its store's world with its own spec in
\* place of the ones it supersedes.
Check(x) ==
    /\ x \notin Managers
    /\ pc[x] = "check"
    /\ IF IdCheck(x) /\ (storeId[StoreOf(x)] = 0 \/ storeId[StoreOf(x)] # pixiId)
       THEN /\ Goto(x, "refused")
            /\ UNCHANGED <<spec, w>>
       ELSE /\ w' = [w EXCEPT ![x] = Committed(x)]
            /\ IF CommitFirst
               THEN spec' = [spec EXCEPT ![StoreOf(x)] = Committed(x)]
               ELSE UNCHANGED spec
            /\ Goto(x, IF EarlyWrite THEN "fast" ELSE "early")
    /\ UNCHANGED <<manifest, prefix, dset, dworld, dman, lock, storeId, pixiId,
                   snap, runs, built, stale, foreign>>

\* An agent's write into the pixi project ahead of its solve.
EarlyWriteStep(x) ==
    /\ x \notin Managers
    /\ pc[x] = "early"
    /\ foreign' = (foreign \/ (x # "l" /\ StoreOf(x) # "S"))
    /\ Goto(x, IF EarlyWrite THEN "check" ELSE "fast")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, lock, storeId,
                   pixiId, snap, runs, w, built, stale>>

Fast(x) ==
    /\ pc[x] = "fast"
    /\ IF dset /\ dworld = w[x] /\ (ManifestCheck(x) => dman = manifest)
       THEN /\ stale' = (stale \/ ProgOf[x] \notin prefix)
            /\ spec' = [spec EXCEPT ![StoreOf(x)] = Committed(x)]
            /\ built' = (built \ Sup(x)) \cup {ProgOf[x]}
            /\ Goto(x, "release")
            /\ UNCHANGED dset
       ELSE /\ dset' = FALSE
            /\ Goto(x, "write")
            /\ UNCHANGED <<stale, built, spec>>
    /\ UNCHANGED <<manifest, prefix, dworld, dman, lock, storeId, pixiId,
                   snap, runs, w, foreign>>

Write(x) ==
    /\ pc[x] = "write"
    /\ manifest' = w[x]
    /\ foreign' = (foreign \/ (x # "l" /\ StoreOf(x) # "S"))
    /\ Goto(x, "solve")
    /\ UNCHANGED <<spec, prefix, dset, dworld, dman, lock, storeId, pixiId,
                   snap, runs, w, built, stale>>

\* pixi installs whatever manifest is on disk when it runs.
Solve(x) ==
    /\ pc[x] = "solve"
    /\ prefix' = manifest
    /\ Goto(x, "record")
    /\ UNCHANGED <<spec, manifest, dset, dworld, dman, lock, storeId, pixiId,
                   snap, runs, w, built, stale, foreign>>

\* The agent records the digest of the world it rendered and commits its
\* spec; a manager drops the agent's digest instead. r's rebuild then fails.
Record(x) ==
    /\ pc[x] = "record"
    /\ IF x \in Managers
       THEN /\ dset' = FALSE
            /\ UNCHANGED <<dworld, dman, built, spec>>
       ELSE /\ dset' = TRUE
            /\ dworld' = w[x]
            /\ dman' = IF x = "l" THEN "old" ELSE w[x]
            /\ spec' = [spec EXCEPT ![StoreOf(x)] = Committed(x)]
            /\ built' = (built \ Sup(x)) \cup {ProgOf[x]}
    /\ Goto(x, IF x = "r" THEN "fail" ELSE "release")
    /\ UNCHANGED <<manifest, prefix, lock, storeId, pixiId, snap, runs, w,
                   stale, foreign>>

\* An agent dies before it records; its lock goes with it.
Crash(x) ==
    /\ x \in {"s", "p"}
    /\ pc[x] \in {"solve", "record"}
    /\ Goto(x, "release")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, lock, storeId,
                   pixiId, snap, runs, w, built, stale, foreign>>

\* r: snapshot, then the rebuild under the lock; with an unlocked snapshot
\* the snapshot comes first and the restore takes the lock again.
RAcquire ==
    /\ pc["r"] = IF UnlockedSnap THEN "acq" ELSE "idle"
    /\ TakeLock("r")
    /\ Goto("r", IF UnlockedSnap THEN "ids" ELSE "snap")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, storeId, pixiId,
                   snap, runs, w, built, stale, foreign>>

RSnap ==
    /\ pc["r"] = IF UnlockedSnap THEN "idle" ELSE "snap"
    /\ snap' = manifest
    /\ Goto("r", IF UnlockedSnap THEN "acq" ELSE "ids")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, lock, storeId,
                   pixiId, runs, w, built, stale, foreign>>

RFail ==
    /\ pc["r"] = "fail"
    /\ IF UnlockedSnap THEN FreeLock("r") ELSE UNCHANGED lock
    /\ Goto("r", IF UnlockedSnap THEN "reacq" ELSE "restore")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, storeId, pixiId,
                   snap, runs, w, built, stale, foreign>>

RReacquire ==
    /\ pc["r"] = "reacq"
    /\ TakeLock("r")
    /\ Goto("r", "restore")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, storeId, pixiId,
                   snap, runs, w, built, stale, foreign>>

\* The restore puts the snapshotted manifest back and forgets the agent's
\* solve, so the next sync solves the store's world again.
RRestore ==
    /\ pc["r"] = "restore"
    /\ manifest' = snap
    /\ dset' = FALSE
    /\ Goto("r", "release")
    /\ UNCHANGED <<spec, prefix, dworld, dman, lock, storeId, pixiId, snap, runs,
                   w, built, stale, foreign>>

Release(x) ==
    /\ pc[x] \in {"release", "refused"}
    /\ FreeLock(x)
    /\ runs' = [runs EXCEPT ![x] = @ + 1]
    /\ Goto(x, IF runs[x] + 1 < MaxRuns(x) THEN "idle" ELSE "done")
    /\ UNCHANGED <<spec, manifest, prefix, dset, dworld, dman, storeId, pixiId,
                   snap, w, built, stale, foreign>>

Terminated ==
    /\ \A x \in Actors : pc[x] = "done"
    /\ UNCHANGED vars

Next ==
    \/ \E x \in Actors :
        \/ Acquire(x) \/ ManagerIds(x) \/ Check(x) \/ EarlyWriteStep(x)
        \/ Fast(x) \/ Write(x) \/ Solve(x) \/ Record(x) \/ Crash(x)
        \/ Release(x)
    \/ RAcquire \/ RSnap \/ RFail \/ RReacquire \/ RRestore
    \/ Terminated

Spec == Init /\ [][Next]_vars

\* Every program a sync built still has its packages in the prefix, and
\* its spec in the store. z leaves the prefix when the sync superseding it
\* solves, before that sync commits or dies, so only its spec is kept.
NoShrink == built \ {"z"} \subseteq prefix
NoLostSpec == built \cap {"a", "z"} \subseteq spec["S"]
NoStaleFastPath == ~stale
NoForeignSolve == ~foreign
================================================================================
