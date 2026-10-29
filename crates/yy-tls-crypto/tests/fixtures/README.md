# TLS interoperability fixtures — TEST KEYS ONLY

These keys are public test data. Never use them for a real service or trust this CA.
Certificates and keys were generated with native OpenSSL 3.1.1 on 2026-10-29,
independently of the RustCrypto backend under test. Validity is 100 years;
leaf certificates include localhost/127.0.0.1 and both serverAuth/clientAuth.
The CA uses RSA-2048/SHA-256. Leaf keys are RSA-2048, P-256, P-384 and Ed25519.
The weak RSA-1024 self-signed certificate is mathematically valid but must be rejected.

To regenerate, run `../generate-fixtures.ps1 -OutputDirectory <scratch-directory>
-OpenSSL <openssl-executable>`, then explicitly copy `ca.pem`, the four leaf
certificates and PKCS#8 keys, the RSA PKCS#1 and EC SEC1 keys, and the two weak
RSA fixture files here. Do not copy the CA private key or scratch CSRs/configs.
Generation changes keys, serials and validity times. Normal tests use these
committed fixtures and need no OpenSSL executable.
