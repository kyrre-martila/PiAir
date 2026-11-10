Independent Python 3 `plistlib` binary-plist fixtures, stored as hex for review.
No receiver/source implementation generated these values.

Reproduction:

```python
import plistlib
anchor = dict(networkTimeSecs=1, networkTimeFrac=-(1 << 63) + 123,
              networkTimeTimelineID=-26263587901610, networkTimeFlags=0,
              rtpTime=0, rate=1)
print(plistlib.dumps(anchor, fmt=plistlib.FMT_BINARY, sort_keys=False).hex())
print(plistlib.dumps(['192.0.2.2', '192.0.2.1'], fmt=plistlib.FMT_BINARY).hex())
```

Clock-ID bits are `0xffffe81d082c2f56`; high-bit fraction checks signed
8-byte integer encoding. The IEEE packet known answer remains explicitly
hand assembled in `tests.rs`, independent of PiAir's packet encoder.
