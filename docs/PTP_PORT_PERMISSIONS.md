# Linux / Raspberry Pi PTP port permissions

PiAir binds its local interface's UDP **319** (PTP event) and **320** (PTP general)
ports and connects each socket to the validated timing peer on the same port.
These fixed ports are required by the working protocol. With
`net.ipv4.ip_unprivileged_port_start=1024`, Linux denies these local binds unless
the executable has permission. The first failure is normally UDP 319; UDP 320
can fail independently. This happens after authenticated initial PTP SETUP; it
is a local socket failure, not receiver capability evidence or an RTSP rejection.

The diagnostic names the failed port, preserves the underlying OS error, and
recommends the capability only for `PermissionDenied`. For example, an address
already in use still reports a local bind error without recommending privileges.
If the second bind fails, Rust drops the first socket. The existing session
failure path attempts bounded TEARDOWN, releases event/timing resources and
closes control; no retry or receiver fallback is added.

## Explicit post-build step

On Raspberry Pi OS, install `libcap2-bin` if `setcap` or `getcap` is missing:

```sh
sudo apt-get install libcap2-bin
```

As **kyrre**, build and then grant the capability to that exact executable:

```sh
cd ~/PiAir
cargo build --release
# Review this small helper before running it with installation privileges.
sudo python3 scripts/grant-ptp-capability.py "$PWD/target/release/piair"
getcap "$PWD/target/release/piair"
# Expected: /home/kyrre/PiAir/target/release/piair cap_net_bind_service=ep
```

The helper accepts a canonical absolute path ending in `piair`. It rejects
symlinks in every component, group/world writable parent directories,
non-ELF files, missing execute bits, set-ID bits, and group/world writable
executables. The file must belong to root or the invoking sudo user. It resolves
`setcap` and `getcap` only in system directories, fails clearly if either is
missing, replaces the capability set with exactly `cap_net_bind_service=ep`, and
verifies the result. Repeating it is safe. It retains a file descriptor and
checks for executable replacement after the operation. No PiAir process is
started by the helper.

Stop builds/deployments while applying the capability. Use a trusted checkout,
trusted executable and trusted parent directories: validation cannot establish
that an ELF binary really is PiAir or protect against a malicious owner modifying
its own files. Path checks and the external tools are separate operations, so
the helper is not a security boundary against concurrent path changes by a
trusted directory owner. Do not run a helper from someone else's writable checkout with
sudo. The helper changes only the executable's capability metadata; it does not
change ownership or permissions of configuration, pairing credentials or
receiver-state directories.

**File capabilities may disappear whenever `cargo build --release` replaces the
executable**, and on copy/deployment to another filesystem or host. Repeat the
helper and `getcap` after each build or deployment, against the executable you
will actually run. Cargo may hardlink the executable to its `deps` artifact;
capabilities belong to the inode and therefore apply to those aliases of the
same PiAir binary too. For a separate deployed binary, substitute its absolute path
in both commands. Do not set capabilities on `cargo`, `rustc`, a shell, Python,
or a directory. Do not build with sudo.

For an already inspected, trusted path, the underlying explicit operation is:

```sh
sudo setcap cap_net_bind_service=ep /home/kyrre/PiAir/target/release/piair
getcap /home/kyrre/PiAir/target/release/piair
```

Prefer the validating helper for repeatable deployment. To revoke this grant:

```sh
sudo setcap -r /home/kyrre/PiAir/target/release/piair
getcap /home/kyrre/PiAir/target/release/piair
# Expected: no output
```

## Security scope

`CAP_NET_BIND_SERVICE` is the minimum Linux capability needed to bind these
privileged ports. It permits binding **any privileged TCP/UDP port**, not just
319/320, for processes executing this file. Grant it only to a trusted PiAir
binary and control who can execute/replace that binary. It does not grant raw
socket access, network administration, or root filesystem access. Do not grant
`CAP_NET_RAW` or `CAP_NET_ADMIN`, change global sysctls, or run PiAir as root.
PiAir must continue running as kyrre with its secure user-owned receiver-state
directories. No config/state permission relaxation is needed.

A filesystem without capability xattrs, a `nosuid` mount, a container capability
bounding set, or another host security policy can prevent this from taking
effect. If verification or playback still fails, inspect the deployment/mount
policy rather than broadening privileges. `getcap` verifies stored metadata,
not that the current execution environment honors it.

## Manual Raspberry Pi verification

Connect to `kyrre@192.168.1.43` and use the post-build sequence above. Confirm
`id -un` prints `kyrre`, and verify the executable immediately before playback:

```sh
getcap "$PWD/target/release/piair"
./target/release/piair select device:0a05559f132f --method transient
./target/release/piair -v ptp-wav-playback device:0a05559f132f \
  --experimental-owntone-transient \
  --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav \
  --seconds 15 \
  --volume-db -20
```

HomePod Stua must already be approved. Confirm the interactive prompt; explicit
volume changes can persist. Check that initial SETUP proceeds without a local
UDP bind permission error, music plays for approximately the full 15 seconds,
and session cleanup completes. The operator reported this result after PR #64
using `cap_net_bind_service=ep`; this change does not claim a new hardware test.
Apple clock TLV rejection messages are outside this permission change.

Unprivileged helper tests:

```sh
python3 -m unittest discover -s scripts -p 'test_*.py'
```
