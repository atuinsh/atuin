#!/usr/bin/env node
// seal-input.mjs [out.json] [plaintext]
//
// VIEWER-DIRECTION interop emitter: seals a viewer-input blob using the
// SHIPPED, UNMODIFIED hub viewer crypto module, exactly as `term.onData` does
// in lab_share_viewer.js -- encryptBlob(key, bytes, frameAad(INPUT, 0)) with a
// fresh random nonce per call.
//
// Its Rust counterpart (`transport.rs::open_js_sealed_input_blob`) runs
// unconditionally against the FROZEN record checked in next to this script
// (`js-sealed-input.json`), and against a freshly generated one when
// `INTEROP_INPUT` points at this script's output. It asserts that both
// implementations built the same 9-byte input AAD, that the host opens what
// the viewer sealed, and that a byte-identical replay of that same genuine
// blob is refused by the never-forget ledger -- D1's attack, driven from the
// real viewer implementation.
//
// Regenerate the frozen record (only when the hub's crypto module genuinely
// changes; the point of freezing it is that it should not):
//
//   node crates/atuin-lab-share/tests/interop/seal-input.mjs
//
// The hub lives in a separate repository, so its path is resolved as a
// sibling of this one and can be overridden:
//
//   HUB_CRYPTO=/path/to/hub/assets/js/lab_share/crypto.js node seal-input.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const hubCrypto =
  process.env.HUB_CRYPTO ??
  path.resolve(here, "../../../../../hub/assets/js/lab_share/crypto.js");
if (!fs.existsSync(hubCrypto)) {
  console.error(
    `no hub crypto module at ${hubCrypto}\n` +
      "set HUB_CRYPTO to hub/assets/js/lab_share/crypto.js",
  );
  process.exit(2);
}
const { decodeKey, encryptBlob, frameAad, FrameType } = await import(
  pathToFileURL(hubCrypto).href
);

const [, , outArg, plaintextArg] = process.argv;
const outPath = outArg ?? path.join(here, "js-sealed-input.json");

// The fragment for transport.rs's `test_key()` = [0x42; 32]. The Rust side
// re-derives this from its own key and asserts equality, so if `test_key`
// ever changes the interop test fails loudly instead of silently testing
// nothing.
const FRAGMENT = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI";
const plaintext = plaintextArg ?? "js-sealed viewer keystroke\r";

const key = await decodeKey(FRAGMENT);
if (!key) {
  console.error("BAD FRAGMENT");
  process.exit(2);
}

const aad = frameAad(FrameType.INPUT, 0);
const pt = new TextEncoder().encode(plaintext);
const blob = await encryptBlob(key, pt, aad);

const record = {
  fragment: FRAGMENT,
  aad_hex: Buffer.from(aad).toString("hex"),
  plaintext_hex: Buffer.from(pt).toString("hex"),
  blob_hex: Buffer.from(blob).toString("hex"),
};
fs.writeFileSync(outPath, `${JSON.stringify(record, null, 2)}\n`);

console.log(`sealed ${pt.length}B plaintext -> ${blob.length}B blob`);
console.log(`  hub crypto = ${hubCrypto}`);
console.log(`  aad        = ${record.aad_hex}`);
console.log(`  nonce      = ${record.blob_hex.slice(0, 24)}`);
console.log(`  wrote ${outPath}`);
