// SPDX-License-Identifier: MIT OR Apache-2.0
//! 09-25 entry: SHA512-CRYPT ($6$) hash + verify builtins — every NS
//! passdb (Dovecot {SHA512-CRYPT}$6$…) uses this scheme, so
//! password_hash()/password_verify() grew the sha512-crypt scheme, the
//! $6$/$5$ verify prefixes, and the Dovecot prefix.

use mix::evaluator::Evaluator;
use mix::lexer::Lexer;
use mix::parser::Parser;

async fn run(src: &str) -> Result<String, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens, src);
    let stmts = parser.parse_program().map_err(|e| e.to_string())?;
    let stdout = mix::evaluator::SharedBuf::new();
    let stderr = mix::evaluator::SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    eval.execute(&stmts).await.map_err(|e| e.to_string())?;
    Ok(stdout.to_string_lossy())
}

/// Pinned vector: `printf 'vector-pw' | mkpasswd -m sha512crypt
/// -S testsalt -R 1000 --stdin` (shadow-utils mkpasswd, the same tool the
/// TODO entry was shelling out to).
const VECTOR: &str = "$6$rounds=1000$testsalt$pJiR2JZDZxtYoBdU749uokWBxsGFrMNLhFayE.be8uJClPWvdHSeiwAM9vJ0i.4hdjjgaNlHowi9/kLtLgs1k1";

#[tokio::test]
async fn sha512_crypt_hash_round_trips() {
    let out = run(
        "$h = password_hash(\"pw\", {scheme: \"sha512-crypt\", rounds: 1000})\n\
         print($h)\n\
         print(password_verify(\"pw\", $h))\n",
    )
    .await
    .expect("sha512-crypt hash");
    assert!(out.contains("$6$"), "hash must be $6$: {out}");
    assert!(out.contains("true"), "verify must match: {out}");
}

#[tokio::test]
async fn sha512_crypt_verifies_a_mkpasswd_vector() {
    // The external-vector check: a hash made by mkpasswd (the tool this
    // entry was shelling out to) must verify — this pins interop with
    // glibc/Dovecot SHA512-CRYPT.
    let src = format!(
        "print(password_verify(\"vector-pw\", \"{VECTOR}\"))\n\
         print(password_verify(\"vector-pw\", \"{{SHA512-CRYPT}}{VECTOR}\"))\n\
         print(password_verify(\"wrong\", \"{VECTOR}\"))\n"
    );
    let out = run(&src).await.expect("vector verify");
    assert!(out.contains("true"), "mkpasswd vector must verify: {out}");
    // "wrong" must be false, not an error.
    assert!(
        out.contains("false"),
        "wrong password must answer false: {out}"
    );
}

#[tokio::test]
async fn malformed_sha_crypt_hash_raises() {
    let err = run("print(password_verify(\"pw\", \"$6$not-a-hash\"))\n")
        .await
        .expect_err("malformed $6$ hash must raise");
    assert!(err.contains("sha-crypt"), "got: {err}");
    let err = run("print(password_verify(\"pw\", \"{SHA512-CRYPT}broken\"))\n")
        .await
        .expect_err("prefix without body must raise");
    assert!(err.contains("prefix"), "got: {err}");
}

/// Pinned implicit-rounds vector: `openssl passwd -6 -salt abcdefghijklmnop
/// vector-pw` (glibc crypt form, default 5000 rounds, no `rounds=` field);
/// `doveadm pw -t` confirms it. This is the shape external passdbs store.
const VECTOR_IMPLICIT: &str = "$6$abcdefghijklmnop$VMOKau8dnMqIm2asH7W/.nI7GnYIs1L77GCuD7Mo5QrAnGgYaLjp1ajzd4oUPMCEDAPAqoHdr4dgs.XzzeVCG/";

/// Split `$6$[rounds=N$]salt$hash` into (rounds field if any, salt, hash).
fn parts(h: &str) -> (Option<String>, String, String) {
    let body = h.trim().strip_prefix("$6$").expect("must start $6$");
    let f: Vec<&str> = body.split('$').collect();
    match f.as_slice() {
        [r, s, d] if r.starts_with("rounds=") => {
            (Some(r.to_string()), s.to_string(), d.to_string())
        }
        [s, d] => (None, s.to_string(), d.to_string()),
        _ => panic!("unexpected $6$ layout: {h}"),
    }
}

