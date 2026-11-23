import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, closeSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { request as httpsRequest } from "node:https";
import { setTimeout as sleep } from "node:timers/promises";

const root = resolve(import.meta.dirname, "../../../..");
const unit = "registration-e2e";
const link = "regtest0";
const port = 9443;
const streamPort = 9911;
const registryBinary = process.env.LAB_REGISTRY ?? join(root, "registry/rust/target/release/lab-registry");
const collectorBinary = process.env.LAB_METRICS ?? join(root, "agent/lab-metrics");
const backupBinary = process.env.LAB_BACKUP_COLLECT ?? join(root, "backup/rust/target/release/lab-backup-collect");
const registerBinary = process.env.LAB_REGISTER_DEPLOYMENT ?? join(root, "registry/rust/target/release/lab-register-deployment");
const publishBinary = process.env.LAB_PUBLISH_RUNTIME ?? join(root, "registry/rust/target/release/lab-publish-runtime");
const dashboardBinary = process.env.LAB_DASHBOARD ?? join(root, "dashboard/rust/target/release/lab-dashboard");
const keep = process.env.REGISTRATION_KEEP_STAGE === "1";

function command(program: string, ...args: string[]): string {
  const job = spawnSync(program, args, { encoding: "utf8", timeout: 45_000 });
  if (job.error || job.status !== 0) throw new Error(program + " failed", { cause: job.error ?? job.stderr?.slice(0, 160) });
  return job.stdout;
}

function cleanupCommand(program: string, ...args: string[]): void {
  const child = spawnSync(program, args, { stdio: "ignore", timeout: 15_000 });
  if (child.status !== 0) console.error(program + " cleanup was unverified");
}

async function wait<T>(label: string, read: () => Promise<T | undefined>, seconds = 70): Promise<T> {
  const deadline = performance.now() + seconds * 1000;
  let last: unknown;
  while (performance.now() < deadline) {
    try {
      const value = await read();
      if (value) {
        console.log("PASS " + label);
        return value;
      }
    } catch (error) {
      last = error;
    }
    await sleep(1000);
  }
  throw new Error(label + " timed out", { cause: last });
}

function measure(pid: number): [number, number] {
  const stat = readFileSync("/proc/" + pid + "/stat", "utf8").replace(/^.*\)\s*/, "").trim().split(/\s+/);
  const memory = readFileSync("/proc/" + pid + "/status", "utf8").match(/^VmRSS:\s+(\d+)/m);
  assert.ok(memory);
  return [Number(stat[11]) + Number(stat[12]), Number(memory[1]) * 1024];
}

