"""Synthetic consecutive-request oracle, NOT an Apple TV capture.
Run from repository root with Python cryptography 50.0.0. No PiAir imports.
HAP control framing: SHA512 HKDF, LE length/AAD, 00000000 || LE64 counter.
CSeq 3/4 models two plaintext pair-verify requests followed by GET and SETUP.
RX deliberately advances twice before TX advances once (independent directions).
"""
from pathlib import Path
import plistlib
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

root = Path('tests/fixtures')

def frames(direction, blocks, counter=0):
    key = HKDF(algorithm=hashes.SHA512(), length=32, salt=b'Control-Salt',
               info=f'Control-{direction}-Encryption-Key'.encode()).derive(bytes(range(32)))
    result = b''
    for block in blocks:
        length = len(block).to_bytes(2, 'little')
        result += length + ChaCha20Poly1305(key).encrypt(
            bytes(4) + counter.to_bytes(8, 'little'), block, length)
        counter += 1
    return result

def save(name, value):
    (root / ('sequence-' + name)).write_bytes(value)

body = plistlib.dumps(dict(deviceID='12:34:56:78:9A:BC:DE:F0',
    sessionUUID='00112233-4455-4677-8899-AABBCCDDEEFF',
    timingProtocol='NTP', timingPort=7010), fmt=plistlib.FMT_BINARY, sort_keys=False)
save('setup.plist', body)

def request(method, uri, seq, payload=b''):
    return (f'{method} {uri} RTSP/1.0\r\nCSeq: {seq}\r\n'
        'User-Agent: PiAir/0.1\r\nX-Apple-ProtocolVersion: 1\r\n'
        f'Content-Length: {len(payload)}\r\n'
        'Client-Instance: 123456789ABCDEF0\r\nDACP-ID: 123456789ABCDEF0\r\n'
        'Active-Remote: 123456789\r\nX-Apple-Client-Name: PiAir\r\n'
        + ('Content-Type: application/x-apple-binary-plist\r\n' if payload else '')
        + '\r\n').encode() + payload

save('get.tx', frames('Write', [request('GET', '/info', 3)]))
save('setup.tx', frames('Write', [request('SETUP', 'rtsp://127.0.0.1/305419896', 4, body)], 1))
# Structurally realistic binary /info plist with synthetic display metadata.
info = plistlib.dumps(dict(features=(1 << 9) | (1 << 19) | (1 << 41),
    statusFlags=0, model='AppleTV6,2', name='Fixture', displayData=bytes(1400)),
    fmt=plistlib.FMT_BINARY, sort_keys=False)
save('info.plist', info)
reply = (f'RTSP/1.0 200 OK\r\nCSeq: 3\r\nContent-Length: {len(info)}\r\n'
         'Content-Type: application/x-apple-binary-plist\r\n\r\n').encode() + info
assert 1024 < len(reply) < 2048
save('info.rx', frames('Read', [reply[:1024], reply[1024:]]))
for name, response, counter in [
    ('setup.rx', b'RTSP/1.0 200 OK\r\nCSeq: 4\r\n\r\n', 2),
    ('reset.rx', b'RTSP/1.0 200 OK\r\nCSeq: 4\r\n\r\n', 0),
    ('wrong-cseq.rx', b'RTSP/1.0 200 OK\r\nCSeq: 5\r\n\r\n', 2),
    ('duplicate-length.rx', b'RTSP/1.0 200 OK\r\nCSeq: 4\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n', 2),
    ('extra.rx', b'RTSP/1.0 200 OK\r\nCSeq: 4\r\n\r\nx', 2),
]:
    save(name, frames('Read', [response], counter))
# A separate three-block plaintext exercises outbound chunking without changing
# the realistic GET/SETUP envelope above. All byte values, including zero, occur.
bulk = bytes(range(256)) * 9
save('bulk.tx', frames('Write', [bulk[i:i+1024] for i in range(0, len(bulk), 1024)]))
