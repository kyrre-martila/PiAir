"""Independent Python plistlib / cryptography fixtures; no sender code used.
Synthetic values, not hardware captures. Run from the repository root.
"""
from pathlib import Path
import plistlib
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

fields = dict(audioFormat=0x40000, audioMode='default', ct=2, spf=352, sr=44100,
              type=96, isMedia=True, latencyMin=11025, latencyMax=88200,
              controlPort=6001, shk=bytes(range(32)), streamConnectionID=1,
              supportsDynamicStreamID=False)
Path('tests/fixtures/realtime-offer.plist').write_bytes(
    plistlib.dumps(dict(streams=[fields]), fmt=plistlib.FMT_BINARY))
bits = '001'+'0000'+'0'*12+'1'+'00'+'1'+f'{352:032b}'+'0'*(352*32)+'111'
bits += '0'*(-len(bits)%8)
alac = int(bits,2).to_bytes(len(bits)//8,'big')
header = bytes.fromhex('80e0ffff' 'fffffff5' '01020304')
nonce = bytes(8)
sealed = ChaCha20Poly1305(bytes(range(32))).encrypt(bytes(4)+nonce,alac,header[4:12])
Path('tests/fixtures/realtime-packet.bin').write_bytes(header+sealed+nonce)
cycle = []
cycle.append(cycle)
Path('tests/fixtures/cyclic.plist').write_bytes(plistlib.dumps(cycle,fmt=plistlib.FMT_BINARY))
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
for direction, label in [('tx', 'Events-Read-Encryption-Key'), ('rx', 'Events-Write-Encryption-Key')]:
    key = HKDF(algorithm=hashes.SHA512(), length=32, salt=b'Events-Salt', info=label.encode()).derive(bytes(range(32)))
    plain = b'event fixture'
    size = len(plain).to_bytes(2, 'little')
    frame = size + ChaCha20Poly1305(key).encrypt(bytes(12), plain, size)
    Path(f'tests/fixtures/event-{direction}.bin').write_bytes(frame)
