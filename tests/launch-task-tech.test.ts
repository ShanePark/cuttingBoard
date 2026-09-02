import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { commandTech, launchTaskTech } from "../src/presentation-services.ts";
import type { ContainerInfo, LaunchTask, ServiceSnapshot } from "../src/types.ts";

const task = (command: string, container: string | null = null): LaunchTask => ({ name: "t", cwd: ".", command, expected_port: null, container });

test("a saved command reveals its framework before the runtime hosting it", () => {
  assert.equal(commandTech("java -Xmx1g -classpath /app/target/classes:/home/dev/.m2/repository/org/springframework/boot/spring-boot-starter-web/2.5.12/spring-boot-starter-web-2.5.12.jar com.example.App"), "spring");
  assert.equal(commandTech("./gradlew bootRun"), "spring");
  assert.equal(commandTech("node /home/dev/app/node_modules/.bin/vite dev --host 0.0.0.0 --port 5174"), "vite");
  assert.equal(commandTech("npm run dev"), "nodejs");
  assert.equal(commandTech("python manage.py runserver 0.0.0.0:8000"), "django");
  assert.equal(commandTech("cargo run --release"), "rust");
  assert.equal(commandTech("ssh -o BatchMode=yes -o ExitOnForwardFailure=yes -fN -L 48983:localhost:8983 oasis"), "ssh");
  assert.equal(commandTech("PORT=3000 npm start"), "nodejs");
});

test("plain runtimes and unknown scripts do not borrow a framework icon", () => {
  assert.equal(commandTech("/opt/jdk/bin/java -jar build/libs/app.jar"), "java");
  assert.equal(commandTech("node invited.js"), "nodejs");
  assert.equal(commandTech("./run.sh"), null);
  assert.equal(commandTech(""), null);
});

test("a task icon comes from its service, then its container image, then its command", () => {
  const service = { tech: "spring" } as ServiceSnapshot;
  const containers = [{ name: "app-db", image: "postgres:16" } as ContainerInfo];
  assert.equal(launchTaskTech(task("./run.sh"), service, []), "spring");
  assert.equal(launchTaskTech(task("", "app-db"), null, containers), "postgresql");
  assert.equal(launchTaskTech(task("", "app-db"), null, []), "docker");
  assert.equal(launchTaskTech(task("npm run dev"), null, containers), "nodejs");
  assert.equal(launchTaskTech(task("./run.sh"), null, []), null);
});

test("the task card and its details modal draw the same inferred icon", () => {
  const rendering = readFileSync(new URL("../src/launch-rendering.ts", import.meta.url), "utf8");
  const modal = readFileSync(new URL("../src/modal-content.ts", import.meta.url), "utf8");
  assert.match(rendering, /const tech = launchTaskTech\(task, matchedService, context\.containers\)/);
  assert.match(rendering, /iconMarkup: tech \? techIcon\(tech, 44\) : uiIcon\("terminal", 25\)/);
  assert.match(modal, /const tech = launchTaskTech\(task, matchedService, containers\)/);
  assert.match(modal, /\$\{tech \? techIcon\(tech, 56\) : uiIcon\("terminal", 34\)\}/);
});
