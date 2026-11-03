import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const actions = readFileSync(new URL("../src/launch-actions.ts", import.meta.url), "utf8");
const api = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
const forms = readFileSync(new URL("../src/modal-forms.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

// The comparison itself happens in the backend, under the lease that guards the write, and is
// tested there as `save_profile_from_form`.
test("saving sends the backend the profile the form opened with", () => {
  const save = actions.slice(actions.indexOf("async function saveProfileFromModal"), actions.indexOf("function requestDeleteProfile"));
  assert.match(save, /const baseline = context\.profileFormBaseline\(\)/);
  assert.match(save, /context\.api\.saveProfile\(profile, baseline === undefined \? undefined : JSON\.parse\(baseline\)\)/);
  assert.match(api, /invoke<LaunchProfile\[\]>\("save_profile", \{ profile, baseline \}\)/);
  assert.match(main, /profileFormBaseline: modalForms\.profileFormBaseline/);
});

test("a refused save refreshes the board behind the form and still shows the refusal", () => {
  const save = actions.slice(actions.indexOf("async function saveProfileFromModal"), actions.indexOf("function requestDeleteProfile"));
  const refresh = save.indexOf("await context.refreshLaunch(true)");
  const rethrow = save.indexOf("throw error");
  const close = save.indexOf("context.closeModal()");
  assert.ok(refresh !== -1 && refresh < rethrow && rethrow < close);
});

test("the edit form keeps the profile it opened with", () => {
  assert.match(forms, /data-profile-baseline="\$\{h\(JSON\.stringify\(profile\)\)\}"/);
  assert.match(forms, /dataset\.profileBaseline/);
});
