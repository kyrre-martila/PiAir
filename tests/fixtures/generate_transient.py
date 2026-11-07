"""Independent transient vector generator: Python integer SRP arithmetic and
cryptography HKDF/ChaCha20Poly1305. Optional developer tool; never run by PiAir.
RFC 5054 3072-bit group (RFC 3526 group 15), generator 5. Synthetic test secrets.
Run from repository root: python3 tests/fixtures/generate_transient.py
"""
from pathlib import Path
from hashlib import sha512
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

N = int("ffffffffffffffffc90fdaa22168c234c4c6628b80dc1cd129024e088a67cc74020bbea63b139b22514a08798e3404ddef9519b3cd3a431b302b0a6df25f14374fe1356d6d51c245e485b576625e7ec6f44c42e9a637ed6b0bff5cb6f406b7edee386bfb5a899fa5ae9f24117c4b1fe649286651ece45b3dc2007cb8a163bf0598da48361c55d39a69163fa8fd24cf5f83655d23dca3ad961c62f356208552bb9ed529077096966d670c354e4abc9804f1746c08ca18217c32905e462e36ce3be39e772c180e86039b2783a2ec07a28fb5c55df06f4c52c9de2bcbf6955817183995497cea956ae515d2261898fa051015728e5a8aaac42dad33170d04507a33a85521abdf1cba64ecfb850458dbef0a8aea71575d060c7db3970f85a6e1e4c7abf5ae8cdb0933d71e8c94e04a25619dcee3d2261ad2ee6bf12ffa06d98a0864d87602733ec86a64521f2b18177b200cbbe117577a615d6c770988c0bad946e208e24fa074e5ab3143db5bfce0fd108e4b82d120a93ad2caffffffffffffffff", 16)
g = 5

def h(*parts):
    return sha512(b"".join(parts)).digest()

def pad(n):
    return n.to_bytes(384, "big")

def minimal(n):
    return n.to_bytes((n.bit_length() + 7)//8, "big")

a = int.from_bytes(bytes(range(1, 33)), "big")
b = int.from_bytes(bytes(range(33, 65)), "big")
salt = bytes(range(16))
x = int.from_bytes(h(salt.lstrip(b"\0"), h(b"Pair-Setup:3939")), "big")
k = int.from_bytes(h(pad(N), pad(g)), "big")
v = pow(g, x, N)
A = pow(g, a, N)
while pad(A)[0] != 0:
    a += 1
    A = pow(g, a, N)
B = (k*v + pow(g, b, N)) % N
while pad(B)[0] != 0:
    b += 1
    B = (k*v + pow(g, b, N)) % N
u = int.from_bytes(h(pad(A), pad(B)), "big")
S = pow((A * pow(v, u, N)) % N, b, N)
assert S == pow((B - k*pow(g, x, N)) % N, a + u*x, N)
K = h(minimal(S))
xor = bytes(a ^ b for a, b in zip(h(pad(N)), h(minimal(g))))
proof = h(xor, h(b"Pair-Setup"), salt.lstrip(b"\0"), minimal(A), minimal(B), K)
receiver_proof = h(minimal(A), proof, K)
values = {"private": minimal(a), "salt": salt, "a_public": pad(A),
          "b_public": pad(B), "key": K, "proof": proof, "receiver_proof": receiver_proof}
for direction in ["Write", "Read"]:
    key = HKDF(algorithm=hashes.SHA512(), length=32, salt=b"Control-Salt",
               info=f"Control-{direction}-Encryption-Key".encode()).derive(K)
    plain = b"OPTIONS * RTSP/1.0\r\nCSeq: 3\r\n\r\n"
    length = len(plain).to_bytes(2, "little")
    values[direction.lower()+"_frame"] = length + ChaCha20Poly1305(key).encrypt(bytes(12), plain, length)
Path("tests/fixtures/transient-srp.toml").write_text(
    "# Synthetic independent SRP and control fixture; see generate_transient.py.\n" +
    "".join(f'{k} = "{v.hex()}"\n' for k, v in values.items()))