async function main(): Promise<void> {
  if (process.platform !== "linux" || process.getuid?.() !== 0 || process.env.DISPOSABLE_VM_TEST !== "1") {
    throw new Error("Requires root and DISPOSABLE_VM_TEST=1 in a disposable Linux VM");
  }
  for (const path of [registryBinary, collectorBinary, backupBinary, registerBinary, publishBinary, ...(keep ? [dashboardBinary] : [])]) {
    if (!existsSync(path)) throw new Error("Build the requested test binary first: " + path);
  }
  process.umask(0o077);
  const workspace = keep ? "/var/tmp/registration-review-state" : mkdtempSync("/var/tmp/registration-e2e-");
  if (keep) mkdirSync(workspace, { mode: 0o700 });
  const path = (name: string) => join(workspace, name);
  const write = (name: string, value: unknown) => writeFileSync(path(name), JSON.stringify(value), { mode: 0o600 });
  const ca = path("ca.pem");
  const caKey = path("ca.key");
  const token = () => randomBytes(32).toString("base64url");
  const enrollment = token();
  const reader = token();
  const writer = token();
  const publisher = token();
  const runtimeBinding = { kind: "native", provider: "systemd", instance: unit + ".service" };
  let networkAdded = false;
  let collectorStarted = false;
  let successful = false;
  let registry: ChildProcess | undefined;
  let authorized: Buffer | undefined;
  let installedKey = false;
  const authorizedKeys = "/root/.ssh/authorized_keys";
  const get = <T>(pathname: string, payload?: unknown, credential = reader, revision?: number): Promise<T> => new Promise((finish, reject) => {
    const send = payload === undefined ? undefined : Buffer.from(JSON.stringify(payload));
    const req = httpsRequest({
      host: "127.0.0.1", port, path: pathname, ca: readFileSync(ca),
      method: payload === undefined ? "GET" : revision !== undefined ? "PUT" : "POST",
      headers: { "Authorization": "Bearer " + credential, "Content-Type": "application/json",
        ...(send ? { "Content-Length": send.length } : {}), ...(revision !== undefined ? { "If-Match": revision } : {}) },
      timeout: 7000,
    }, response => {
      let bytes = "";
      response.setEncoding("utf8");
      response.on("data", chunk => { bytes += chunk; if (bytes.length > 4 * 1024 * 1024) response.destroy(new Error("Registry response too large")); });
      response.on("error", reject);
      response.on("end", () => {
        try {
          if (!response.statusCode || response.statusCode >= 300) throw new Error("Registry HTTP " + response.statusCode);
          finish(JSON.parse(bytes));
        } catch (error) { reject(error); }
      });
    });
    req.on("error", reject);
    req.on("timeout", () => req.destroy(new Error("Registry request timed out")));
    req.end(send);
  });

  const readStream = (ip: string, identity = "test-node"): Promise<{history?: unknown[]}> => new Promise((finish, reject) => {
    const req = httpsRequest({
      host: ip, port: streamPort, path: "/stream", servername: identity,
      ca: readFileSync(ca), cert: readFileSync(path("client.pem")), key: readFileSync(path("client.key")),
      minVersion: "TLSv1.3", timeout: 8000,
    }, response => {
      let bytes = "";
      response.setEncoding("utf8");
      response.on("data", chunk => {
        bytes += chunk;
        const newline = bytes.indexOf("\n");
        if (newline < 0) {
          if (bytes.length > 4 * 1024 * 1024) response.destroy(new Error("Collector frame too large"));
          return;
        }
        try { finish(JSON.parse(bytes.slice(0, newline))); }
        catch (error) { reject(error); }
        response.destroy();
      });
      response.on("error", reject);
    });
    req.on("error", reject);
    req.on("timeout", () => req.destroy(new Error("Collector request timed out")));
    req.end();
  });

  try {
    command("openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
      "-nodes", "-days", "1", "-subj", "/CN=Disposable-Test-CA", "-addext", "keyUsage=critical,keyCertSign,cRLSign",
      "-keyout", caKey, "-out", ca);
    for (const [name, san, usage] of [
      ["registry", "IP:127.0.0.1", "serverAuth"], ["node", "DNS:test-node", "serverAuth"], ["client", "DNS:test-client", "clientAuth"],
    ]) {
      command("openssl", "req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
        "-subj", "/CN=" + name, "-keyout", path(name + ".key"), "-out", path(name + ".csr"));
      writeFileSync(path(name + ".ext"), "subjectAltName=" + san + "\nextendedKeyUsage=" + usage + "\n");
      command("openssl", "x509", "-req", "-in", path(name + ".csr"), "-CA", ca, "-CAkey", caKey, "-CAcreateserial",
        "-days", "1", "-extfile", path(name + ".ext"), "-out", path(name + ".pem"));
    }
    writeFileSync(path("token"), enrollment, { mode: 0o600 });
    writeFileSync(path("publisher-token"), publisher, { mode: 0o600 });
    const registryUrl = "https://127.0.0.1:" + port;
    write("registry-operator.json", { url: registryUrl, reader_token: reader, writer_token: writer, ca_file: ca });
    write("publisher.json", { registry: registryUrl, record_id: "test-runtime", token_file: path("publisher-token"),
      ca_file: ca, unit: runtimeBinding.instance });
    write("registry.json", {
      database: path("catalog.db"), bind: "127.0.0.1", port, reader_token: reader, writer_token: writer,
      cert: path("registry.pem"), key: path("registry.key"),
      machine_enrollments: { "machine-test": {
        token_sha256: createHash("sha256").update(enrollment).digest("hex"),
        allowed_networks: ["192.0.2.0/24"], tls_name: "test-node",
      } },
      presence_tls: { ca_file: ca, cert_file: path("client.pem"), key_file: path("client.key") },
      runtime_publishers: { "test-runtime": { token: publisher, binding: runtimeBinding } },
    });
    command("ip", "link", "add", link, "type", "dummy");
    networkAdded = true;
    command("ip", "link", "set", link, "up");
    command("ip", "addr", "add", "192.0.2.10/24", "dev", link);
    const log = openSync(path("registry.log"), "w");
    try { registry = spawn(registryBinary, ["--config", path("registry.json")], { stdio: ["ignore", log, log], detached: true }); }
    finally { closeSync(log); }
    await wait("Rust registry starts", () => get("/v1/services"));
    const machine = {
      name: "Test node", host: "test-node", owner: "e2e", capabilities: ["machine"],
      endpoints: { ssh: "ssh://192.0.2.10" }, lifecycle: "active", runbook: "Disposable test only", dependencies: [],
      resources: { registration: { endpoint_keys: ["ssh"], metrics: true, probe: true },
        metrics_stream: { enabled: true, url: "https://192.0.2.10:9911/stream", server_name: "test-node" } },
      probe: { tcp_host: "192.0.2.10", tcp_port: streamPort },
    };
    await get("/v1/services/machine-test", machine, writer, 0);
    await get("/v1/services/app", { ...machine, name: "Test application", capabilities: ["test-service"],
      endpoints: { health: "http://192.0.2.10:9911/health", public: "https://example.test" },
      resources: { address_bindings: [{ machine_id: "machine-test", endpoint_keys: ["health"], probe: true }] },
    }, writer, 0);
    write("agent.json", { listen: ":" + streamPort, disks: [{ name: "root", path: "/" }], cert: path("node.pem"),
      key: path("node.key"), client_ca: ca, registration: {
        url: "https://127.0.0.1:9443/v1/machines/machine-test/presence", token_file: path("token"), ca_file: ca, interface: link,
      },
    });
    command("systemd-run", "--unit=" + unit, "--property=Type=exec", collectorBinary, "-config", path("agent.json"), "-socket", path("agent.sock"));
    collectorStarted = true;
    type MachineRow = { revision: number; spec: { endpoints: Record<string, string>; resources: { agent_presence?: { address?: string } } } };
    const registered = async (ip: string) => {
      const row = await get<MachineRow>("/v1/services/machine-test");
      return row.spec.resources.agent_presence?.address === ip ? row : undefined;
    };
    await wait("daemon self-registers without login", () => registered("192.0.2.10"));
    await wait("real cached metrics over mTLS", async () => (await readStream("192.0.2.10")).history);
    await assert.rejects(() => readStream("192.0.2.10", "wrong-node"));
    console.log("PASS wrong TLS identity rejected");

    const runtime = { name: "Disposable collector", host: machine.host, owner: machine.owner,
      capabilities: ["test-service"], lifecycle: "active", runbook: machine.runbook,
      endpoints: {}, dependencies: [], resources: { runtime_binding: runtimeBinding, deployment_generation: "review-generation" } };
    write("runtime-manifest.json", runtime);
    const managed = (action: "register" | "retire") => JSON.parse(command(registerBinary,
      "--config", path("registry-operator.json"), action, "test-runtime", path("runtime-manifest.json")));
    assert.equal(managed("register").lifecycle, "active");
    assert.equal(JSON.parse(command(publishBinary, path("publisher.json"), "--systemd")).state, "present");
    const observed = await get<{runtime_observation?: { state: string }}>("/v1/services/test-runtime");
    assert.equal(observed.runtime_observation?.state, "present");
    await assert.rejects(() => get("/v1/services/app/runtime", { state: "present", ttl_seconds: 900 }, publisher), /401|403/);
    assert.equal(managed("retire").lifecycle, "retired");
    await assert.rejects(() => get("/v1/services/test-runtime/runtime", { state: "present", ttl_seconds: 900 }, publisher), /409/);
    console.log("PASS Rust deployment registration, scoped systemd observation, and exact retirement");

    const backup = path("backrest");
    mkdirSync(join(backup, "private"), { recursive: true });
    mkdirSync(join(backup, "deployment"));
    command("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", join(backup, "private/collector-key"));
    writeFileSync(join(backup, "private/known_hosts"), "machine-test " + readFileSync("/etc/ssh/ssh_host_ed25519_key.pub", "utf8"));
    write("backrest/private/registry-reader.json", { url: "https://127.0.0.1:9443", reader_token: reader, ca_file: ca });
    write("backrest/deployment/manifest.json", { test: { machine_id: "machine-test", username: "root" } });
    writeFileSync(path("backup-config"), "Disposable configuration backup content.\n".repeat(20));
    mkdirSync("/root/.ssh", { recursive: true, mode: 0o700 });
    authorized = existsSync(authorizedKeys) ? readFileSync(authorizedKeys) : undefined;
    const pub = readFileSync(join(backup, "private/collector-key.pub"), "utf8").trim();
    writeFileSync(authorizedKeys, Buffer.concat([authorized ?? Buffer.alloc(0),
      Buffer.from('\ncommand="tar -czf - -C ' + workspace + ' backup-config",restrict ' + pub + "\n")]));
    installedKey = true;
    chmodSync(authorizedKeys, 0o600);
    command(backupBinary, "test", "--base", backup);
    command(backupBinary, "test", "clean", "--base", backup);
    console.log("PASS Rust backup collector reaches the authenticated source");

    command("ip", "addr", "del", "192.0.2.10/24", "dev", link);
    command("ip", "addr", "add", "192.0.2.11/24", "dev", link);
    const updated = await wait("new address discovered by the running daemon", () => registered("192.0.2.11"));
    assert.equal(updated.revision, 1);
    assert.equal(updated.spec.endpoints.ssh, "ssh://192.0.2.11");
    const service = await get<{spec: { endpoints: Record<string, string>; probe: {tcp_host: string} }}>("/v1/services/app");
    assert.equal(service.spec.endpoints.health, "http://192.0.2.11:9911/health");
    assert.equal(service.spec.endpoints.public, "https://example.test");
    assert.equal(service.spec.probe.tcp_host, "192.0.2.11");
    command(backupBinary, "test", "--base", backup);
    console.log("PASS real SSH backup follows the verified IP with the same pinned key");
    assert.ok((await readStream("192.0.2.11")).history);
    command("systemctl", "restart", unit);
    await wait("system-service restart recovers independently of login", async () => (await readStream("192.0.2.11")).history);
    const pid = Number(command("systemctl", "show", unit, "-p", "MainPID", "--value").trim());
    assert.ok(pid > 0);
    const [start] = measure(pid);
    const began = performance.now();
    for (let elapsed = 10; elapsed <= 60; elapsed += 10) {
      await sleep(10_000);
      console.log("Idle overhead measurement " + elapsed + " seconds");
    }
    const [finish, rss] = measure(pid);
    const rate = Number(command("getconf", "CLK_TCK").trim());
    const cpu = (finish - start) / rate / ((performance.now() - began) / 1000) * 100;
    const memory = readFileSync("/proc/meminfo", "utf8").match(/^MemTotal:\s+(\d+)/m);
    assert.ok(memory);
    const share = rss / (Number(memory[1]) * 1024);
    assert.ok(cpu < 1 && share < 0.01);
    console.log("PASS collection and address migration with idle CPU " + cpu.toFixed(4) + "% and RAM " + (share * 100).toFixed(3) + "%");
    if (keep) {
      const assets = join(root, "dashboard");
      const privateDirectory = join(assets, "private");
      mkdirSync(privateDirectory, {mode:0o700});
      for (const [source, file] of [["ca.pem","stream-ca.pem"],["client.pem","stream-client.pem"],["client.key","stream-client-key.pem"]]) {
        copyFileSync(path(source), join(privateDirectory,file));
      }
      writeFileSync(join(privateDirectory,"reader.json"), JSON.stringify({url:"https://127.0.0.1:9443",reader_token:reader,ca_file:ca}));
      writeFileSync(join(privateDirectory,"origins.json"), "[]");
      if (registry?.pid) writeFileSync(path("registry.pid"), String(registry.pid));
      command("systemd-run", "--unit=registration-review-dashboard", dashboardBinary, assets, "8089");
      registry?.unref();
      console.log("Disposable registration review retained at " + workspace);
    }
    successful = true;
  } finally {
    if (installedKey) {
      if (authorized) writeFileSync(authorizedKeys, authorized);
      else rmSync(authorizedKeys, { force: true });
    }
    if (!(keep && successful)) {
      if (collectorStarted) cleanupCommand("systemctl", "stop", unit);
      registry?.kill("SIGTERM");
      if (networkAdded) cleanupCommand("ip", "link", "del", link);
      if (!keep) rmSync(workspace, { recursive: true, force: true });
    }
  }
}

await main();
