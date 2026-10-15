#!/usr/bin/env python3
"""Regenerate public TLS test fixtures (requires Python cryptography).

The private key is public test data. Never use these certificates or key outside tests.
Tests load the checked-in files and do not require Python or OpenSSL.
"""
from datetime import datetime, timezone
from ipaddress import ip_address
from pathlib import Path
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID, ExtendedKeyUsageOID

HERE = Path(__file__).resolve().parent
START = datetime(2020, 1, 1, tzinfo=timezone.utc)
END = datetime(2100, 1, 1, tzinfo=timezone.utc)

def name(label):
    return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, label)])

def write_cert(filename, cert):
    (HERE / filename).write_bytes(cert.public_bytes(serialization.Encoding.PEM))

def root(label):
    key = ec.generate_private_key(ec.SECP256R1())
    cert = (x509.CertificateBuilder().subject_name(name(label)).issuer_name(name(label))
            .public_key(key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(START).not_valid_after(END)
            .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
            .add_extension(x509.KeyUsage(digital_signature=True, content_commitment=False,
                key_encipherment=False, data_encipherment=False, key_agreement=False,
                key_cert_sign=True, crl_sign=True, encipher_only=None, decipher_only=None), critical=True)
            .sign(key, hashes.SHA256()))
    return key, cert

ca_key, ca = root('Ergent PUBLIC TEST CA - never deploy')
_, other = root('Ergent unrelated PUBLIC TEST CA - never deploy')
write_cert('ca.pem', ca)
write_cert('other-ca.pem', other)
server_key = ec.generate_private_key(ec.SECP256R1())
(HERE / 'server-key.pem').write_bytes(server_key.private_bytes(
    serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
for filename, address, end in [
    ('server.pem', '127.0.0.1', END),
    ('wrong-ip.pem', '127.0.0.2', END),
    ('expired.pem', '127.0.0.1', datetime(2021, 1, 1, tzinfo=timezone.utc)),
]:
    cert = (x509.CertificateBuilder().subject_name(name('Ergent PUBLIC TEST server'))
            .issuer_name(ca.subject).public_key(server_key.public_key())
            .serial_number(x509.random_serial_number()).not_valid_before(START).not_valid_after(end)
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(x509.SubjectAlternativeName([x509.IPAddress(ip_address(address))]), critical=False)
            .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
            .sign(ca_key, hashes.SHA256()))
    write_cert(filename, cert)
