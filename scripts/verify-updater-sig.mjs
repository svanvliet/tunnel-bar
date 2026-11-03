#!/usr/bin/env node
// Verifies a Tauri updater signature (minisign format) against the public key in src-tauri/tauri.conf.json.
// Usage: node scripts/verify-updater-sig.mjs <file> [<file>.sig] [expected-version]
// Checks the key ID, the file signature and the trusted comment's signature, and (if given) that the trusted
// comment carries version:<expected-version>, which requireSignedVersion needs.
import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";

const [file, sigFile = `${file}.sig`, expectedVersion] = process.argv.slice(2);
if (!file) {
  console.error("usage: verify-updater-sig.mjs <file> [<file>.sig] [expected-version]");
  process.exit(2);
}

const fail = (msg) => {
  console.error(`FAIL: ${msg}`);
  process.exit(1);
};
// Tauri wraps the minisign text files in base64; line 2 of each is the base64 payload.
const minisignLines = (b64) => Buffer.from(b64.trim(), "base64").toString("utf8").split(/\r?\n/);

const conf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const pk = Buffer.from(minisignLines(conf.plugins.updater.pubkey)[1], "base64");
if (pk.length !== 42 || pk.subarray(0, 2).toString() !== "Ed") fail("unexpected public key format");
const keyId = pk.subarray(2, 10);
const publicKey = createPublicKey({
  key: { kty: "OKP", crv: "Ed25519", x: pk.subarray(10).toString("base64url") },
  format: "jwk",
});

const lines = minisignLines(readFileSync(sigFile, "utf8"));
const sig = Buffer.from(lines[1], "base64");
const trusted = lines[2].replace(/^trusted comment: /, "");
const globalSig = Buffer.from(lines[3], "base64");
if (sig.length !== 74) fail("unexpected signature format");
const alg = sig.subarray(0, 2).toString();
if (!sig.subarray(2, 10).equals(keyId)) fail("signature key ID does not match the public key");

const data = readFileSync(file);
const signed = alg === "ED" ? createHash("blake2b512").update(data).digest() : data;
if (!verify(null, signed, publicKey, sig.subarray(10))) fail("file signature is invalid");
if (!verify(null, Buffer.concat([sig.subarray(10), Buffer.from(trusted)]), publicKey, globalSig)) {
  fail("trusted comment signature is invalid");
}
if (expectedVersion && !trusted.split("\t").includes(`version:${expectedVersion}`)) {
  fail(`trusted comment lacks version:${expectedVersion} (${trusted})`);
}
console.log(`OK: key ${Buffer.from(keyId).reverse().toString("hex").toUpperCase()}, ${trusted}`);
