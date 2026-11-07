"""Independent fixture generator; optional Python cryptography 50.0.0.
No PiAir source or Rust crypto used. Not run by cargo or CI; no network.
HAP R2 section 6.5.2. Shared secret 00..1f, counters 0 and 1.
Run from repository root: python tests/fixtures/generate_control.py
"""
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

for direction in ["Write", "Read"]:
    key = HKDF(algorithm=hashes.SHA512(), length=32, salt=b"Control-Salt",
               info=f"Control-{direction}-Encryption-Key".encode()).derive(bytes(range(32)))
    for counter, plain in enumerate([b"OPTIONS * RTSP/1.0\r\nCSeq: 3\r\n\r\n", b"second block"]):
        length = len(plain).to_bytes(2, "little")
        nonce = bytes(4) + counter.to_bytes(8, "little")
        print((length + ChaCha20Poly1305(key).encrypt(nonce, plain, length)).hex())
