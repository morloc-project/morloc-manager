# Provisioning (PROV)

### PROV-1 A compiler comes from a release named by a tag
Intent: proposed
Code: conforms 2026-11-15

`latest` resolves to the newest release; a bare version `X.Y.Z` means tag
`vX.Y.Z`. The release carries a manifest naming, per platform, the compiler
binary, the runtime source, the SHA-256 of every asset, and the contract
versions of that release. A platform the manifest does not list is refused
with a pointer to a container backend.

### PROV-2 Every downloaded artifact is verified before use
Intent: proposed
Code: deviates F10

Every file mim downloads and later executes or installs is checked against a
SHA-256 published with it. A missing digest or a mismatch is a refusal,
never a warning.

### PROV-3 A release whose contracts this mim cannot read is refused at once
Intent: proposed
Code: conforms 2026-10-09
Tests: the_gate_refuses_every_contract_version_the_parsers_reject

Before anything is installed, each contract version in the release manifest
is compared with what this mim reads. A version mim cannot read, newer or
older, is refused with the direction of the fix (upgrade mim, or pick
another release).

### PROV-4 A provisioned release is immutable
Intent: ruled 2026-11-15
Code: deviates F118
Tests: a_republished_release_is_detected, the_runtime_version_is_the_stamps_first_line

Releases are stored once per scope, keyed by version and platform. An entry
is staged and then marked complete; only a complete entry is used, and it is
never modified. A tag whose content changes under the same version is
detected rather than served from the stale entry.

### PROV-5 The runtime is built inside the environment
Intent: proposed
Code: deviates F96
Tests: native_init_links_nothing_outside_the_environment

The runtime library, the nexus and the language bindings are built by
`morloc init` against the environment's own solved toolchain, not the
host's, and write nothing outside the environment.

### PROV-6 The runtime ABI is checked against the compiler
Intent: open
Code: unaudited

The release manifest declares an ABI version. Question: what does mim do
with it? Candidates: (a) nothing, since the runtime is always built from the
release's own source; (b) record it and have `doctor` and `freeze` refuse a
runtime whose ABI differs; (c) drop it from the manifest.

### PROV-7 A container environment has one image, rebuilt only when its inputs change
Intent: ruled 2026-11-15
Code: conforms 2026-11-15
Tests: the_agent_identity_follows_the_staged_binary

The image is built from a base image, system packages, certificates, the
compiler and mim. A rebuild is skipped when the inputs are unchanged and the
image still exists. The solved toolchain and the built runtime live outside
the image, in the environment's data.

### PROV-8 The agent is the same version as the manager
Intent: proposed
Code: conforms 2026-11-15
Tests: the_agent_identity_follows_the_staged_binary, a_dynamic_mim_is_not_staged_into_a_container

The mim that the build hook invokes inside an environment is the mim that
manages it (or a verified release of the same version for another
platform). An override exists for development.

### PROV-9 Extra certificate authorities reach every tool that downloads
Intent: ruled 2026-11-20
Code: conforms 2026-11-20
Tests: raw_der_with_a_trailing_key_is_refused, a_download_carries_the_bundle_it_trusts, a_download_for_no_environment_honours_the_users_bundle, a_download_for_an_environment_trusts_its_bundle, curl_is_built_only_by_curl_base, a_pending_trust_is_a_private_host_bundle_removed_on_drop

A certificate bundle given to an environment is validated (certificates
only, no keys), installed in its image, and exported to the solver, the
package managers, the runtime and mim's own downloads. Its fingerprints are
recorded so drift is reported. A download that belongs to no environment
honours the user's `CURL_CA_BUNDLE` or `SSL_CERT_FILE`; `demos` uses the
default environment's bundle; an interactive `new` asks for the bundle
before the version, whose check is a download.

### PROV-10 An environment image carries editors that highlight morloc
Intent: proposed
Code: conforms 2026-11-27
Tests: generates_expected_dockerfile, the_slim_image_leaves_out_the_editors, editor_files_are_staged_at_their_image_paths, editor_files_carry_both_morloc_syntaxes

The environment image installs vim and nano with morloc syntax files at
their system paths. The files' contents are an input of the image under
PROV-7. A slim deployment image carries neither.
