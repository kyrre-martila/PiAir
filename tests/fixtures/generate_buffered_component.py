"""Independent Python cryptography fixture; no PiAir/sender implementation used.
Receiver-observed layout at openairplay/airplay2-receiver 6c343d3 audio.py.
This is NOT a capture or evidence of negotiation/Apple TV compatibility.
Run at repository root; writes tests/fixtures/buffered-component.bin.
"""
from pathlib import Path
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

# ALAC CPE, instance 0, reserved 12 bits, explicit count, shift 0, escape 1.
bits = '001' + '0000' + '0'*12 + '1' + '00' + '1'
bits += f'{2:032b}'
for sample in [-32768, 32767, 0, -1]:
    bits += f'{sample & 65535:016b}'
bits += '111'  # END
bits += '0' * (-len(bits) % 8)
payload = int(bits, 2).to_bytes(len(bits)//8, 'big')
header = bytes.fromhex('80fffffefffffffe00000000')
nonce = bytes(8)
sealed = ChaCha20Poly1305(bytes(range(32))).encrypt(bytes(4)+nonce, payload, header[4:12])
packet = (2+12+len(sealed)+8).to_bytes(2, 'big') + header + sealed + nonce
Path('tests/fixtures/buffered-component.bin').write_bytes(packet)
