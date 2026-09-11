import assert from "node:assert/strict";
import test from "node:test";
import { isMacosBundleBuild, parseSigningIdentities, resolveSigningIdentity, signedBuildArgs } from "../scripts/tauri-cli.mjs";

const securityOutput = [
  '  1) 1111111111111111111111111111111111111111 "Apple Distribution: Example (TEAM1)"',
  '  2) 2222222222222222222222222222222222222222 "Apple Development: Example (TEAM1)"',
  '     2 valid identities found'
].join("\n");

test("parses usable signing identities from security output", () => {
  assert.deepEqual(parseSigningIdentities(securityOutput), ["Apple Development: Example (TEAM1)"]);
});

test("recognizes only signed macOS bundle builds", () => {
  assert.equal(isMacosBundleBuild(["build", "--bundles", "app"], "darwin"), true);
  assert.equal(isMacosBundleBuild(["build", "--no-bundle"], "darwin"), false);
  assert.equal(isMacosBundleBuild(["build", "--no-sign"], "darwin"), false);
  assert.equal(isMacosBundleBuild(["build", "--help"], "darwin"), false);
  assert.equal(isMacosBundleBuild(["build", "--bundles", "app"], "linux"), false);
  assert.equal(isMacosBundleBuild(["dev"], "darwin"), false);
});

test("prefers a sole Apple Development identity over distribution signing", () => {
  assert.equal(
    resolveSigningIdentity({ env: {}, securityOutput }),
    "Apple Development: Example (TEAM1)"
  );
});

test("uses an explicitly selected identity and rejects ad-hoc signing", () => {
  assert.equal(
    resolveSigningIdentity({ env: { APPLE_SIGNING_IDENTITY: "Developer ID Application: Example (TEAM1)" } }),
    "Developer ID Application: Example (TEAM1)"
  );
  assert.throws(
    () => resolveSigningIdentity({ env: { APPLE_SIGNING_IDENTITY: "-" } }),
    /ad-hoc signing/
  );
});

test("fails instead of choosing an ambiguous identity", () => {
  const output = [
    '  1) 1111111111111111111111111111111111111111 "Apple Development: One (TEAM1)"',
    '  2) 2222222222222222222222222222222222222222 "Apple Development: Two (TEAM1)"'
  ].join("\n");
  assert.throws(
    () => resolveSigningIdentity({ env: {}, securityOutput: output }),
    /Multiple macOS code-signing identities/
  );
});

test("fails clearly when no signing identity is available", () => {
  assert.throws(
    () => resolveSigningIdentity({ env: {}, securityOutput: "" }),
    /No usable macOS code-signing identity/
  );
});

test("reports a failed security query", () => {
  assert.throws(
    () => resolveSigningIdentity({ env: {}, securityOutput: "", securityError: "access denied", securityStatus: 1 }),
    /Could not inspect macOS code-signing identities: access denied/
  );
});

test("adds a macOS signing override to bundle builds", () => {
  const args = signedBuildArgs(["build", "--bundles", "app"], { APPLE_SIGNING_IDENTITY: "Apple Development: Example (TEAM1)" }, "darwin");
  assert.deepEqual(args.slice(0, 3), ["build", "--bundles", "app"]);
  assert.equal(args[3], "--config");
  assert.deepEqual(JSON.parse(args[4]!), { bundle: { macOS: { signingIdentity: "Apple Development: Example (TEAM1)" } } });
});
