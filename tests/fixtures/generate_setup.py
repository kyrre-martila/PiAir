"""Independent NTP audio SETUP fixture: Python plistlib + cryptography, no PiAir code.
Run from repository root. All identities and key bytes below are synthetic.
"""
from pathlib import Path
import plistlib
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

body = plistlib.dumps({
    'deviceID': '02:01:02:03:04:05',
    'sessionUUID': '00112233-4455-4677-8899-AABBCCDDEEFF',
    'timingProtocol': 'NTP',
    'timingPort': 7010,
}, fmt=plistlib.FMT_BINARY, sort_keys=False)
request = (
    'SETUP rtsp://127.0.0.1/305419896 RTSP/1.0\r\n'
    'CSeq: 1\r\nUser-Agent: PiAir/0.1\r\nX-Apple-ProtocolVersion: 1\r\n'
    f'Content-Length: {len(body)}\r\n'
    'Client-Instance: 123456789ABCDEF0\r\nDACP-ID: 123456789ABCDEF0\r\n'
    'Active-Remote: 123456789\r\nX-Apple-Client-Name: PiAir\r\n'
    'Content-Type: application/x-apple-binary-plist\r\n\r\n'
).encode() + body
key = HKDF(algorithm=hashes.SHA512(), length=32, salt=b'Control-Salt',
           info=b'Control-Write-Encryption-Key').derive(bytes(range(32)))
frames = b''
for counter, start in enumerate(range(0, len(request), 1024)):
    block = request[start:start+1024]
    length = len(block).to_bytes(2, 'little')
    frames += length + ChaCha20Poly1305(key).encrypt(
        bytes(4) + counter.to_bytes(8, 'little'), block, length)
Path('tests/fixtures/initial-setup.plist').write_bytes(body)
Path('tests/fixtures/initial-setup.frame').write_bytes(frames)
