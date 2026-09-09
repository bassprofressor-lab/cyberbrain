# Certificates for the hub's TLS tests

Self-signed, for tests only. The key in here opens nothing: it was generated for
`localhost`, it is in a public repository, and nothing is ever served with it outside
`cargo test`. It is checked in rather than generated during the test run so that the test
does not need a certificate library, and so that a failure means the hub changed rather
than the generator did.

`hub-test-ca.pem` signs `hub-test-leaf.pem`; the leaf is `CN=localhost` with
`subjectAltName = DNS:localhost, IP:127.0.0.1` and runs to 2126, because a test that starts
failing on a date is a test that fails for a reason nobody is looking for.

Regenerate with:

    openssl req -x509 -newkey rsa:2048 -sha256 -days 36500 -nodes \
      -keyout ca.key -out hub-test-ca.pem \
      -subj "/CN=Cyberbrain hub test CA" -addext "basicConstraints=critical,CA:TRUE"
    openssl req -newkey rsa:2048 -nodes -keyout hub-test-leaf-key.pem -out leaf.csr \
      -subj "/CN=localhost"
    printf "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n" > ext.cnf
    openssl x509 -req -in leaf.csr -CA hub-test-ca.pem -CAkey ca.key -CAcreateserial \
      -out hub-test-leaf.pem -days 36500 -sha256 -extfile ext.cnf

The fingerprint the hub prints for this leaf is the one `openssl x509 -fingerprint -sha256`
gives for the same file, which is what `the_fingerprint_is_the_one_a_browser_shows` checks.
