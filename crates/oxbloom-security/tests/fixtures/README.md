# Test fixtures

TEST-ONLY keys. They are public, carry no trust and must never be used outside the
`oxbloom-security` test suite.

Generated with OpenSSL:

```bash
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out rsa_private.pem
openssl pkey -in rsa_private.pem -pubout -out rsa_public.pem
openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out ec_private.pem
openssl pkey -in ec_private.pem -pubout -out ec_public.pem
```

The EC private key is converted to PKCS#8 because `jsonwebtoken` only reads PKCS#8 EC private
keys.
