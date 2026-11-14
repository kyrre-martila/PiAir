# Speaker trust and credentials

Discovery is passive mDNS. Advertisements, friendly names, IPs and advertised public
identifiers are unverified. Approving an exact canonical `device:<12 lowercase hex>`
ID records user intent; it is not cryptographic authentication. Setup asks for
explicit approval and pins the selected endpoint and authentication method.
Unknown receivers are never automatically approved or contacted.

Connections and negotiated sockets check current approval, selected endpoint,
validated peer/ports and credentials. Changed or ambiguous identity/endpoint
observations fail closed. There is no receiver or authentication fallback.
`allowlist remove ID` revokes approval locally without deleting credentials;
`unpair ID` removes local credentials without contacting the receiver, clearing
selection or revoking approval. Use the appropriate operation deliberately.

## Authentication

Persistent HAP enrollment requires genuine receiver-supported PIN delivery or an
independently obtained HAP setup code. An AirPlay password is not a setup code.
Do not invent codes or guess methods. PiAir checks SRP proof, encrypted enrollment
and signed receiver identity before saving. Pair-verify then requires that exact
stored identifier/key. Changed keys never silently replace credentials. Advertised
`pi` is only a change-detection pin, not the authenticated pairing identifier.

Transient authentication, explicitly selected for compatible HomePods, establishes
connection-scoped SRP encryption. It does not verify a persistent receiver identity
or save persistent pairing credentials. It is never a fallback for failed HAP.
First enrollment relies on physical code provenance and correct endpoint approval;
transient use relies on explicit endpoint selection and local-network trust.

Once enrollment M5 is sent, the receiver may retain a controller record even if
M6 or local persistence fails. Inspect local state and receiver-side controller
records before retrying; no automatic remote cleanup or enrollment retry occurs.
Unsupported access policy, password, PIN, group/proxy or MFi requirements fail
closed. Successful authentication or packet delivery does not prove audibility.

## Local storage and logs

State lives in `receiver-state/` next to configuration: owned nonsymlink 0700
directory, owned regular single-link 0600 files, bounded strict parsing and
O_NOFOLLOW reads. Writes use synced private temporary files; credentials are
persisted without clobbering existing enrollment. Credentials are not encrypted
at rest. Parent directories and same-user processes are trusted. Protect backups;
never commit configuration, pairing codes, identity files or credentials.

Corrupt or permissive state fails closed. Repair ownership/permissions deliberately.
Legacy unsupported credential schemas require explicit local removal and enrollment;
there is no silent trust migration. Current valid credentials survive updates,
rollback and default uninstall. See [installation and recovery](INSTALL.md).

Secrets use zeroizing owners, but complete process-memory erasure is not guaranteed:
allocator/serialization/big-integer copies can remain. SRP big-integer arithmetic
is not constant-time. No security audit or protection against hostile same-user
processes is claimed. Application Rust forbids unsafe code; dependencies may contain
it. Logs exclude raw TXT, PCM, PINs, credentials and key material; review even
verbose logs privately before sharing. PiAir code is Apache-2.0, and the bundled
recording retains its [separate attribution](../tests/assets/audio/LICENSE.md).