#[tokio::test]
async fn sha512_crypt_verifies_an_implicit_rounds_vector() {
    let src = format!(
        "print(password_verify(\"vector-pw\", \"{VECTOR_IMPLICIT}\"))\n\
         print(password_verify(\"nope\", \"{VECTOR_IMPLICIT}\"))\n"
    );
    let out = run(&src).await.expect("implicit vector verify");
    assert_eq!(out.trim(), "true\nfalse", "got: {out}");
}

/// Shape pin for emitted hashes. The crate's own salted-hash path wrote a
/// 22-char salt but hashed only its first 16, so glibc and Dovecot rejected
/// every hash it produced while password_verify (with the same truncation)
/// still round-tripped it. The emitted hash must have a 16-char salt, use the
/// implicit form at default rounds, and carry an 86-char digest.
#[tokio::test]
async fn sha512_crypt_emits_glibc_shaped_hashes() {
    let out = run("print(password_hash(\"pw\", {scheme: \"sha512-crypt\"}))\n\
         print(password_hash(\"pw\", {scheme: \"sha512-crypt\", rounds: 1000}))\n")
    .await
    .expect("sha512-crypt hash");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "got: {out}");

    let (r, salt, digest) = parts(lines[0]);
    assert_eq!(
        r, None,
        "default rounds must use the implicit form: {}",
        lines[0]
    );
    assert_eq!(
        salt.len(),
        16,
        "salt must be 16 chars (SHA-crypt cap): {}",
        lines[0]
    );
    assert_eq!(digest.len(), 86, "sha512 digest is 86 chars: {}", lines[0]);
    // {SHA512-CRYPT} + hash stays within a 127-char password column.
    assert!(
        14 + lines[0].len() <= 127,
        "too long for a 127-char column: {}",
        lines[0].len()
    );

    let (r, salt, _) = parts(lines[1]);
    assert_eq!(
        r.as_deref(),
        Some("rounds=1000"),
        "non-default rounds stay explicit: {}",
        lines[1]
    );
    assert_eq!(salt.len(), 16, "salt must be 16 chars: {}", lines[1]);
}

#[tokio::test]
async fn sha512_crypt_default_form_round_trips() {
    let out = run("$h = password_hash(\"pw\", {scheme: \"sha512-crypt\"})\n\
         print(password_verify(\"pw\", $h))\n\
         print(password_verify(\"pw\", \"{SHA512-CRYPT}\" .. $h))\n\
         print(password_verify(\"px\", $h))\n")
    .await
    .expect("round trip");
    assert_eq!(out.trim(), "true\ntrue\nfalse", "got: {out}");
}

/// A hash with a 22-char salt, which an earlier build of password_hash
/// produced: its digest covers only the first 16 salt chars, so external
/// passdbs reject it. password_verify must RAISE rather than bless it.
const BROKEN_22_SALT: &str = "$6$rounds=5000$G5T26sqP8W2d0s2b7YxL9/$D57e3Hln3BA7PJg01dl0PTb8rHjexMYyupWmdxXGFwvc.J14Oi8.B6p9f7py18eIPtNxY7j18KgwlZvTsovGZ1";

#[tokio::test]
async fn overlong_salt_hash_raises() {
    let err = run(&format!(
        "print(password_verify(\"Test1234pass\", \"{BROKEN_22_SALT}\"))\n"
    ))
    .await
    .expect_err("22-char salt must raise");
    assert!(err.contains("salt longer than 16"), "got: {err}");
    let err = run(&format!(
        "print(password_verify(\"Test1234pass\", \"{{SHA512-CRYPT}}{BROKEN_22_SALT}\"))\n"
    ))
    .await
    .expect_err("prefixed 22-char salt must raise");
    assert!(err.contains("salt longer than 16"), "got: {err}");
}
